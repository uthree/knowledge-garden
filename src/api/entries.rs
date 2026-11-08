use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::AppState;
use crate::auth::{Auth, Viewer};
use crate::error::{Error, Result};
use crate::model::*;
use crate::policy::{VisibilityFilter, can_edit, can_read};
use crate::store::{EntryFilter, EntryMetaUpdate, NewEntry, NewLink, VersionContent};

const MAX_PROVENANCE_BYTES: usize = 64 * 1024;

pub async fn kinds() -> Json<Value> {
    let kinds: Vec<Value> = EntryKind::ALL
        .iter()
        .map(|k| {
            json!({
                "kind": k,
                "description": k.description(),
                "default_status": k.default_status(),
                "recommended_sections": k.recommended_sections().iter().map(|s| s.label).collect::<Vec<_>>(),
            })
        })
        .collect();
    Json(json!({
        "kinds": kinds,
        "visibilities": Visibility::ALL,
        "statuses": Status::ALL,
        "relations": Relation::ALL,
    }))
}

/// Loads an entry's access facts, returning 404 if it is missing or not readable.
pub(super) async fn readable_entry(state: &AppState, viewer: Option<&Principal>, id: &str) -> Result<EntryAccess> {
    state
        .store
        .entry_access(id)
        .await?
        .filter(|e| can_read(&state.config, viewer, e))
        .ok_or(Error::NotFound)
}

async fn editable_entry(state: &AppState, auth: &Auth, id: &str) -> Result<EntryAccess> {
    auth.require(Scope::EntriesWrite)?;
    let entry = readable_entry(state, Some(&auth.0), id).await?;
    if !can_edit(&auth.0, &entry) {
        return Err(Error::forbidden("only the author or an admin can change this entry"));
    }
    Ok(entry)
}

fn validate_provenance(p: Option<Value>) -> Result<Value> {
    let p = p.unwrap_or_else(|| json!({}));
    if !p.is_object() {
        return Err(Error::bad_request("provenance must be a JSON object"));
    }
    if p.to_string().len() > MAX_PROVENANCE_BYTES {
        return Err(Error::bad_request("provenance is too large"));
    }
    Ok(p)
}

fn content(
    title: &str,
    summary: Option<String>,
    body: String,
    lang: Option<String>,
    provenance: Value,
) -> Result<VersionContent> {
    let title = validate_title(title).map_err(Error::BadRequest)?;
    let summary = validate_summary(summary).map_err(Error::BadRequest)?;
    validate_body(&body).map_err(Error::BadRequest)?;
    let lang = lang.map(|l| l.trim().to_string()).filter(|l| !l.is_empty());
    if lang.as_ref().is_some_and(|l| l.len() > 16) {
        return Err(Error::bad_request(
            "lang must be a short language tag such as 'ja' or 'en'",
        ));
    }
    Ok(VersionContent {
        title,
        summary,
        body,
        lang,
        provenance,
    })
}

#[derive(Deserialize)]
pub struct LinkRequest {
    /// A local entry id, a local entry URI, or the URI of an entry on another Garden.
    pub target: String,
    pub relation: Relation,
    pub target_version: Option<i64>,
    pub note: Option<String>,
}

struct ResolvedLink {
    target_uri: String,
    target_id: Option<String>,
    req: LinkRequest,
}

async fn resolve_link(state: &AppState, viewer: &Principal, req: LinkRequest) -> Result<ResolvedLink> {
    let target = req.target.trim();
    let local_id = match state.config.local_entry_id(target) {
        Some(id) => Some(id.to_string()),
        None if target.starts_with("https://") || target.starts_with("http://") => {
            state.store.entry_id_by_uri(target).await?
        }
        None => Some(target.to_string()),
    };
    let (target_uri, target_id) = match local_id {
        Some(id) => {
            let entry = readable_entry(state, Some(viewer), &id)
                .await
                .map_err(|_| Error::bad_request(format!("link target not found: {target}")))?;
            if let Some(v) = req.target_version
                && (v < 1 || v > entry.current_version)
            {
                return Err(Error::bad_request(format!("target has no version {v}")));
            }
            (entry.uri, Some(entry.id))
        }
        None => (target.to_string(), None),
    };
    if req.note.as_ref().is_some_and(|n| n.chars().count() > 2_000) {
        return Err(Error::bad_request("link note is longer than 2000 characters"));
    }
    Ok(ResolvedLink {
        target_uri,
        target_id,
        req,
    })
}

