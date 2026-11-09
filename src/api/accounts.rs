use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::AppState;
use crate::auth::{Auth, INVITE_PREFIX, TOKEN_PREFIX, generate_secret, hash_secret};
use crate::config::Registration;
use crate::error::{Error, Result};
use crate::model::*;
use crate::store::{NewInvite, NewToken, NewUser};

pub async fn garden_info(State(state): State<AppState>) -> Json<Value> {
    let c = &state.config;
    Json(json!({
        "name": c.name,
        "base_url": c.base_url,
        "guest_read": c.guest_read,
        "registration": c.registration,
        "default_visibility": c.default_visibility,
        "software": { "name": "knowledge-garden", "version": env!("CARGO_PKG_VERSION") },
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterRequest {
    pub invite_code: Option<String>,
    pub username: String,
    pub display_name: Option<String>,
}

fn scopes_for(role: Role) -> Vec<Scope> {
    let mut scopes = DEFAULT_SCOPES.to_vec();
    if role == Role::Admin {
        scopes.push(Scope::Admin);
    }
    scopes
}

/// Creates a user and returns their first (human) token.
pub async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    validate_username(&req.username).map_err(Error::BadRequest)?;
    let display_name = req.display_name.unwrap_or_else(|| req.username.clone());
    let user = match (state.config.registration, req.invite_code) {
        (Registration::Closed, _) => return Err(Error::forbidden("registration is closed")),
        (_, Some(code)) => {
            state
                .store
                .redeem_invite(&hash_secret(&code), &req.username, &display_name)
                .await?
        }
        (Registration::Open, None) => {
            state
                .store
                .create_user(NewUser {
                    username: req.username,
                    display_name,
                    role: Role::Member,
                })
                .await?
        }
        (Registration::Invite, None) => return Err(Error::forbidden("an invite code is required")),
    };
    let secret = generate_secret(TOKEN_PREFIX);
    let token = state
        .store
        .create_token(NewToken {
            user_id: user.id.clone(),
            name: "default".into(),
            kind: TokenKind::Human,
            scopes: scopes_for(user.role),
            token_hash: hash_secret(&secret),
        })
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "user": user, "token": token, "secret": secret })),
    ))
}

pub async fn me(Auth(p): Auth) -> Json<Value> {
    Json(json!({ "user": p.user, "token": p.token }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateTokenRequest {
    pub name: String,
    #[serde(default = "agent_kind")]
    pub kind: TokenKind,
    pub scopes: Option<Vec<Scope>>,
}

fn agent_kind() -> TokenKind {
    TokenKind::Agent
}

/// Issues a token for the caller, typically one per agent (e.g. `claude-code`).
/// Only human tokens can issue tokens, so an agent cannot widen its own access.
pub async fn create_token(
    State(state): State<AppState>,
    Auth(p): Auth,
    Json(req): Json<CreateTokenRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    if p.token.kind != TokenKind::Human {
        return Err(Error::forbidden("only human tokens can create tokens"));
    }
    let name = req.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(Error::bad_request("token name must be 1-64 characters"));
    }
    let mut scopes = req.scopes.unwrap_or_else(|| DEFAULT_SCOPES.to_vec());
    scopes.sort_by_key(|s| s.as_str());
    scopes.dedup();
    if scopes.contains(&Scope::Admin) && p.user.role != Role::Admin {
        return Err(Error::forbidden("only admins can create admin tokens"));
    }
    let secret = generate_secret(TOKEN_PREFIX);
    let token = state
        .store
        .create_token(NewToken {
            user_id: p.user.id,
            name,
            kind: req.kind,
            scopes,
            token_hash: hash_secret(&secret),
        })
        .await?;
    Ok((StatusCode::CREATED, Json(json!({ "token": token, "secret": secret }))))
}

pub async fn list_tokens(State(state): State<AppState>, Auth(p): Auth) -> Result<Json<Vec<Token>>> {
    Ok(Json(state.store.list_tokens(&p.user.id).await?))
}

pub async fn revoke_token(State(state): State<AppState>, Auth(p): Auth, Path(id): Path<String>) -> Result<StatusCode> {
    if p.token.kind != TokenKind::Human && p.token.id != id {
        return Err(Error::forbidden("agent tokens can only revoke themselves"));
    }
    if state.store.revoke_token(&p.user.id, &id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(Error::NotFound)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateInviteRequest {
    #[serde(default = "member_role")]
    pub role: Role,
    pub note: Option<String>,
    /// Days until the invite expires (default 7, 0 = never).
    pub expires_in_days: Option<i64>,
}

fn member_role() -> Role {
    Role::Member
}

pub async fn create_invite(
    State(state): State<AppState>,
    auth: Auth,
    Json(req): Json<CreateInviteRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    if !auth.0.is_admin() {
        return Err(Error::forbidden("only admins can create invites"));
    }
    let (invite, code) = issue_invite(&state, &auth.0.user.id, req.role, req.note, req.expires_in_days).await?;
    Ok((StatusCode::CREATED, Json(json!({ "invite": invite, "code": code }))))
}

pub async fn issue_invite(
    state: &AppState,
    created_by: &str,
    role: Role,
    note: Option<String>,
    expires_in_days: Option<i64>,
) -> Result<(Invite, String)> {
    let days = expires_in_days.unwrap_or(7);
    if !(0..=365).contains(&days) {
        return Err(Error::bad_request("expires_in_days must be between 0 and 365"));
    }
    let code = generate_secret(INVITE_PREFIX);
    let invite = state
        .store
        .create_invite(NewInvite {
            code_hash: hash_secret(&code),
            role,
            created_by: created_by.to_string(),
            note,
            expires_at: (days > 0).then(|| Utc::now() + Duration::days(days)),
        })
        .await?;
    Ok((invite, code))
}

pub async fn list_invites(State(state): State<AppState>, auth: Auth) -> Result<Json<Vec<Invite>>> {
    if !auth.0.is_admin() {
        return Err(Error::forbidden("only admins can list invites"));
    }
    Ok(Json(state.store.list_invites().await?))
}
