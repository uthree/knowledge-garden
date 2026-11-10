//! Running a Garden: the pieces of `knowledge-garden serve` and `create-admin`
//! that other programs (the desktop app's local server) reuse.

use std::future::Future;
use std::time::Duration;

use crate::auth::{TOKEN_PREFIX, generate_secret, hash_secret};
use crate::error::Result;
use crate::model::{Role, TokenKind, User, scopes_for_role, validate_username};
use crate::store::{NewToken, NewUser, Store};
use crate::{AppState, router};

/// Serves the full application on `listener` until `shutdown` completes, then
/// finishes the requests in flight. With `sync_interval`, it also pulls from
/// approved peer Gardens in the background while serving.
pub async fn serve(
    state: AppState,
    listener: tokio::net::TcpListener,
    sync_interval: Option<Duration>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let sync = sync_interval
        .filter(|d| !d.is_zero())
        .map(|interval| tokio::spawn(crate::federation::run_sync_loop(state.clone(), interval)));
    let result = axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown)
        .await;
    if let Some(sync) = sync {
        sync.abort();
    }
    result
}

/// Creates an admin user and their first human token (named `token_name`), and
/// returns the user with the token's secret, which is not stored anywhere.
pub async fn bootstrap_admin(
    store: &dyn Store,
    username: &str,
    display_name: &str,
    token_name: &str,
) -> Result<(User, String)> {
    validate_username(username).map_err(crate::error::Error::BadRequest)?;
    let user = store
        .create_user(NewUser {
            username: username.to_string(),
            display_name: display_name.to_string(),
            role: Role::Admin,
        })
        .await?;
    let secret = generate_secret(TOKEN_PREFIX);
    store
        .create_token(NewToken {
            user_id: user.id.clone(),
            name: token_name.to_string(),
            kind: TokenKind::Human,
            scopes: scopes_for_role(Role::Admin),
            token_hash: hash_secret(&secret),
        })
        .await?;
    Ok((user, secret))
}