async fn store_link(state: &AppState, source: &EntryAccess, by: &Principal, link: ResolvedLink) -> Result<String> {
    if link.target_id.as_deref() == Some(source.id.as_str()) {
        return Err(Error::bad_request("an entry cannot link to itself"));
    }
    state
        .store
        .add_link(NewLink {
            source_id: source.id.clone(),
            target_uri: link.target_uri,
            target_id: link.target_id,
            target_version: link.req.target_version,
            relation: link.req.relation,
            note: link.req.note,
            created_by: by.user.id.clone(),
        })
        .await
}

#[derive(Deserialize)]
pub struct CreateEntryRequest {
    pub kind: EntryKind,
    pub title: String,
    pub summary: Option<String>,
    pub body: String,
    pub lang: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub visibility: Option<Visibility>,
    pub status: Option<Status>,
    pub provenance: Option<Value>,
    #[serde(default)]
    pub links: Vec<LinkRequest>,
}

pub async fn create(
    State(state): State<AppState>,
    auth: Auth,
    Json(req): Json<CreateEntryRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    auth.require(Scope::EntriesWrite)?;
    let tags = normalize_tags(&req.tags).map_err(Error::BadRequest)?;
    let provenance = validate_provenance(req.provenance)?;
    let warnings = req.kind.content_warnings(&req.body, &provenance);
    let content = content(&req.title, req.summary, req.body, req.lang, provenance)?;
    // Resolve links first so a bad link rejects the whole request.
    let mut links = Vec::new();
    for l in req.links {
        links.push(resolve_link(&state, &auth.0, l).await?);
    }

    let id = uuid::Uuid::now_v7().to_string();
    let uri = state.config.entry_uri(&id);
    state
        .store
        .create_entry(NewEntry {
            id: id.clone(),
            uri: uri.clone(),
            origin: state.config.base_url.clone(),
            kind: req.kind,
            visibility: req.visibility.unwrap_or(state.config.default_visibility),
            status: req.status.unwrap_or(req.kind.default_status()),
            author_id: auth.0.user.id.clone(),
            token_id: auth.0.token.id.clone(),
            tags,
            content,
        })
        .await?;
    let source = EntryAccess {
        id: id.clone(),
        uri,
        visibility: Visibility::Private,
        author_id: auth.0.user.id.clone(),
        current_version: 1,
    };
    for link in links {
        store_link(&state, &source, &auth.0, link).await?;
    }
    let entry = state.store.get_entry(&id, None).await?.ok_or(Error::NotFound)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "entry": entry, "warnings": warnings })),
    ))
}

#[derive(Deserialize)]
pub struct GetQuery {
    pub version: Option<i64>,
}

