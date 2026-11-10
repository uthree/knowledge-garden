//! The pages. Each handler checks the form, calls the API handler that does
//! the work, and renders the result (or the error) as HTML.

use axum::Json;
use axum::extract::multipart::MultipartRejection;
use axum::extract::{Form, Multipart, Path, Query, State};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use chrono::{Duration, Utc};
use maud::{Markup, html};
use serde::Deserialize;
use serde_json::Value;

use super::view::{self, Current, csrf_field, notice_error, notice_info, time, urlencode};
use super::{Ctx, markdown, redirect, redirect_with_cookie, safe_next, wants_html};
use crate::api::{accounts, attachments, comments, entries, index, search};
use crate::auth::{Viewer, generate_secret, hash_password, hash_secret, validate_password, verify_password};
use crate::config::Registration;
use crate::error::{Error, Result};
use crate::model::*;
use crate::policy::can_edit;
use crate::store::{NewSession, NewUser, RegexField};

const PAGE_SIZE: i64 = 20;

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn parse_opt<T: std::str::FromStr<Err = String>>(s: Option<String>) -> Result<Option<T>> {
    non_empty(s).map(|s| s.parse().map_err(Error::BadRequest)).transpose()
}

fn parse_json_object(s: Option<String>, field: &str) -> Result<Option<Value>> {
    let Some(s) = non_empty(s) else { return Ok(None) };
    let v: Value =
        serde_json::from_str(&s).map_err(|e| Error::BadRequest(format!("{field} が JSON として読めません: {e}")))?;
    if !v.is_object() {
        return Err(Error::BadRequest(format!(
            "{field} は JSON オブジェクト（{{...}}）にしてください"
        )));
    }
    Ok(Some(v))
}

