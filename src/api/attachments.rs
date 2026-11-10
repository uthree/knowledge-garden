//! Files attached to entries: upload, list, download and delete.
//!
//! A file is readable by exactly the people who can read its entry, and can be
//! added or removed by the people who can edit the entry. Downloads support
//! `Range` (for seeking in audio and video) and `ETag` revalidation.

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header::{
    ACCEPT_RANGES, CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_SECURITY_POLICY,
    CONTENT_TYPE, ETAG, IF_NONE_MATCH, RANGE, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use super::entries::{editable_entry, readable_entry};
use crate::AppState;
use crate::auth::{Auth, Viewer};
use crate::error::{Error, Result};
use crate::files::{Preview, StoredBlob, content_type_for, validate_name};
use crate::model::{Attachment, Principal};
use crate::store::NewAttachment;

/// Attachments of a readable entry, with their URLs filled in.
pub(crate) async fn attachments_of(state: &AppState, entry_id: &str) -> Result<Vec<Attachment>> {
    Ok(state
        .store
        .list_attachments(entry_id)
        .await?
        .into_iter()
        .map(|a| a.locate(&state.config, entry_id))
        .collect())
}

/// Deletes blobs that no attachment uses any more.
pub(crate) async fn forget_blobs(state: &AppState, shas: impl IntoIterator<Item = String>) {
    for sha in shas {
        match state.store.blob_in_use(&sha).await {
            Ok(false) => state.files.remove(&sha).await,
            Ok(true) => {}
            Err(e) => tracing::warn!("could not check whether blob {sha} is in use: {e}"),
        }
    }
}

/// Records an already-stored blob as a file of an entry the caller may edit.
pub(crate) async fn attach(
    state: &AppState,
    by: &Principal,
    entry_id: &str,
    name: &str,
    declared_type: Option<&str>,
    blob: StoredBlob,
    replace: bool,
) -> Result<Attachment> {
    let result = state
        .store
        .add_attachment(
            NewAttachment {
                entry_id: entry_id.to_string(),
                name: name.to_string(),
                content_type: content_type_for(name, declared_type).to_string(),
                size: blob.size as i64,
                sha256: blob.sha256.clone(),
                token_id: by.token.id.clone(),
            },
            replace,
        )
        .await;
    match result {
        Ok((a, replaced)) => {
            forget_blobs(state, replaced).await;
            Ok(a.locate(&state.config, entry_id))
        }
        Err(e) => {
            forget_blobs(state, [blob.sha256]).await;
            Err(e)
        }
    }
}

pub async fn list(
    State(state): State<AppState>,
    Viewer(viewer): Viewer,
    Path(id): Path<String>,
) -> Result<Json<Vec<Attachment>>> {
    readable_entry(&state, viewer.as_ref(), &id).await?;
    Ok(Json(attachments_of(&state, &id).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UploadQuery {
    /// File name, e.g. `loss-curve.png`. Referenced from Markdown as `attachment:<name>`.
    pub name: String,
    /// Replace an existing file with the same name instead of failing.
    #[serde(default)]
    pub replace: bool,
}

/// `POST /api/v1/entries/{id}/attachments?name=plot.png` with the raw file as the body.
pub async fn upload(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
    Query(q): Query<UploadQuery>,
    headers: HeaderMap,
    body: Body,
) -> Result<(StatusCode, Json<Attachment>)> {
    editable_entry(&state, &auth, &id).await?;
    let name = validate_name(&q.name).map_err(Error::BadRequest)?;
    let declared = headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok());
    if let Some(len) = headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        && len > state.config.max_attachment_bytes
    {
        return Err(Error::PayloadTooLarge(format!(
            "the file is larger than this Garden's limit of {} MB",
            state.config.max_attachment_bytes.div_ceil(1024 * 1024)
        )));
    }
    state.write_limiter.check_write(&auth.0)?;

    let mut writer = state.files.writer(state.config.max_attachment_bytes).await?;
    let mut body = body;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| Error::bad_request(format!("upload was interrupted: {e}")))?;
        if let Ok(data) = frame.into_data() {
            writer.write(&data).await?;
        }
    }
    let blob = writer.finish().await?;
    let a = attach(&state, &auth.0, &id, &name, declared, blob, q.replace).await?;
    Ok((StatusCode::CREATED, Json(a)))
}

pub async fn delete(
    State(state): State<AppState>,
    auth: Auth,
    Path((id, name)): Path<(String, String)>,
) -> Result<StatusCode> {
    editable_entry(&state, &auth, &id).await?;
    state.write_limiter.check_write(&auth.0)?;
    let sha = state
        .store
        .delete_attachment(&id, &name)
        .await?
        .ok_or(Error::NotFound)?;
    forget_blobs(&state, [sha]).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, Default)]
pub struct DownloadQuery {
    /// Any value: always download instead of showing the file in the browser.
    pub download: Option<String>,
}

/// `GET /api/v1/entries/{id}/attachments/{name}`: the file itself.
pub async fn download(
    State(state): State<AppState>,
    Viewer(viewer): Viewer,
    Path((id, name)): Path<(String, String)>,
    Query(q): Query<DownloadQuery>,
    headers: HeaderMap,
) -> Result<Response> {
    serve(&state, viewer.as_ref(), &id, &name, q.download.is_some(), &headers).await
}

