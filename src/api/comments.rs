use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::Value;

use super::entries::readable_entry;
use crate::AppState;
use crate::auth::{Auth, Viewer};
use crate::error::{Error, Result};
use crate::model::*;
use crate::store::NewComment;

const MAX_COMMENT_BYTES: usize = 100_000;

pub async fn list(
    State(state): State<AppState>,
    Viewer(viewer): Viewer,
    Path(id): Path<String>,
) -> Result<Json<Vec<Comment>>> {
    readable_entry(&state, viewer.as_ref(), &id).await?;
    Ok(Json(state.store.list_comments(&id).await?))
}

#[derive(Deserialize)]
pub struct CreateCommentRequest {
    #[serde(default = "comment_kind")]
    pub kind: CommentKind,
    pub body: String,
    pub parent_id: Option<String>,
    /// Required for reproduction reports.
    pub result: Option<ReproResult>,
    /// The environment the reproduction was tried in (JSON object).
    pub environment: Option<Value>,
    /// The entry version that was tried. Defaults to the current version for reproduction reports.
    pub entry_version: Option<i64>,
}

fn comment_kind() -> CommentKind {
    CommentKind::Comment
}

pub async fn create(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
    Json(req): Json<CreateCommentRequest>,
) -> Result<(StatusCode, Json<Comment>)> {
    auth.require(Scope::CommentsWrite)?;
    let entry = readable_entry(&state, Some(&auth.0), &id).await?;

    if req.body.trim().is_empty() {
        return Err(Error::bad_request("body must not be empty"));
    }
    if req.body.len() > MAX_COMMENT_BYTES {
        return Err(Error::bad_request("comment is too large"));
    }
    let entry_version = match (req.kind, req.entry_version) {
        (_, Some(v)) if v < 1 || v > entry.current_version => {
            return Err(Error::bad_request(format!("entry has no version {v}")));
        }
        (CommentKind::Reproduction, None) => Some(entry.current_version),
        (_, v) => v,
    };
    match req.kind {
        CommentKind::Reproduction if req.result.is_none() => {
            return Err(Error::bad_request(
                "reproduction reports need a result: reproduced, partial or failed",
            ));
        }
        CommentKind::Comment if req.result.is_some() || req.environment.is_some() => {
            return Err(Error::bad_request(
                "result and environment are only for kind 'reproduction'",
            ));
        }
        _ => {}
    }
    if req.environment.as_ref().is_some_and(|e| !e.is_object()) {
        return Err(Error::bad_request("environment must be a JSON object"));
    }
    if let Some(parent_id) = &req.parent_id {
        let parent = state.store.get_comment(parent_id).await?;
        match parent {
            Some(p) if p.entry_id == entry.id && p.parent_id.is_none() => {}
            Some(p) if p.entry_id == entry.id => {
                return Err(Error::bad_request("replies can only be one level deep"));
            }
            _ => return Err(Error::bad_request("parent comment not found on this entry")),
        }
    }

    state.write_limiter.check_write(&auth.0)?;
    let comment = state
        .store
        .add_comment(NewComment {
            entry_id: entry.id,
            parent_id: req.parent_id,
            author_id: auth.0.user.id.clone(),
            token_id: auth.0.token.id.clone(),
            kind: req.kind,
            body: req.body,
            result: req.result,
            environment: req.environment,
            entry_version,
        })
        .await?;
    Ok((StatusCode::CREATED, Json(comment)))
}