fn split_tags(s: &str) -> Vec<String> {
    s.split([',', '、', ' ', '\u{3000}', '\n'])
        .map(|t| t.trim().trim_start_matches('#').to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

// ---------------------------------------------------------------- home and search

#[derive(Deserialize, Default)]
pub struct HomeQuery {
    q: Option<String>,
    kind: Option<String>,
    status: Option<String>,
    tag: Option<String>,
    author: Option<String>,
    origin: Option<String>,
    offset: Option<i64>,
}

pub async fn home(ctx: Ctx, state: State<crate::AppState>, uri: Uri) -> Response {
    if !wants_html(&ctx.headers) {
        return index::api_index(state).await.into_response();
    }
    let q: HomeQuery = Query::try_from_uri(&uri).map(|q| q.0).unwrap_or_default();
    let r = home_page(&ctx, q).await;
    ctx.finish(r).await
}

async fn home_page(ctx: &Ctx, q: HomeQuery) -> Result<Response> {
    let kind: Option<EntryKind> = parse_opt(q.kind)?;
    let status: Option<Status> = parse_opt(q.status)?;
    let tag = non_empty(q.tag);
    let author = non_empty(q.author);
    let origin = non_empty(q.origin);
    let query = non_empty(q.q).unwrap_or_default();
    let offset = q.offset.unwrap_or(0).max(0);
    let state = State(ctx.state.clone());
    let viewer = Viewer(ctx.viewer().cloned());

    let (rows, total): (Vec<Markup>, i64) = if query.is_empty() {
        let Json(page) = entries::list(
            state,
            viewer,
            Query(entries::ListQuery {
                kind,
                status,
                author: author.clone(),
                tag: tag.clone(),
                any_tag: None,
                has_reproduction: None,
                origin: origin.clone(),
                since: None,
                until: None,
                limit: Some(PAGE_SIZE),
                offset: Some(offset),
            }),
        )
        .await?;
        (page.items.iter().map(|e| entry_row(e, None)).collect(), page.total)
    } else {
        let Json(page) = search::search(
            state,
            viewer,
            Query(search::SearchParams {
                q: Some(query.clone()),
                regex: None,
                regex_field: RegexField::All,
                kind,
                status,
                author: author.clone(),
                tag: tag.clone(),
                any_tag: None,
                has_reproduction: None,
                origin: origin.clone(),
                since: None,
                until: None,
                limit: Some(PAGE_SIZE),
                offset: Some(offset),
            }),
        )
        .await?;
        (
            page.items
                .iter()
                .map(|h| entry_row(&h.entry, h.snippet.as_deref()))
                .collect(),
            page.total,
        )
    };

    let title = if !query.is_empty() {
        format!("「{query}」の検索結果")
    } else if let Some(t) = &tag {
        format!("#{t}")
    } else if let Some(k) = kind {
        view::kind_label(k).to_string()
    } else if let Some(a) = &author {
        format!("@{a} の記事")
    } else {
        match origin.as_deref() {
            Some("local") => "この Garden の記事".into(),
            Some("remote") => "ほかの Garden の記事".into(),
            _ => "すべての記事".into(),
        }
    };

    // Links to neighbouring pages keep every filter but the offset.
    let mut params: Vec<(&str, String)> = Vec::new();
    if !query.is_empty() {
        params.push(("q", query.clone()));
    }
    if let Some(k) = kind {
        params.push(("kind", k.as_str().into()));
    }
    if let Some(s) = status {
        params.push(("status", s.as_str().into()));
    }
    for (k, v) in [("tag", &tag), ("author", &author), ("origin", &origin)] {
        if let Some(v) = v {
            params.push((k, v.clone()));
        }
    }
    let page_link = |off: i64| {
        let mut ser = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in &params {
            ser.append_pair(k, v);
        }
        if off > 0 {
            ser.append_pair("offset", &off.to_string());
        }
        format!("/?{}", ser.finish())
    };

    let body = html! {
        div.kg-heading {
            h1.at-title { (title) }
            span.at-small.at-faint { (total) " 件" }
        }
        @if kind.is_some() {
            @if let Some(k) = kind { p.at-muted.kg-lead { (k.description()) } }
        }
        @if rows.is_empty() {
            div.kg-empty {
                @if query.is_empty() {
                    p.at-muted { "まだ記事がありません。" }
                    @if ctx.session.is_some() { a.at-button.at-button--primary href="/new" { "最初の記事を書く" } }
                } @else {
                    p.at-muted { "一致する記事はありません。語を減らすか、別の言い方で探してみてください。" }
                }
            }
        } @else {
            ol.kg-entries { @for r in &rows { (r) } }
            @if total > PAGE_SIZE {
                nav.kg-pager aria-label="ページ" {
                    @if offset > 0 {
                        a.at-button.at-button--sm href=(page_link((offset - PAGE_SIZE).max(0))) { span data-icon="chevron-left" {} "前へ" }
                    }
                    span.at-small.at-faint { (offset + 1) "–" ((offset + PAGE_SIZE).min(total)) " / " (total) }
                    @if offset + PAGE_SIZE < total {
                        a.at-button.at-button--sm href=(page_link(offset + PAGE_SIZE)) { "次へ" span data-icon="chevron-right" {} }
                    }
                }
            }
        }
    };
    let current = Current {
        kind,
        tag,
        origin,
        page: Some("home"),
    };
    Ok(ctx.page(&title, current, &query, body).await)
}

fn entry_row(e: &EntrySummary, snippet: Option<&str>) -> Markup {
    html! {
        li.kg-entry {
            div.kg-entry__meta {
                span.at-badge { span data-icon=(view::kind_icon(e.kind)) {} (view::kind_label(e.kind)) }
                (view::status_badge(e.status))
                (view::repro_stats(&e.reproductions))
                @if !e.is_local { span.at-badge.at-badge--info data-tooltip="ほかの Garden から複製" { (view::host_of(&e.origin)) } }
                @if e.visibility != Visibility::Public { span.at-badge { (view::visibility_label(e.visibility)) } }
            }
            a.kg-entry__title href={"/entries/" (e.id)} { (e.title) }
            @if let Some(s) = snippet {
                p.kg-entry__summary { (s) }
            } @else if let Some(s) = &e.summary {
                p.kg-entry__summary { (s) }
            }
            div.kg-entry__foot {
                (view::byline(&e.author, &e.posted_via))
                span.at-faint { (time(&e.updated_at)) }
                (view::tag_links(&e.tags))
            }
        }
    }
}

// ---------------------------------------------------------------- entry page

#[derive(Deserialize, Default)]
pub struct CommentForm {
    csrf: Option<String>,
    kind: Option<String>,
    body: Option<String>,
    result: Option<String>,
    environment: Option<String>,
    parent_id: Option<String>,
}

pub async fn entry(
    ctx: Ctx,
    state: State<crate::AppState>,
    viewer: std::result::Result<Viewer, Error>,
    Path(id): Path<String>,
    uri: Uri,
) -> Response {
    if !wants_html(&ctx.headers) {
        // The entry's URI: answer agents and peers with JSON, as the API does.
        let r = async {
            let q = Query::<entries::GetQuery>::try_from_uri(&uri).map_err(|e| Error::BadRequest(e.body_text()))?;
            entries::get(state, viewer?, Path(id), q).await
        }
        .await;
        return r.into_response();
    }
    #[derive(Deserialize, Default)]
    struct V {
        version: Option<i64>,
    }
    let v: V = Query::try_from_uri(&uri).map(|q| q.0).unwrap_or_default();
    let r = entry_page(&ctx, &id, v.version, None, None).await;
    ctx.finish(r).await
}

async fn entry_page(
    ctx: &Ctx,
    id: &str,
    version: Option<i64>,
    draft: Option<&CommentForm>,
    error: Option<&str>,
) -> Result<Response> {
    let state = || State(ctx.state.clone());
    let viewer = || Viewer(ctx.viewer().cloned());
    let Json(e) = entries::get(
        state(),
        viewer(),
        Path(id.to_string()),
        Query(entries::GetQuery { version }),
    )
    .await?;
    let Json(versions) = entries::versions(state(), viewer(), Path(id.to_string())).await?;
    let Json(links) = entries::links(state(), viewer(), Path(id.to_string())).await?;
    let Json(all_comments) = comments::list(state(), viewer(), Path(id.to_string())).await?;
    let access = ctx.state.store.entry_access(id).await?.ok_or(Error::NotFound)?;
    let editable = ctx.viewer().is_some_and(|p| can_edit(p, &access));
    let can_comment = ctx.viewer().is_some_and(|p| p.has(Scope::CommentsWrite));
    let old = e.version != e.current_version;
    let has_provenance = e.provenance.as_object().is_some_and(|o| !o.is_empty());

    let top: Vec<&Comment> = all_comments.iter().filter(|c| c.parent_id.is_none()).collect();
    let replies = |parent: &str| -> Vec<&Comment> {
        all_comments
            .iter()
            .filter(|c| c.parent_id.as_deref() == Some(parent))
            .collect()
    };

    let body = html! {
        article.kg-article {
            @if old {
                (notice_info(&format!("これは版 {} です。最新は版 {} です。", e.version, e.current_version)))
                p { a href={"/entries/" (e.id)} { "最新の版を読む" } }
            }
            header.kg-article__header {
                div.kg-entry__meta {
                    a.at-badge href={"/?kind=" (e.kind.as_str())} { span data-icon=(view::kind_icon(e.kind)) {} (view::kind_label(e.kind)) }
                    (view::status_badge(e.status))
                    (view::repro_stats(&e.reproductions))
                    @if e.visibility != Visibility::Public { span.at-badge { (view::visibility_label(e.visibility)) } }
                    @if !e.is_local {
                        a.at-badge.at-badge--info href=(e.uri) rel="noopener noreferrer" data-tooltip="元の Garden で開く" {
                            (view::host_of(&e.origin)) " から複製"
                        }
                    }
                }
                h1.at-title.kg-article__title { (e.title) }
                @if let Some(s) = &e.summary { p.kg-lead.at-muted { (s) } }
                div.kg-entry__foot {
                    (view::byline(&e.author, &e.posted_via))
                    span.at-faint { "版 " (e.version) " · " (time(&e.version_created_at)) }
                    (view::tag_links(&e.tags))
                    span.at-spacer {}
                    @if editable && !old {
                        a.at-button.at-button--sm href={"/entries/" (e.id) "/edit"} { "編集" }
                    }
                }
            }
            div.at-markdown.kg-body { (markdown::render_with(&e.body, &e.id, &e.attachments)) }

            @if !e.attachments.is_empty() || editable {
                (attachments_section(&e, editable && !old))
            }

            @if has_provenance {
                details.kg-details {
                    summary { "実行環境・出典（provenance）" }
                    div.at-markdown__code { pre { code { (serde_json::to_string_pretty(&e.provenance).unwrap_or_default()) } } }
                }
            }

            @if !links.outgoing.is_empty() || !links.incoming.is_empty() {
                section.kg-section {
                    h2.at-heading { "関連する記事" }
                    ul.kg-links {
                        @for l in &links.outgoing {
                            li {
                                span.at-badge { (view::relation_label(l.relation)) }
                                @match (&l.target_id, &l.target_title) {
                                    (Some(tid), Some(title)) => a href={"/entries/" (tid)} { (title) },
                                    _ => a href=(l.target_uri) rel="noopener noreferrer" { (l.target_uri) },
                                }
                                @if let Some(n) = &l.note { span.at-faint { " — " (n) } }
                            }
                        }
                        @for l in &links.incoming {
                            li {
                                span.at-badge.at-badge--accent { "この記事を" (view::relation_label(l.relation)) }
                                a href={"/entries/" (l.source_id)} { (l.source_title) }
                                @if let Some(n) = &l.note { span.at-faint { " — " (n) } }
                            }
                        }
                    }
                }
            }

            @if versions.len() > 1 {
                details.kg-details {
                    summary { "版の履歴（" (versions.len()) "）" }
                    ol.kg-versions {
                        @for v in versions.iter().rev() {
                            li {
                                a href={"/entries/" (e.id) "?version=" (v.version)} aria-current=[(v.version == e.version).then_some("page")] {
                                    "版 " (v.version) ": " (v.title)
                                }
                                span.at-faint { " " (time(&v.created_at)) " · " (v.posted_via.name) }
                            }
                        }
                    }
                }
            }

            section.kg-section #comments {
                h2.at-heading { "コメントと再現報告（" (all_comments.len()) "）" }
                @if top.is_empty() {
                    p.at-muted { "まだありません。" }
                }
                ol.kg-comments {
                    @for c in &top {
                        li { (comment_view(c, &e, can_comment, ctx))
                            @let rs = replies(&c.id);
                            @if !rs.is_empty() {
                                ol.kg-comments.kg-comments--replies {
                                    @for r in rs { li { (comment_view(r, &e, false, ctx)) } }
                                }
                            }
                        }
                    }
                }
                @if can_comment {
                    (comment_form(ctx, &e, draft, error))
                } @else if ctx.session.is_none() {
                    p.at-muted { a href={"/login?next=" (urlencode(&format!("/entries/{}", e.id)))} { "ログイン" } "するとコメントや再現報告を書けます。" }
                }
            }
        }
    };
    Ok(ctx
        .page(
            &e.title,
            Current {
                kind: Some(e.kind),
                ..Default::default()
            },
            "",
            body,
        )
        .await)
}

/// `12.3 MB` style sizes.
fn human_size(bytes: i64) -> String {
    let b = bytes as f64;
    if b < 1024.0 {
        format!("{bytes} B")
    } else if b < 1024.0 * 1024.0 {
        format!("{:.1} KB", b / 1024.0)
    } else if b < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB", b / 1024.0 / 1024.0)
    } else {
        format!("{:.1} GB", b / 1024.0 / 1024.0 / 1024.0)
    }
}

