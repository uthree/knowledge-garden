mod accounts;
mod comments;
mod entries;
mod federation;
pub(crate) mod index;
mod search;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware::map_response;
use axum::routing::{delete, get, post};

use crate::AppState;

/// The REST API (`/health` and `/api/v1`).
pub fn rest_router(state: AppState) -> Router {
    let api = Router::new()
        .route("/", get(index::api_index))
        .route("/garden", get(accounts::garden_info))
        .route("/kinds", get(entries::kinds))
        .route("/register", post(accounts::register))
        .route("/me", get(accounts::me))
        .route("/tokens", get(accounts::list_tokens).post(accounts::create_token))
        .route("/tokens/{id}", delete(accounts::revoke_token))
        .route("/invites", get(accounts::list_invites).post(accounts::create_invite))
        .route("/entries", get(entries::list).post(entries::create))
        .route("/entries/{id}", get(entries::get).patch(entries::patch))
        .route("/entries/{id}/versions", get(entries::versions).post(entries::revise))
        .route("/entries/{id}/versions/{version}", get(entries::get_version))
        .route("/entries/{id}/links", get(entries::links).post(entries::add_link))
        .route("/entries/{id}/comments", get(comments::list).post(comments::create))
        .route("/tags", get(entries::tags))
        .route("/search", get(search::search))
        .route("/peers", get(federation::list_peers).post(federation::add_peer))
        .route("/peers/{id}", axum::routing::patch(federation::update_peer))
        .route("/peers/{id}/sync", post(federation::sync_now));

    Router::new()
        .route("/", get(index::api_index))
        .route("/health", get(|| async { "ok" }))
        .route(crate::federation::WELL_KNOWN_PATH, get(federation::well_known))
        .route(crate::federation::FEED_PATH, get(federation::feed))
        // Entry URIs (`<base_url>/entries/<id>`) resolve to the entry itself.
        .route("/entries/{id}", get(entries::get))
        .nest("/api/v1", api)
        .fallback(index::no_route)
        .layer(map_response(index::json_errors))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .with_state(state)
}

/// The full application: the REST API plus the MCP endpoint at `/mcp`,
/// which forwards tool calls to the REST API.
pub fn router(state: AppState) -> Router {
    let rest = rest_router(state.clone());
    rest.clone().merge(crate::mcp::router(state, rest))
}
