//! Token handling and the request extractors that identify the caller.

use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use sha2::{Digest, Sha256};

use crate::AppState;
use crate::error::Error;
use crate::model::{Principal, Scope};

pub const TOKEN_PREFIX: &str = "kg_";
pub const INVITE_PREFIX: &str = "kginv_";

/// Generates a random secret such as `kg_3f9a...` (256 bits).
pub fn generate_secret(prefix: &str) -> String {
    let bytes: [u8; 32] = rand::random();
    format!("{prefix}{}", hex::encode(bytes))
}

/// Secrets are stored only as SHA-256 hashes. They are high-entropy random
/// values, so a fast hash is sufficient (no password stretching needed).
pub fn hash_secret(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

/// The caller, if a valid bearer token was given. An invalid token is an error,
/// not a silent fallback to guest access.
pub struct Viewer(pub Option<Principal>);

/// A caller that must be authenticated.
pub struct Auth(pub Principal);

impl Auth {
    pub fn require(&self, scope: Scope) -> Result<(), Error> {
        if self.0.has(scope) {
            Ok(())
        } else {
            Err(Error::forbidden(format!("token lacks the '{scope}' scope")))
        }
    }
}

impl FromRequestParts<AppState> for Viewer {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Error> {
        let Some(header) = parts.headers.get(AUTHORIZATION) else {
            return Ok(Viewer(None));
        };
        let value = header
            .to_str()
            .map_err(|_| Error::Unauthorized("invalid Authorization header".into()))?;
        let secret = value
            .strip_prefix("Bearer ")
            .map(str::trim)
            .ok_or_else(|| Error::Unauthorized("expected 'Authorization: Bearer <token>'".into()))?;
        let principal = state
            .store
            .authenticate(&hash_secret(secret))
            .await?
            .ok_or_else(|| Error::Unauthorized("invalid or revoked token".into()))?;
        Ok(Viewer(Some(principal)))
    }
}

impl FromRequestParts<AppState> for Auth {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Error> {
        match Viewer::from_request_parts(parts, state).await?.0 {
            Some(p) => Ok(Auth(p)),
            None => Err(Error::Unauthorized("this endpoint requires a token".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_random_and_hash_is_stable() {
        let a = generate_secret(TOKEN_PREFIX);
        let b = generate_secret(TOKEN_PREFIX);
        assert_ne!(a, b);
        assert!(a.starts_with("kg_") && a.len() == 3 + 64);
        assert_eq!(hash_secret(&a), hash_secret(&a));
        assert_ne!(hash_secret(&a), hash_secret(&b));
    }
}