fn preview_icon(p: crate::files::Preview) -> &'static str {
    use crate::files::Preview;
    match p {
        Preview::Image => "eye",
        Preview::Audio => "headphones",
        Preview::Video => "play",
        Preview::Pdf => "file",
        Preview::Text => "code",
        Preview::None => "paperclip",
    }
}

/// Whether the body (or an old version's body) already shows this file.
fn referenced(body: &str, a: &Attachment) -> bool {
    body.contains(&format!("attachment:{}", a.name))
        || body.contains(&format!("attachment:{}", crate::files::encode_path_segment(&a.name)))
}

/// The files of an entry. Files the body does not show are previewed here.
fn attachments_section(e: &Entry, editable: bool) -> Markup {
    html! {
        section.kg-section #attachments {
            div.kg-section__head {
                h2.at-heading { "添付ファイル（" (e.attachments.len()) "）" }
                @if editable {
                    a.at-button.at-button--sm href={"/entries/" (e.id) "/edit#attachments"} { span data-icon="paperclip" {} "ファイルを添付" }
                }
            }
            @if e.attachments.is_empty() {
                p.at-muted { "まだありません。画像・音声・動画・PDF などを添付すると、本文の中で表示できます。" }
            }
            ul.kg-attachments {
                @for a in &e.attachments {
                    li.kg-attachment {
                        div.kg-attachment__row {
                            span data-icon=(preview_icon(a.preview)) {}
                            a.kg-attachment__name href=(markdown::file_path(&e.id, &a.name)) { (a.name) }
                            span.at-faint { (human_size(a.size)) " · " (a.content_type) }
                            span.at-spacer {}
                            a.at-small href={(markdown::file_path(&e.id, &a.name)) "?download=1"} { "ダウンロード" }
                        }
                        @if a.preview != crate::files::Preview::None && !referenced(&e.body, a) {
                            div.kg-attachment__preview { (maud::PreEscaped(markdown::embed_html(&e.id, a, ""))) }
                        }
                    }
                }
            }
        }
    }
}

fn comment_view(c: &Comment, e: &Entry, can_reply: bool, ctx: &Ctx) -> Markup {
    html! {
        div.kg-comment #{"comment-" (c.id)} {
            div.kg-comment__head {
                (view::byline(&c.author, &c.posted_via))
                @if let Some(r) = c.result { (view::repro_badge(r)) }
                @if let Some(v) = c.entry_version { span.at-faint { "版 " (v) " に対して" } }
                span.at-faint { (time(&c.created_at)) }
            }
            div.at-markdown { (markdown::render_with(&c.body, &e.id, &e.attachments)) }
            @if let Some(env) = &c.environment {
                details.kg-details {
                    summary { "試した環境" }
                    div.at-markdown__code { pre { code { (serde_json::to_string_pretty(env).unwrap_or_default()) } } }
                }
            }
            @if can_reply {
                details.kg-reply {
                    summary.at-small { "返信" }
                    form method="post" action={"/entries/" (e.id) "/comments"} {
                        (csrf_field(ctx.session.as_ref()))
                        input type="hidden" name="parent_id" value=(c.id);
                        input type="hidden" name="kind" value="comment";
                        span.at-field.at-field--area { textarea name="body" rows="3" required aria-label="返信" {} }
                        div.kg-actions { button.at-button.at-button--sm type="submit" { "返信する" } }
                    }
                }
            }
        }
    }
}

fn comment_form(ctx: &Ctx, e: &Entry, draft: Option<&CommentForm>, error: Option<&str>) -> Markup {
    let d = |f: fn(&CommentForm) -> &Option<String>| draft.and_then(|d| f(d).clone()).unwrap_or_default();
    let kind = d(|d| &d.kind);
    let result = d(|d| &d.result);
    let is_repro = kind == "reproduction";
    html! {
        form.kg-form.kg-comment-form method="post" action={"/entries/" (e.id) "/comments#comments"} {
            h3.at-heading { "書き込む" }
            @if let Some(err) = error { (notice_error(err)) }
            (csrf_field(ctx.session.as_ref()))
            div.at-form-field {
                label for="c-kind" { "種類" }
                select.at-select #c-kind name="kind" data-kg-toggle="repro" {
                    option value="comment" selected[!is_repro] { "コメント" }
                    option value="reproduction" selected[is_repro] { "再現報告（版 " (e.current_version) " を試した結果）" }
                }
            }
            div.kg-repro data-kg-when="repro" {
                div.at-form-field {
                    label for="c-result" { "結果" }
                    select.at-select #c-result name="result" {
                        option value="" { "選んでください" }
                        @for r in ReproResult::ALL {
                            option value=(r.as_str()) selected[result == r.as_str()] { (view::repro_label(*r)) }
                        }
                    }
                }
                div.at-form-field {
                    label for="c-env" { "試した環境（JSON、任意）" }
                    span.at-field.at-field--area {
                        textarea.at-mono #c-env name="environment" rows="3" placeholder="{\"os\": \"Ubuntu 24.04\", \"gpu\": \"RTX 4090\"}" { (d(|d| &d.environment)) }
                    }
                }
            }
            div.at-form-field {
                label for="c-body" { "本文（Markdown）" }
                span.at-field.at-field--area { textarea #c-body name="body" rows="5" required { (d(|d| &d.body)) } }
            }
            div.kg-actions { button.at-button.at-button--primary type="submit" { "送信" } }
        }
    }
}

pub async fn comment(ctx: Ctx, Path(id): Path<String>, Form(f): Form<CommentForm>) -> Response {
    let r = post_comment(&ctx, &id, &f).await;
    let r = match r {
        Err(e) if e.is_correctable() && ctx.session.is_some() => {
            entry_page(&ctx, &id, None, Some(&f), Some(&e.to_string()))
                .await
                .map(|mut resp| {
                    *resp.status_mut() = e.status();
                    resp
                })
        }
        other => other,
    };
    ctx.finish(r).await
}

async fn post_comment(ctx: &Ctx, id: &str, f: &CommentForm) -> Result<Response> {
    let auth = ctx.auth()?;
    ctx.check_form(f.csrf.as_deref())?;
    let kind: CommentKind = parse_opt(f.kind.clone())?.unwrap_or(CommentKind::Comment);
    let (result, environment) = if kind == CommentKind::Reproduction {
        (
            parse_opt(f.result.clone())?,
            parse_json_object(f.environment.clone(), "試した環境")?,
        )
    } else {
        (None, None)
    };
    let (_, Json(c)) = comments::create(
        State(ctx.state.clone()),
        auth,
        Path(id.to_string()),
        Json(comments::CreateCommentRequest {
            kind,
            body: f.body.clone().unwrap_or_default(),
            parent_id: non_empty(f.parent_id.clone()),
            result,
            environment,
            entry_version: None,
        }),
    )
    .await?;
    Ok(redirect(&format!("/entries/{id}#comment-{}", c.id)))
}

// ---------------------------------------------------------------- writing

#[derive(Deserialize, Default, Clone)]
pub struct EntryForm {
    csrf: Option<String>,
    kind: Option<String>,
    title: Option<String>,
    summary: Option<String>,
    body: Option<String>,
    tags: Option<String>,
    visibility: Option<String>,
    status: Option<String>,
    provenance: Option<String>,
}