pub async fn get(
    State(state): State<AppState>,
    Viewer(viewer): Viewer,
    Path(id): Path<String>,
    Query(q): Query<GetQuery>,
) -> Result<Json<Entry>> {
    readable_entry(&state, viewer.as_ref(), &id).await?;
    let entry = state.store.get_entry(&id, q.version).await?.ok_or(Error::NotFound)?;
    Ok(Json(entry))
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub kind: Option<EntryKind>,
    pub status: Option<Status>,
    pub author: Option<String>,
    /// Comma-separated; entries must have all of these tags.
    pub tag: Option<String>,
    /// Comma-separated; entries must have at least one of these tags.
    pub any_tag: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

fn split_tags(s: Option<String>) -> Result<Vec<String>> {
    let raw: Vec<String> = s.unwrap_or_default().split(',').map(str::to_string).collect();
    normalize_tags(&raw).map_err(Error::BadRequest)
}

pub async fn list(
    State(state): State<AppState>,
    Viewer(viewer): Viewer,
    Query(q): Query<ListQuery>,
) -> Result<Json<Page<EntrySummary>>> {
    let filter = EntryFilter {
        kind: q.kind,
        status: q.status,
        author: q.author,
        tags_all: split_tags(q.tag)?,
        tags_any: split_tags(q.any_tag)?,
        limit: q.limit.unwrap_or(20).clamp(1, 100),
        offset: q.offset.unwrap_or(0).max(0),
    };
    let vis = VisibilityFilter::for_viewer(&state.config, viewer.as_ref());
    Ok(Json(state.store.list_entries(&filter, &vis).await?))
}

pub async fn versions(
    State(state): State<AppState>,
    Viewer(viewer): Viewer,
    Path(id): Path<String>,
) -> Result<Json<Vec<VersionInfo>>> {
    readable_entry(&state, viewer.as_ref(), &id).await?;
    Ok(Json(state.store.list_versions(&id).await?))
}

/// Fields left out are copied from the current version.
#[derive(Deserialize)]
pub struct ReviseRequest {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub body: Option<String>,
    pub lang: Option<String>,
    pub provenance: Option<Value>,
}

pub async fn revise(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
    Json(req): Json<ReviseRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    editable_entry(&state, &auth, &id).await?;
    let current = state.store.get_entry(&id, None).await?.ok_or(Error::NotFound)?;
    let provenance = match req.provenance {
        Some(p) => validate_provenance(Some(p))?,
        None => current.provenance,
    };
    let body = req.body.unwrap_or(current.body);
    let warnings = current.kind.content_warnings(&body, &provenance);
    let content = content(
        &req.title.unwrap_or(current.title),
        req.summary.or(current.summary),
        body,
        req.lang.or(current.lang),
        provenance,
    )?;
    let version = state.store.add_version(&id, &auth.0.token.id, content).await?;
    let entry = state
        .store
        .get_entry(&id, Some(version))
        .await?
        .ok_or(Error::NotFound)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "entry": entry, "warnings": warnings })),
    ))
}

#[derive(Deserialize)]
pub struct PatchRequest {
    pub visibility: Option<Visibility>,
    pub status: Option<Status>,
    pub tags: Option<Vec<String>>,
}

/// Changes metadata that is not part of a version: visibility, status and tags.
pub async fn patch(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
    Json(req): Json<PatchRequest>,
) -> Result<Json<Entry>> {
    editable_entry(&state, &auth, &id).await?;
    let tags = req
        .tags
        .map(|t| normalize_tags(&t))
        .transpose()
        .map_err(Error::BadRequest)?;
    state
        .store
        .update_entry_meta(
            &id,
            EntryMetaUpdate {
                visibility: req.visibility,
                status: req.status,
                tags,
            },
        )
        .await?;
    Ok(Json(state.store.get_entry(&id, None).await?.ok_or(Error::NotFound)?))
}

pub async fn links(
    State(state): State<AppState>,
    Viewer(viewer): Viewer,
    Path(id): Path<String>,
) -> Result<Json<Links>> {
    readable_entry(&state, viewer.as_ref(), &id).await?;
    let vis = VisibilityFilter::for_viewer(&state.config, viewer.as_ref());
    Ok(Json(state.store.links(&id, &vis).await?))
}

pub async fn add_link(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
    Json(req): Json<LinkRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    let source = editable_entry(&state, &auth, &id).await?;
    let link = resolve_link(&state, &auth.0, req).await?;
    let link_id = store_link(&state, &source, &auth.0, link).await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": link_id }))))
}

#[derive(Deserialize)]
pub struct TagsQuery {
    pub limit: Option<i64>,
}

pub async fn tags(
    State(state): State<AppState>,
    Viewer(viewer): Viewer,
    Query(q): Query<TagsQuery>,
) -> Result<Json<Vec<TagCount>>> {
    let vis = VisibilityFilter::for_viewer(&state.config, viewer.as_ref());
    Ok(Json(
        state
            .store
            .tag_counts(&vis, q.limit.unwrap_or(100).clamp(1, 500))
            .await?,
    ))
}