/// Sends an attachment to someone who may read its entry.
pub(crate) async fn serve(
    state: &AppState,
    viewer: Option<&Principal>,
    entry_id: &str,
    name: &str,
    force_download: bool,
    req: &HeaderMap,
) -> Result<Response> {
    readable_entry(state, viewer, entry_id).await?;
    let a = state
        .store
        .get_attachment(entry_id, name)
        .await?
        .ok_or(Error::NotFound)?;
    let Some((mut file, len)) = state.files.open(&a.sha256).await? else {
        tracing::error!("blob {} of {entry_id}/{name} is missing", a.sha256);
        return Err(Error::NotFound);
    };

    let etag = format!("\"{}\"", a.sha256);
    let mut h = HeaderMap::new();
    h.insert(ETAG, HeaderValue::from_str(&etag).expect("hex etag"));
    // Visibility can change at any time, so caches must check with us before reuse.
    h.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    h.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    let preview = Preview::of(&a.content_type);
    let content_type = if preview == Preview::Text {
        format!("{}; charset=utf-8", a.content_type)
    } else {
        a.content_type.clone()
    };
    h.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&content_type).unwrap_or(HeaderValue::from_static(crate::files::OCTET_STREAM)),
    );
    let inline = !force_download && preview != Preview::None;
    h.insert(CONTENT_DISPOSITION, content_disposition(inline, &a.name));
    // Files are other people's content: they may be framed by our pages (PDF previews)
    // but must not run scripts (an SVG could carry one). Browsers' PDF viewers do not
    // work in a sandbox, and PDFs are not scriptable in them anyway.
    h.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(if preview == Preview::Pdf {
            "frame-ancestors 'self'"
        } else {
            "default-src 'none'; img-src 'self' data:; media-src 'self'; style-src 'unsafe-inline'; sandbox; frame-ancestors 'self'"
        }),
    );

    if req
        .get(IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag || t.trim() == "*"))
    {
        return Ok((StatusCode::NOT_MODIFIED, h).into_response());
    }

    let range = req
        .get(RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|r| parse_range(r, len));
    let (status, start, count) = match range {
        None => (StatusCode::OK, 0, len),
        Some(Err(())) => {
            h.insert(
                CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{len}")).expect("valid header"),
            );
            return Ok((StatusCode::RANGE_NOT_SATISFIABLE, h).into_response());
        }
        Some(Ok((start, end))) => {
            h.insert(
                CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes {start}-{end}/{len}")).expect("valid header"),
            );
            (StatusCode::PARTIAL_CONTENT, start, end - start + 1)
        }
    };
    h.insert(CONTENT_LENGTH, HeaderValue::from(count));
    if start > 0 {
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(Error::internal)?;
    }
    let body = Body::from_stream(tokio_util::io::ReaderStream::new(file.take(count)));
    Ok((status, h, body).into_response())
}

fn content_disposition(inline: bool, name: &str) -> HeaderValue {
    let fallback: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '"' && c != '\\' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let kind = if inline { "inline" } else { "attachment" };
    HeaderValue::from_str(&format!(
        "{kind}; filename=\"{fallback}\"; filename*=UTF-8''{}",
        crate::files::encode_path_segment(name)
    ))
    .unwrap_or(HeaderValue::from_static("attachment"))
}

/// Parses a single `bytes=` range. `None` means "send the whole file" (no range,
/// or one we do not support such as several ranges); `Err` means unsatisfiable.
fn parse_range(header: &str, len: u64) -> Option<std::result::Result<(u64, u64), ()>> {
    let spec = header.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (a, b) = (a.trim(), b.trim());
    let range = if a.is_empty() {
        // The last `b` bytes.
        let n: u64 = b.parse().ok()?;
        if n == 0 || len == 0 {
            return Some(Err(()));
        }
        (len.saturating_sub(n), len - 1)
    } else {
        let start: u64 = a.parse().ok()?;
        let end = if b.is_empty() {
            len.saturating_sub(1)
        } else {
            b.parse::<u64>().ok()?.min(len.saturating_sub(1))
        };
        if start >= len || end < start {
            return Some(Err(()));
        }
        (start, end)
    };
    Some(Ok(range))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-1", 10), Some(Ok((0, 1))));
        assert_eq!(parse_range("bytes=5-", 10), Some(Ok((5, 9))));
        assert_eq!(parse_range("bytes=-3", 10), Some(Ok((7, 9))));
        assert_eq!(parse_range("bytes=8-100", 10), Some(Ok((8, 9))));
        assert_eq!(parse_range("bytes=10-", 10), Some(Err(())));
        assert_eq!(parse_range("bytes=0-1,4-5", 10), None);
        assert_eq!(parse_range("items=0-1", 10), None);
    }

    #[test]
    fn disposition_keeps_unicode_names() {
        let v = content_disposition(true, "損失.png");
        assert_eq!(
            v.to_str().unwrap(),
            "inline; filename=\"__.png\"; filename*=UTF-8''%E6%90%8D%E5%A4%B1.png"
        );
    }
}