fn entry_form(ctx: &Ctx, action: &str, f: &EntryForm, editing: Option<&Entry>, error: Option<&str>) -> Markup {
    let v = |s: &Option<String>| s.clone().unwrap_or_default();
    let kind = v(&f.kind);
    let visibility = f
        .visibility
        .clone()
        .unwrap_or_else(|| ctx.state.config.default_visibility.as_str().to_string());
    let status = v(&f.status);
    html! {
        form.kg-form method="post" action=(action) {
            @if let Some(err) = error { (notice_error(err)) }
            (csrf_field(ctx.session.as_ref()))
            @if editing.is_none() {
                div.at-form-field {
                    label for="f-kind" { "種類" }
                    select.at-select #f-kind name="kind" required data-kg-template="f-body" {
                        @for k in EntryKind::ALL {
                            @let template: String = k.recommended_sections().iter().map(|s| format!("## {}\n\n", s.label)).collect();
                            option value=(k.as_str()) selected[kind == k.as_str()] data-template=(template) {
                                (view::kind_label(*k)) " — " (k.description())
                            }
                        }
                    }
                    span.at-form-field__help { "本文が空のうちは、種類に合わせた見出しの雛形が入ります（使わなくても構いません）。" }
                }
            }
            div.at-form-field {
                label for="f-title" { "タイトル" }
                span.at-field { input #f-title name="title" required maxlength="200" value=(v(&f.title)); }
            }
            div.at-form-field {
                label for="f-summary" { "要約（任意）" }
                span.at-field { input #f-summary name="summary" maxlength="1000" value=(v(&f.summary)); }
                span.at-form-field__help { "一覧と検索結果に表示されます。" }
            }
            div.at-form-field {
                label for="f-body" { "本文（Markdown）" }
                span.at-field.at-field--area { textarea.kg-editor #f-body name="body" rows="22" required { (v(&f.body)) } }
            }
            div.kg-form-row {
                div.at-form-field {
                    label for="f-tags" { "タグ" }
                    span.at-field { input #f-tags name="tags" value=(v(&f.tags)) placeholder="rust, sqlite"; }
                    span.at-form-field__help { "カンマか空白で区切ります。" }
                }
                div.at-form-field {
                    label for="f-status" { "状態" }
                    select.at-select #f-status name="status" {
                        @if editing.is_none() { option value="" { "種類の既定" } }
                        @for s in Status::ALL {
                            option value=(s.as_str()) selected[status == s.as_str()] { (view::status_label(*s)) }
                        }
                    }
                }
                div.at-form-field {
                    label for="f-visibility" { "公開範囲" }
                    select.at-select #f-visibility name="visibility" {
                        @for vis in Visibility::ALL {
                            option value=(vis.as_str()) selected[visibility == vis.as_str()] { (view::visibility_label(*vis)) }
                        }
                    }
                }
            }
            details.kg-details open[f.provenance.as_deref().is_some_and(|p| !p.trim().is_empty() && p.trim() != "{}")] {
                summary { "実行環境・出典（provenance、任意）" }
                div.at-form-field {
                    span.at-field.at-field--area {
                        textarea.at-mono name="provenance" rows="5" aria-label="provenance"
                            placeholder="{\"commit\": \"abc123\", \"command\": \"cargo bench\", \"environment\": {\"os\": \"Ubuntu 24.04\"}}" { (v(&f.provenance)) }
                    }
                    span.at-form-field__help { "実験の再現に必要な情報を JSON オブジェクトで書きます。" }
                }
            }
            div.kg-actions {
                @if let Some(e) = editing {
                    a.at-button href={"/entries/" (e.id)} { "キャンセル" }
                    button.at-button.at-button--primary type="submit" { "保存" }
                } @else {
                    button.at-button.at-button--primary type="submit" { "投稿" }
                }
            }
        }
    }
}

pub async fn new_form(ctx: Ctx) -> Response {
    let r = async {
        ctx.auth()?;
        let body = html! {
            h1.at-title { "記事を書く" }
            (entry_form(&ctx, "/new", &EntryForm::default(), None, None))
            p.at-small.at-muted { "画像・音声・動画・PDF などのファイルは、投稿したあと編集画面で添付できます。" }
        };
        Ok(ctx.page("記事を書く", Current::default(), "", body).await)
    }
    .await;
    ctx.finish(r).await
}

pub async fn create(ctx: Ctx, Form(f): Form<EntryForm>) -> Response {
    let r = match create_entry(&ctx, &f).await {
        Err(e) if e.is_correctable() && ctx.session.is_some() => {
            let body = html! {
                h1.at-title { "記事を書く" }
                (entry_form(&ctx, "/new", &f, None, Some(&e.to_string())))
            };
            let mut resp = ctx.page("記事を書く", Current::default(), "", body).await;
            *resp.status_mut() = e.status();
            Ok(resp)
        }
        other => other,
    };
    ctx.finish(r).await
}

async fn create_entry(ctx: &Ctx, f: &EntryForm) -> Result<Response> {
    let auth = ctx.auth()?;
    ctx.check_form(f.csrf.as_deref())?;
    let kind: EntryKind = parse_opt(f.kind.clone())?.ok_or_else(|| Error::bad_request("種類を選んでください"))?;
    let (_, Json(v)) = entries::create(
        State(ctx.state.clone()),
        auth,
        Json(entries::CreateEntryRequest {
            kind,
            title: f.title.clone().unwrap_or_default(),
            summary: non_empty(f.summary.clone()),
            body: f.body.clone().unwrap_or_default(),
            lang: None,
            tags: split_tags(f.tags.as_deref().unwrap_or("")),
            visibility: parse_opt(f.visibility.clone())?,
            status: parse_opt(f.status.clone())?,
            provenance: parse_json_object(f.provenance.clone(), "provenance")?,
            links: Vec::new(),
        }),
    )
    .await?;
    let id = v["entry"]["id"].as_str().unwrap_or_default();
    Ok(redirect(&format!("/entries/{id}")))
}

async fn editable(ctx: &Ctx, id: &str) -> Result<Entry> {
    let auth = ctx.auth()?;
    let access = ctx.state.store.entry_access(id).await?;
    let access = access
        .filter(|a| crate::policy::can_read(&ctx.state.config, Some(&auth.0), a))
        .ok_or(Error::NotFound)?;
    if !access.is_local {
        return Err(Error::forbidden(
            "ほかの Garden から複製した記事は、元の Garden でしか編集できません",
        ));
    }
    if !can_edit(&auth.0, &access) {
        return Err(Error::forbidden("記事を編集できるのは作者と管理者だけです"));
    }
    let mut e = ctx.state.store.get_entry(id, None).await?.ok_or(Error::NotFound)?;
    e.attachments = attachments::attachments_of(&ctx.state, id).await?;
    Ok(e)
}

fn form_from(e: &Entry) -> EntryForm {
    let has_provenance = e.provenance.as_object().is_some_and(|o| !o.is_empty());
    EntryForm {
        csrf: None,
        kind: Some(e.kind.as_str().into()),
        title: Some(e.title.clone()),
        summary: e.summary.clone(),
        body: Some(e.body.clone()),
        tags: Some(e.tags.join(", ")),
        visibility: Some(e.visibility.as_str().into()),
        status: Some(e.status.as_str().into()),
        provenance: has_provenance.then(|| serde_json::to_string_pretty(&e.provenance).unwrap_or_default()),
    }
}

async fn edit_page(ctx: &Ctx, e: &Entry, f: &EntryForm, error: Option<&str>) -> Response {
    edit_page_with(ctx, e, f, error, None).await
}

/// The files of an entry being edited: Markdown to paste, removal, and upload.
fn attachments_editor(ctx: &Ctx, e: &Entry, error: Option<&str>) -> Markup {
    let max_mb = ctx.state.config.max_attachment_bytes.div_ceil(1024 * 1024);
    html! {
        section.kg-section.kg-attachments-editor #attachments {
            h2.at-heading { "添付ファイル" }
            @if let Some(err) = error { (notice_error(err)) }
            @if !e.attachments.is_empty() {
                ul.kg-attachments {
                    @for (i, a) in e.attachments.iter().enumerate() {
                        li.kg-attachment {
                            div.kg-attachment__row {
                                @if a.preview == crate::files::Preview::Image {
                                    img.kg-attachment__thumb src=(markdown::file_path(&e.id, &a.name)) alt="" loading="lazy";
                                } @else {
                                    span data-icon=(preview_icon(a.preview)) {}
                                }
                                a.kg-attachment__name href=(markdown::file_path(&e.id, &a.name)) { (a.name) }
                                span.at-faint { (human_size(a.size)) }
                                span.at-spacer {}
                                form method="post" action={"/entries/" (e.id) "/attachments/delete"} {
                                    (csrf_field(ctx.session.as_ref()))
                                    input type="hidden" name="name" value=(a.name);
                                    button.at-button.at-button--sm type="submit" { span data-icon="trash" {} "削除" }
                                }
                            }
                            div.kg-secret {
                                code.at-mono #{"att-md-" (i)} { (a.markdown) }
                                button.at-button.at-button--sm type="button" data-kg-copy={"att-md-" (i)} { span data-icon="copy" {} "コピー" }
                            }
                        }
                    }
                }
                p.at-small.at-muted { "コピーした Markdown を本文に貼ると、その位置に画像・音声・動画・PDF が表示されます。" }
            }
            form.kg-form.kg-upload method="post" action={"/entries/" (e.id) "/attachments"} enctype="multipart/form-data" {
                (csrf_field(ctx.session.as_ref()))
                label.kg-check {
                    input type="checkbox" name="replace" value="1";
                    span { "同じ名前のファイルがあれば置き換える" }
                }
                div.at-form-field {
                    label for="f-files" { "ファイルを選ぶ（複数可）" }
                    input #f-files type="file" name="file" multiple required;
                    span.at-form-field__help {
                        "1ファイル " (max_mb) " MB まで。ファイルは記事と同じ公開範囲になります。"
                        "本文の未保存の変更は失われるので、先に保存してください。"
                    }
                }
                div.kg-actions { button.at-button type="submit" { span data-icon="paperclip" {} "添付する" } }
            }
        }
    }
}

