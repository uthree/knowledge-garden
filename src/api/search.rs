use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};

use super::entries::ListQuery;
use crate::AppState;
use crate::auth::Viewer;
use crate::error::{Error, Result};
use crate::model::{EntryKind, EntrySummary, Page, ReproResult, Status};
use crate::policy::VisibilityFilter;
use crate::search::{Highlighter, compile_regex, parse_query};
use crate::store::{RegexField, SearchQuery};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchParams {
    /// Terms (all must match), `"phrases"` and `-exclusions`.
    pub q: Option<String>,
    /// A regular expression (Rust `regex` syntax; prefix with `(?i)` for case-insensitive).
    pub regex: Option<String>,
    #[serde(default)]
    pub regex_field: RegexField,
    // The list filters are repeated here because `#[serde(flatten)]` breaks
    // numeric fields in query strings.
    pub kind: Option<EntryKind>,
    pub status: Option<Status>,
    pub author: Option<String>,
    pub tag: Option<String>,
    pub any_tag: Option<String>,
    pub has_reproduction: Option<ReproResult>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Serialize)]
pub struct SearchHit {
    pub entry: EntrySummary,
    pub score: Option<f64>,
    /// An excerpt of the body around the first match.
    pub snippet: Option<String>,
    /// Which fields matched: title, summary, body, tags.
    pub matched_in: Vec<&'static str>,
}

pub async fn search(
    State(state): State<AppState>,
    Viewer(viewer): Viewer,
    Query(p): Query<SearchParams>,
) -> Result<Json<Page<SearchHit>>> {
    let parsed = parse_query(p.q.as_deref().unwrap_or("")).map_err(Error::BadRequest)?;
    let regex_src = p.regex.filter(|r| !r.is_empty());
    let regex = regex_src
        .as_deref()
        .map(compile_regex)
        .transpose()
        .map_err(Error::BadRequest)?;
    if parsed.is_empty() && regex.is_none() {
        return Err(Error::bad_request(
            "give q or regex (use /api/v1/entries to list without a query)",
        ));
    }

    let query = SearchQuery {
        indexed_terms: parsed.indexed_terms(),
        short_terms: parsed.short_terms(),
        exclude: parsed.exclude.clone(),
        regex: regex_src,
        regex_field: p.regex_field,
        filter: ListQuery {
            kind: p.kind,
            status: p.status,
            author: p.author,
            tag: p.tag,
            any_tag: p.any_tag,
            has_reproduction: p.has_reproduction,
            since: p.since,
            until: p.until,
            limit: p.limit,
            offset: p.offset,
        }
        .into_filter()?,
    };
    let vis = VisibilityFilter::for_viewer(&state.config, viewer.as_ref());
    let page = state.store.search(&query, &vis).await?;

    let hl = Highlighter::new(&parsed.include, regex);
    let items = page
        .items
        .into_iter()
        .map(|row| {
            let tags = row.entry.tags.join(" ");
            let fields: [(&'static str, &str); 4] = [
                ("title", &row.entry.title),
                ("summary", row.entry.summary.as_deref().unwrap_or("")),
                ("body", &row.body),
                ("tags", &tags),
            ];
            let matched_in = fields
                .iter()
                .filter(|(_, text)| hl.matches(text))
                .map(|(name, _)| *name)
                .collect();
            SearchHit {
                snippet: hl.snippet(&row.body),
                matched_in,
                score: row.score,
                entry: row.entry,
            }
        })
        .collect();
    Ok(Json(Page {
        items,
        total: page.total,
        limit: page.limit,
        offset: page.offset,
    }))
}
