use axum::Json;
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::AppState;
use crate::auth::Auth;
use crate::error::{Error, Result};
use crate::federation::{
    FEED_PAGE_SIZE, FEED_PATH, FeedPage, GardenKey, SyncReport, build_feed, discover, normalize_base_url, sync_peer,
    verify_request,
};
use crate::store::{Peer, PeerStatus};

/// `GET /.well-known/knowledge-garden`: who this Garden is, for other Gardens.
pub async fn well_known(State(state): State<AppState>) -> Result<Json<Value>> {
    let key = GardenKey::load(&state).await?;
    Ok(Json(json!({
        "name": state.config.name,
        "base_url": state.config.base_url,
        "public_key": key.public_key(),
        "software": { "name": "knowledge-garden", "version": env!("CARGO_PKG_VERSION") },
        "federation": { "feed": FEED_PATH, "signature": "ed25519" },
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedQuery {
    pub after: Option<String>,
    pub limit: Option<i64>,
}

/// `GET /federation/v1/feed`: changes to shared entries, for approved peers only.
pub async fn feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Query(q): Query<FeedQuery>,
) -> Result<Json<FeedPage>> {
    let path_and_query = uri.path_and_query().map(|p| p.as_str()).unwrap_or(FEED_PATH);
    verify_request(&state, &headers, "GET", path_and_query).await?;
    let key = GardenKey::load(&state).await?;
    let limit = q.limit.unwrap_or(FEED_PAGE_SIZE).clamp(1, 500);
    Ok(Json(build_feed(&state, &key, q.after.as_deref(), limit).await?))
}

fn require_admin(auth: &Auth) -> Result<()> {
    if auth.0.is_admin() {
        Ok(())
    } else {
        Err(Error::forbidden("only admins can manage peer Gardens"))
    }
}

pub async fn list_peers(State(state): State<AppState>, auth: Auth) -> Result<Json<Vec<Peer>>> {
    require_admin(&auth)?;
    Ok(Json(state.store.list_peers().await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddPeerRequest {
    pub base_url: String,
}

/// Approves another Garden: we will serve it our feed and mirror its feed.
/// Its public key is fetched and pinned now.
pub async fn add_peer(
    State(state): State<AppState>,
    auth: Auth,
    Json(req): Json<AddPeerRequest>,
) -> Result<(StatusCode, Json<Peer>)> {
    require_admin(&auth)?;
    let base_url = normalize_base_url(&req.base_url)?;
    if base_url == state.config.base_url {
        return Err(Error::bad_request("a Garden cannot peer with itself"));
    }
    let (name, public_key) = discover(&state, &base_url).await?;
    let peer = state.store.add_peer(&base_url, &name, &public_key).await?;
    Ok((StatusCode::CREATED, Json(peer)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdatePeerRequest {
    pub status: PeerStatus,
}

pub async fn update_peer(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
    Json(req): Json<UpdatePeerRequest>,
) -> Result<Json<Peer>> {
    require_admin(&auth)?;
    if !state.store.set_peer_status(&id, req.status).await? {
        return Err(Error::NotFound);
    }
    Ok(Json(state.store.get_peer(&id).await?.ok_or(Error::NotFound)?))
}

/// Pulls new changes from a peer now instead of waiting for the next scheduled sync.
pub async fn sync_now(State(state): State<AppState>, auth: Auth, Path(id): Path<String>) -> Result<Json<SyncReport>> {
    require_admin(&auth)?;
    let peer = state.store.get_peer(&id).await?.ok_or(Error::NotFound)?;
    Ok(Json(sync_peer(&state, &peer).await?))
}