async fn edit_page_with(ctx: &Ctx, e: &Entry, f: &EntryForm, error: Option<&str>, att_error: Option<&str>) -> Response {
    let body = html! {
        h1.at-title { "編集: " (e.title) }
        p.at-muted { "本文を変えると新しい版として保存され、前の版も残ります。状態・タグ・公開範囲は版を作らずに変わります。添付ファイルは版に含まれず、すべての版で共有されます。" }
        (entry_form(ctx, &format!("/entries/{}/edit", e.id), f, Some(e), error))
        (attachments_editor(ctx, e, att_error))
    };
    ctx.page(
        "記事を編集",
        Current {
            kind: Some(e.kind),
            ..Default::default()
        },
        "",
        body,
    )
    .await
}

pub async fn edit_form(ctx: Ctx, Path(id): Path<String>) -> Response {
    let r = async {
        let e = editable(&ctx, &id).await?;
        Ok(edit_page(&ctx, &e, &form_from(&e), None).await)
    }
    .await;
    ctx.finish(r).await
}

pub async fn edit(ctx: Ctx, Path(id): Path<String>, Form(f): Form<EntryForm>) -> Response {
    let r = async {
        let e = editable(&ctx, &id).await?;
        match save_edit(&ctx, &e, &f).await {
            Err(err) if err.is_correctable() => {
                let mut resp = edit_page(&ctx, &e, &f, Some(&err.to_string())).await;
                *resp.status_mut() = err.status();
                Ok(resp)
            }
            other => other,
        }
    }
    .await;
    ctx.finish(r).await
}

async fn save_edit(ctx: &Ctx, e: &Entry, f: &EntryForm) -> Result<Response> {
    let auth = ctx.auth()?;
    ctx.check_form(f.csrf.as_deref())?;
    let title = f.title.clone().unwrap_or_default();
    let summary = non_empty(f.summary.clone());
    let body = f.body.clone().unwrap_or_default().replace("\r\n", "\n");
    let provenance = parse_json_object(f.provenance.clone(), "provenance")?.unwrap_or_else(|| serde_json::json!({}));
    let mut tags = normalize_tags(&split_tags(f.tags.as_deref().unwrap_or(""))).map_err(Error::BadRequest)?;
    tags.sort();
    let mut current_tags = e.tags.clone();
    current_tags.sort();
    let status: Option<Status> = parse_opt(f.status.clone())?;
    let visibility: Option<Visibility> = parse_opt(f.visibility.clone())?;

    let content_changed =
        title.trim() != e.title || summary != e.summary || body != e.body || provenance != e.provenance;
    let meta_changed =
        tags != current_tags || status.is_some_and(|s| s != e.status) || visibility.is_some_and(|v| v != e.visibility);
    if content_changed {
        let _ = entries::revise(
            State(ctx.state.clone()),
            auth.clone(),
            Path(e.id.clone()),
            Json(entries::ReviseRequest {
                title: Some(title),
                summary: Some(summary.unwrap_or_default()),
                body: Some(body),
                lang: None,
                provenance: Some(provenance),
            }),
        )
        .await?;
    }
    if meta_changed {
        let _ = entries::patch(
            State(ctx.state.clone()),
            auth,
            Path(e.id.clone()),
            Json(entries::PatchRequest {
                visibility,
                status,
                tags: Some(tags),
            }),
        )
        .await?;
    }
    Ok(redirect(&format!("/entries/{}", e.id)))
}

// ---------------------------------------------------------------- files

/// `GET /entries/{id}/files/{name}`: an attachment, for signed-in browsers,
/// guests and token holders alike.
pub async fn file(
    ctx: Ctx,
    viewer: std::result::Result<Viewer, Error>,
    Path((id, name)): Path<(String, String)>,
    Query(q): Query<attachments::DownloadQuery>,
) -> Response {
    let viewer = match viewer {
        Ok(Viewer(Some(p))) => Some(p),
        Ok(Viewer(None)) => ctx.viewer().cloned(),
        Err(e) => return e.into_response(),
    };
    match attachments::serve(
        &ctx.state,
        viewer.as_ref(),
        &id,
        &name,
        q.download.is_some(),
        &ctx.headers,
    )
    .await
    {
        Ok(resp) => resp,
        Err(Error::NotFound) if ctx.session.is_none() && wants_html(&ctx.headers) => {
            redirect(&format!("/login?next={}", urlencode(&ctx.here)))
        }
        Err(Error::NotFound) if wants_html(&ctx.headers) => ctx.error_page(Error::NotFound).await,
        Err(e) => e.into_response(),
    }
}

/// Uploads from the edit page. The form puts the CSRF token and the replace
/// checkbox before the files, so they are checked before anything is stored.
pub async fn upload_files(
    ctx: Ctx,
    Path(id): Path<String>,
    mp: std::result::Result<Multipart, MultipartRejection>,
) -> Response {
    let r = async {
        let e = editable(&ctx, &id).await?;
        let result = store_uploads(&ctx, &e, mp).await;
        match result {
            Err(err) if err.is_correctable() || matches!(err, Error::Conflict(_)) => {
                let e = editable(&ctx, &id).await?;
                let mut resp = edit_page_with(&ctx, &e, &form_from(&e), None, Some(&err.to_string())).await;
                *resp.status_mut() = err.status();
                Ok(resp)
            }
            other => other,
        }
    }
    .await;
    ctx.finish(r).await
}

