mod accounts;
mod comments;
mod entries;
mod search;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post};

use crate::AppState;

/// The REST API (`/health` and `/api/v1`).
pub fn rest_router(state: AppState) -> Router {
    let api = Router::new()
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
        .route("/entries/{id}/links", get(entries::links).post(entries::add_link))
        .route("/entries/{id}/comments", get(comments::list).post(comments::create))
        .route("/tags", get(entries::tags))
        .route("/search", get(search::search));

    Router::new()
        .route("/health", get(|| async { "ok" }))
        .nest("/api/v1", api)
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .with_state(state)
}

/// The full application: the REST API plus the MCP endpoint at `/mcp`,
/// which forwards tool calls to the REST API.
pub fn router(state: AppState) -> Router {
    let rest = rest_router(state.clone());
    rest.clone().merge(crate::mcp::router(state, rest))
}