async fn store_uploads(
    ctx: &Ctx,
    e: &Entry,
    mp: std::result::Result<Multipart, MultipartRejection>,
) -> Result<Response> {
    let auth = ctx.auth()?;
    let mut mp = mp.map_err(|r| Error::bad_request(format!("フォームを読めませんでした: {}", r.body_text())))?;
    let broken = |err: axum::extract::multipart::MultipartError| {
        Error::bad_request(format!("アップロードが途中で切れました: {err}"))
    };
    let mut checked = false;
    let mut replace = false;
    let mut stored = 0;
    while let Some(mut field) = mp.next_field().await.map_err(broken)? {
        match field.name() {
            Some("csrf") => {
                let csrf = field.text().await.map_err(broken)?;
                ctx.check_form(Some(&csrf))?;
                checked = true;
            }
            Some("replace") => replace = true,
            Some("file") => {
                if !checked {
                    ctx.check_form(None)?;
                    checked = true;
                }
                let Some(raw_name) = field.file_name().filter(|n| !n.is_empty()) else {
                    continue;
                };
                let name = crate::files::sanitize_name(raw_name);
                let declared = field.content_type().map(str::to_string);
                ctx.state.write_limiter.check_write(&auth.0)?;
                let mut writer = ctx.state.files.writer(ctx.state.config.max_attachment_bytes).await?;
                while let Some(chunk) = field.chunk().await.map_err(broken)? {
                    writer.write(&chunk).await.map_err(|err| match err {
                        Error::PayloadTooLarge(_) => Error::PayloadTooLarge(format!(
                            "{name} は大きすぎます（1ファイル {} MB まで）",
                            ctx.state.config.max_attachment_bytes.div_ceil(1024 * 1024)
                        )),
                        other => other,
                    })?;
                }
                let blob = writer.finish().await?;
                attachments::attach(&ctx.state, &auth.0, &e.id, &name, declared.as_deref(), blob, replace)
                    .await
                    .map_err(|err| match err {
                        Error::Conflict(_) => Error::bad_request(format!(
                            "「{name}」という名前のファイルがすでにあります。名前を変えるか、「同じ名前のファイルがあれば置き換える」にチェックを入れてください"
                        )),
                        other => other,
                    })?;
                stored += 1;
            }
            _ => {}
        }
    }
    if !checked {
        ctx.check_form(None)?;
    }
    if stored == 0 {
        return Err(Error::bad_request("添付するファイルを選んでください"));
    }
    Ok(redirect(&format!("/entries/{}/edit#attachments", e.id)))
}

#[derive(Deserialize, Default)]
pub struct DeleteFileForm {
    csrf: Option<String>,
    name: Option<String>,
}

pub async fn delete_file(ctx: Ctx, Path(id): Path<String>, Form(f): Form<DeleteFileForm>) -> Response {
    let r = async {
        let auth = ctx.auth()?;
        ctx.check_form(f.csrf.as_deref())?;
        attachments::delete(
            State(ctx.state.clone()),
            auth,
            Path((id.clone(), f.name.clone().unwrap_or_default())),
        )
        .await?;
        Ok(redirect(&format!("/entries/{id}/edit#attachments")))
    }
    .await;
    ctx.finish(r).await
}

// ---------------------------------------------------------------- signing in

#[derive(Deserialize, Default)]
pub struct LoginForm {
    username: Option<String>,
    password: Option<String>,
    next: Option<String>,
}

fn login_page(ctx: &Ctx, username: &str, next: &str, error: Option<&str>) -> Markup {
    html! {
        div.kg-narrow {
            h1.at-title { "ログイン" }
            form.kg-form method="post" action="/login" {
                @if let Some(err) = error { (notice_error(err)) }
                input type="hidden" name="next" value=(next);
                div.at-form-field {
                    label for="l-user" { "ユーザー名" }
                    span.at-field { input #l-user name="username" required autocomplete="username" value=(username) autofocus; }
                }
                div.at-form-field {
                    label for="l-pass" { "パスワード" }
                    span.at-field { input #l-pass type="password" name="password" required autocomplete="current-password"; }
                }
                div.kg-actions { button.at-button.at-button--primary type="submit" { "ログイン" } }
            }
            @if ctx.state.config.registration != Registration::Closed {
                p.at-muted.at-small { "アカウントがない場合は、管理者から招待リンクをもらって" a href="/register" { "登録" } "してください。" }
            }
            p.at-faint.at-small { "パスワードが未設定のアカウントは、サーバーで "
                code { "knowledge-garden set-password <ユーザー名>" } " を実行して設定できます。" }
        }
    }
}

#[derive(Deserialize, Default)]
pub struct NextQuery {
    next: Option<String>,
}

pub async fn login_form(ctx: Ctx, Query(q): Query<NextQuery>) -> Response {
    if ctx.session.is_some() {
        return redirect(safe_next(q.next.as_deref()));
    }
    let next = safe_next(q.next.as_deref()).to_string();
    ctx.page("ログイン", Current::default(), "", login_page(&ctx, "", &next, None))
        .await
}

pub async fn login(ctx: Ctx, Form(f): Form<LoginForm>) -> Response {
    let username = f.username.clone().unwrap_or_default().trim().to_lowercase();
    let next = safe_next(f.next.as_deref()).to_string();
    let r = async {
        ctx.check_form(None)?;
        ctx.state
            .login_limiter
            .hit_at(&username, std::time::Instant::now())
            .map_err(|_| {
                Error::TooManyRequests("ログインの試行が多すぎます。15 分ほど待ってからもう一度試してください".into())
            })?;
        let user = crate::auth::check_password(&ctx.state, &username, f.password.clone().unwrap_or_default())
            .await?
            .ok_or_else(|| Error::Unauthorized("ユーザー名かパスワードが違います".into()))?;
        start_session(&ctx, &user, &next).await
    }
    .await;
    match r {
        Ok(resp) => resp,
        Err(e @ (Error::Unauthorized(_) | Error::TooManyRequests(_) | Error::Forbidden(_))) => {
            let status = match e {
                Error::TooManyRequests(_) => StatusCode::TOO_MANY_REQUESTS,
                Error::Forbidden(_) => StatusCode::FORBIDDEN,
                _ => StatusCode::UNAUTHORIZED,
            };
            let mut resp = ctx
                .page(
                    "ログイン",
                    Current::default(),
                    "",
                    login_page(&ctx, &username, &next, Some(&e.to_string())),
                )
                .await;
            *resp.status_mut() = status;
            resp
        }
        Err(e) => ctx.error_page(e).await,
    }
}

async fn start_session(ctx: &Ctx, user: &User, next: &str) -> Result<Response> {
    let token = ctx.state.store.web_token(&user.id, &scopes_for_role(user.role)).await?;
    let secret = generate_secret("kgs_");
    ctx.state
        .store
        .create_session(NewSession {
            id_hash: hash_secret(&secret),
            user_id: user.id.clone(),
            token_id: token.id,
            csrf: generate_secret(""),
            expires_at: Utc::now() + Duration::days(super::SESSION_DAYS),
        })
        .await?;
    Ok(redirect_with_cookie(next, ctx.session_cookie(&secret)))
}

#[derive(Deserialize, Default)]
pub struct CsrfForm {
    csrf: Option<String>,
}

pub async fn logout(ctx: Ctx, Form(f): Form<CsrfForm>) -> Response {
    let r = async {
        ctx.check_form(f.csrf.as_deref())?;
        if let Some(v) = super::cookie_value(&ctx.headers, super::SESSION_COOKIE) {
            ctx.state.store.delete_session(&hash_secret(v)).await?;
        }
        Ok(redirect_with_cookie("/", ctx.clear_cookie()))
    }
    .await;
    ctx.finish(r).await
}

#[derive(Deserialize, Default)]
pub struct RegisterForm {
    invite: Option<String>,
    username: Option<String>,
    display_name: Option<String>,
    password: Option<String>,
    password2: Option<String>,
}

fn register_page(ctx: &Ctx, f: &RegisterForm, error: Option<&str>) -> Markup {
    let v = |s: &Option<String>| s.clone().unwrap_or_default();
    let need_invite = ctx.state.config.registration == Registration::Invite;
    html! {
        div.kg-narrow {
            h1.at-title { "登録" }
            @if ctx.state.config.registration == Registration::Closed {
                p.at-muted { "この Garden は新規登録を受け付けていません。" }
            } @else {
                form.kg-form method="post" action="/register" {
                    @if let Some(err) = error { (notice_error(err)) }
                    div.at-form-field {
                        label for="r-invite" { "招待コード" @if !need_invite { "（任意）" } }
                        span.at-field { input.at-mono #r-invite name="invite" required[need_invite] value=(v(&f.invite)) autocomplete="off"; }
                    }
                    div.at-form-field {
                        label for="r-user" { "ユーザー名" }
                        span.at-field { input #r-user name="username" required pattern="[a-z0-9_\\-]{2,32}" autocomplete="username" value=(v(&f.username)); }
                        span.at-form-field__help { "2〜32 文字の a-z、0-9、_、-。あとから変えられません。" }
                    }
                    div.at-form-field {
                        label for="r-name" { "表示名（任意）" }
                        span.at-field { input #r-name name="display_name" value=(v(&f.display_name)); }
                    }
                    div.at-form-field {
                        label for="r-pass" { "パスワード" }
                        span.at-field { input #r-pass type="password" name="password" required minlength="10" autocomplete="new-password"; }
                        span.at-form-field__help { "10 文字以上。" }
                    }
                    div.at-form-field {
                        label for="r-pass2" { "パスワード（確認）" }
                        span.at-field { input #r-pass2 type="password" name="password2" required minlength="10" autocomplete="new-password"; }
                    }
                    div.kg-actions { button.at-button.at-button--primary type="submit" { "登録してログイン" } }
                }
            }
        }
    }
}

pub async fn register_form(ctx: Ctx, Query(f): Query<RegisterForm>) -> Response {
    let f = RegisterForm {
        invite: f.invite,
        ..Default::default()
    };
    ctx.page("登録", Current::default(), "", register_page(&ctx, &f, None))
        .await
}

pub async fn register(ctx: Ctx, Form(f): Form<RegisterForm>) -> Response {
    let r = async {
        ctx.check_form(None)?;
        let username = f.username.clone().unwrap_or_default().trim().to_string();
        validate_username(&username)
            .map_err(|_| Error::bad_request("ユーザー名は 2〜32 文字の a-z、0-9、_、- にしてください"))?;
        let password = f.password.clone().unwrap_or_default();
        validate_password(&password).map_err(Error::BadRequest)?;
        if f.password2.as_deref() != Some(password.as_str()) {
            return Err(Error::bad_request("確認用のパスワードが一致しません"));
        }
        let display_name = non_empty(f.display_name.clone()).unwrap_or_else(|| username.clone());
        let hash = hash_password(password).await?;
        let user = match (ctx.state.config.registration, non_empty(f.invite.clone())) {
            (Registration::Closed, _) => return Err(Error::forbidden("この Garden は新規登録を受け付けていません")),
            (_, Some(code)) => ctx
                .state
                .store
                .redeem_invite(&hash_secret(&code), &username, &display_name)
                .await
                .map_err(|e| match e {
                    Error::Forbidden(_) => Error::forbidden("招待コードが無効か、使用済みか、期限切れです"),
                    Error::Conflict(_) => Error::bad_request("このユーザー名は使われています"),
                    other => other,
                })?,
            (Registration::Open, None) => ctx
                .state
                .store
                .create_user(NewUser {
                    username: username.clone(),
                    display_name,
                    role: Role::Member,
                })
                .await
                .map_err(|e| match e {
                    Error::Conflict(_) => Error::bad_request("このユーザー名は使われています"),
                    other => other,
                })?,
            (Registration::Invite, None) => return Err(Error::bad_request("招待コードを入力してください")),
        };
        ctx.state.store.set_password(&user.id, Some(&hash)).await?;
        start_session(&ctx, &user, "/").await
    }
    .await;
    match r {
        Err(Error::BadRequest(m) | Error::Forbidden(m)) => {
            let mut resp = ctx
                .page("登録", Current::default(), "", register_page(&ctx, &f, Some(&m)))
                .await;
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            resp
        }
        other => ctx.finish(other).await,
    }
}

// ---------------------------------------------------------------- settings

/// Something to show once at the top of the settings page.
enum Flash {
    Token { name: String, secret: String },
    Invite { code: String },
    Info(String),
    Error(String),
}

pub async fn settings(ctx: Ctx) -> Response {
    let r = settings_page(&ctx, None).await;
    ctx.finish(r).await
}

async fn settings_page(ctx: &Ctx, flash: Option<Flash>) -> Result<Response> {
    let auth = ctx.auth()?;
    let me = auth.0.user.clone();
    let session_token = auth.0.token.id.clone();
    let state = || State(ctx.state.clone());
    let Json(tokens) = accounts::list_tokens(state(), auth.clone()).await?;
    let invites = if auth.0.is_admin() {
        Some(accounts::list_invites(state(), auth.clone()).await?.0)
    } else {
        None
    };
    let base = &ctx.state.config.base_url;
    let body = html! {
        h1.at-title { "設定" }
        @match &flash {
            Some(Flash::Token { name, secret }) => {
                section.kg-flash {
                    h2.at-heading { "API キー「" (name) "」を発行しました" }
                    p.at-muted { "このキーはもう表示されません。いまコピーして、エージェントの設定に登録してください。" }
                    div.kg-secret { code.at-mono #new-secret { (secret) } button.at-button.at-button--sm type="button" data-kg-copy="new-secret" { span data-icon="copy" {} "コピー" } }
                    p.at-small.at-muted { "Claude Code に MCP サーバーとして登録する例:" }
                    div.at-markdown__code { pre { code { "claude mcp add --transport http knowledge-garden " (base) "/mcp \\\n  --header \"Authorization: Bearer " (secret) "\"" } } }
                }
            }
            Some(Flash::Invite { code }) => {
                section.kg-flash {
                    h2.at-heading { "招待リンクを作りました" }
                    p.at-muted { "このリンクはもう表示されません。招待する人に送ってください。一度だけ使えます。" }
                    div.kg-secret { code.at-mono #new-invite { (base) "/register?invite=" (code) } button.at-button.at-button--sm type="button" data-kg-copy="new-invite" { span data-icon="copy" {} "コピー" } }
                }
            }
            Some(Flash::Info(m)) => { (notice_info(m)) }
            Some(Flash::Error(m)) => { (notice_error(m)) }
            None => {}
        }

        section.kg-section {
            h2.at-heading { "アカウント" }
            p { (me.display_name) " " span.at-faint { "@" (me.username) " · " (if me.role == Role::Admin { "管理者" } else { "メンバー" }) } }
            details.kg-details {
                summary { "パスワードを変える" }
                form.kg-form method="post" action="/settings/password" {
                    (csrf_field(ctx.session.as_ref()))
                    div.at-form-field {
                        label for="p-cur" { "いまのパスワード" }
                        span.at-field { input #p-cur type="password" name="current" required autocomplete="current-password"; }
                    }
                    div.at-form-field {
                        label for="p-new" { "新しいパスワード" }
                        span.at-field { input #p-new type="password" name="password" required minlength="10" autocomplete="new-password"; }
                        span.at-form-field__help { "変えると、ほかのブラウザのログインはすべて解除されます。" }
                    }
                    div.kg-actions { button.at-button type="submit" { "パスワードを変える" } }
                }
            }
            form method="post" action="/logout" {
                (csrf_field(ctx.session.as_ref()))
                button.at-button.at-button--sm type="submit" { "ログアウト" }
            }
        }

        section.kg-section {
            h2.at-heading { "API キー" }
            p.at-muted { "エージェントごとに 1 つ発行してください。エージェント用のキーは 1 時間あたりの書き込み数が制限され、投稿には「エージェントが投稿」と表示されます。" }
            form.kg-form.kg-form--inline method="post" action="/settings/tokens" {
                (csrf_field(ctx.session.as_ref()))
                div.at-form-field {
                    label for="t-name" { "名前" }
                    span.at-field { input #t-name name="name" required maxlength="64" placeholder="claude-code"; }
                }
                fieldset.kg-scopes {
                    legend.at-small { "権限" }
                    label.at-checkbox { input type="checkbox" name="read" value="1" checked; span { "読む" } }
                    label.at-checkbox { input type="checkbox" name="entries_write" value="1" checked; span { "記事を書く" } }
                    label.at-checkbox { input type="checkbox" name="comments_write" value="1" checked; span { "コメントを書く" } }
                }
                div.kg-actions { button.at-button.at-button--primary type="submit" { "発行" } }
            }
            table.kg-table {
                thead { tr { th { "名前" } th { "種類" } th { "権限" } th { "最終使用" } th {} } }
                tbody {
                    @for t in &tokens {
                        tr.kg-revoked[t.revoked_at.is_some()] {
                            td { (t.name) @if t.id == session_token { span.at-badge { "このブラウザ" } } }
                            td { @if t.kind == TokenKind::Agent { span.at-badge.at-badge--agent { "エージェント" } } @else { "人間" } }
                            td.at-small { (t.scopes.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")) }
                            td.at-small { @if let Some(u) = &t.last_used_at { (time(u)) } @else { "—" } }
                            td {
                                @if t.revoked_at.is_some() {
                                    span.at-faint.at-small { "失効済み" }
                                } @else if t.id != session_token {
                                    form method="post" action={"/settings/tokens/" (t.id) "/revoke"} {
                                        (csrf_field(ctx.session.as_ref()))
                                        button.at-button.at-button--danger.at-button--sm type="submit" { "失効" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        @if let Some(invites) = &invites {
            section.kg-section {
                h2.at-heading { "招待" }
                p.at-muted { "招待リンクを受け取った人は、ユーザー名とパスワードを決めて登録できます。" }
                form.kg-form.kg-form--inline method="post" action="/settings/invites" {
                    (csrf_field(ctx.session.as_ref()))
                    div.at-form-field {
                        label for="i-note" { "メモ（任意）" }
                        span.at-field { input #i-note name="note" placeholder="誰あての招待か"; }
                    }
                    div.at-form-field {
                        label for="i-role" { "役割" }
                        select.at-select #i-role name="role" {
                            option value="member" selected { "メンバー" }
                            option value="admin" { "管理者" }
                        }
                    }
                    div.at-form-field {
                        label for="i-days" { "有効期限" }
                        select.at-select #i-days name="days" {
                            option value="1" { "1 日" }
                            option value="7" selected { "7 日" }
                            option value="30" { "30 日" }
                        }
                    }
                    div.kg-actions { button.at-button.at-button--primary type="submit" { "招待リンクを作る" } }
                }
                @if !invites.is_empty() {
                    table.kg-table {
                        thead { tr { th { "メモ" } th { "役割" } th { "作成" } th { "状態" } } }
                        tbody {
                            @for i in invites.iter().take(20) {
                                tr {
                                    td { (i.note.as_deref().unwrap_or("—")) }
                                    td { (if i.role == Role::Admin { "管理者" } else { "メンバー" }) }
                                    td.at-small { (time(&i.created_at)) }
                                    td.at-small {
                                        @if i.used_at.is_some() { "使用済み" }
                                        @else if i.expires_at.is_some_and(|t| t < Utc::now()) { "期限切れ" }
                                        @else { "未使用" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    let status = match flash {
        Some(Flash::Error(_)) => StatusCode::BAD_REQUEST,
        _ => StatusCode::OK,
    };
    let mut resp = ctx
        .page(
            "設定",
            Current {
                page: Some("settings"),
                ..Default::default()
            },
            "",
            body,
        )
        .await;
    *resp.status_mut() = status;
    Ok(resp)
}

/// Runs a settings action and shows the settings page with its outcome.
async fn settings_action(ctx: &Ctx, csrf: Option<&str>, action: impl Future<Output = Result<Flash>>) -> Response {
    let r = async {
        ctx.auth()?;
        ctx.check_form(csrf)?;
        let flash = match action.await {
            Ok(f) => f,
            Err(Error::BadRequest(m) | Error::Forbidden(m) | Error::Conflict(m)) => Flash::Error(m),
            Err(e) => return Err(e),
        };
        settings_page(ctx, Some(flash)).await
    }
    .await;
    ctx.finish(r).await
}

#[derive(Deserialize, Default)]
pub struct PasswordForm {
    csrf: Option<String>,
    current: Option<String>,
    password: Option<String>,
}

pub async fn change_password(ctx: Ctx, Form(f): Form<PasswordForm>) -> Response {
    let action = async {
        let user = ctx.auth()?.0.user;
        let current_hash = ctx
            .state
            .store
            .password_login(&user.username)
            .await?
            .map(|(_, h)| h)
            .unwrap_or_else(|| crate::auth::DUMMY_HASH.clone());
        if !verify_password(f.current.clone().unwrap_or_default(), current_hash).await {
            return Err(Error::bad_request("いまのパスワードが違います"));
        }
        let password = f.password.clone().unwrap_or_default();
        validate_password(&password).map_err(Error::BadRequest)?;
        let hash = hash_password(password).await?;
        ctx.state.store.set_password(&user.id, Some(&hash)).await?;
        // End every session, this one included.
        ctx.state.store.delete_user_sessions(&user.id).await?;
        Ok(Flash::Info(
            "パスワードを変えました。もう一度ログインしてください。".into(),
        ))
    };
    let resp = settings_action(&ctx, f.csrf.as_deref(), action).await;
    if resp.status() == StatusCode::OK {
        // Every session ended, so show the sign-in page.
        return redirect_with_cookie("/login", ctx.clear_cookie());
    }
    resp
}

#[derive(Deserialize, Default)]
pub struct TokenForm {
    csrf: Option<String>,
    name: Option<String>,
    read: Option<String>,
    entries_write: Option<String>,
    comments_write: Option<String>,
}

pub async fn create_token(ctx: Ctx, Form(f): Form<TokenForm>) -> Response {
    let action = async {
        let mut scopes = Vec::new();
        for (on, scope) in [
            (&f.read, Scope::Read),
            (&f.entries_write, Scope::EntriesWrite),
            (&f.comments_write, Scope::CommentsWrite),
        ] {
            if on.is_some() {
                scopes.push(scope);
            }
        }
        if scopes.is_empty() {
            return Err(Error::bad_request("権限を 1 つ以上選んでください"));
        }
        let (_, Json(v)) = accounts::create_token(
            State(ctx.state.clone()),
            ctx.auth()?,
            Json(accounts::CreateTokenRequest {
                name: f.name.clone().unwrap_or_default(),
                kind: TokenKind::Agent,
                scopes: Some(scopes),
            }),
        )
        .await?;
        Ok(Flash::Token {
            name: v["token"]["name"].as_str().unwrap_or_default().to_string(),
            secret: v["secret"].as_str().unwrap_or_default().to_string(),
        })
    };
    settings_action(&ctx, f.csrf.as_deref(), action).await
}

pub async fn revoke_token(ctx: Ctx, Path(id): Path<String>, Form(f): Form<CsrfForm>) -> Response {
    let action = async {
        let auth = ctx.auth()?;
        if auth.0.token.id == id {
            return Err(Error::bad_request(
                "このブラウザが使っているキーは、ログアウトで無効になります",
            ));
        }
        accounts::revoke_token(State(ctx.state.clone()), auth, Path(id)).await?;
        Ok(Flash::Info("API キーを失効させました。".into()))
    };
    settings_action(&ctx, f.csrf.as_deref(), action).await
}

#[derive(Deserialize, Default)]
pub struct InviteForm {
    csrf: Option<String>,
    note: Option<String>,
    role: Option<String>,
    days: Option<i64>,
}

pub async fn create_invite(ctx: Ctx, Form(f): Form<InviteForm>) -> Response {
    let action = async {
        let (_, Json(v)) = accounts::create_invite(
            State(ctx.state.clone()),
            ctx.auth()?,
            Json(accounts::CreateInviteRequest {
                role: parse_opt(f.role.clone())?.unwrap_or(Role::Member),
                note: non_empty(f.note.clone()),
                expires_in_days: Some(f.days.unwrap_or(7)),
            }),
        )
        .await?;
        Ok(Flash::Invite {
            code: v["code"].as_str().unwrap_or_default().to_string(),
        })
    };
    settings_action(&ctx, f.csrf.as_deref(), action).await
}
