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

pub const MIN_PASSWORD_CHARS: usize = 10;
const MAX_PASSWORD_CHARS: usize = 256;

pub fn validate_password(password: &str) -> Result<(), String> {
    let n = password.chars().count();
    if n < MIN_PASSWORD_CHARS {
        return Err(format!("パスワードは {MIN_PASSWORD_CHARS} 文字以上にしてください"));
    }
    if n > MAX_PASSWORD_CHARS {
        return Err(format!("パスワードは {MAX_PASSWORD_CHARS} 文字以下にしてください"));
    }
    Ok(())
}

/// Hashes a password with Argon2id (on a blocking thread: it is deliberately slow).
pub async fn hash_password(password: String) -> Result<String, Error> {
    use argon2::password_hash::PasswordHasher;
    tokio::task::spawn_blocking(move || {
        argon2::Argon2::default()
            .hash_password(password.as_bytes())
            .map(|h| h.to_string())
            .map_err(Error::internal)
    })
    .await
    .map_err(Error::internal)?
}

/// Checks a password against an Argon2 PHC string.
pub async fn verify_password(password: String, hash: String) -> bool {
    use argon2::password_hash::PasswordVerifier;
    use argon2::password_hash::phc::PasswordHash;
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash).is_ok_and(|h| {
            argon2::Argon2::default()
                .verify_password(password.as_bytes(), &h)
                .is_ok()
        })
    })
    .await
    .unwrap_or(false)
}

/// A hash of a random password, checked when the username is unknown so that
/// the response time does not tell which usernames exist.
pub(crate) static DUMMY_HASH: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    use argon2::password_hash::PasswordHasher;
    argon2::Argon2::default()
        .hash_password(generate_secret("").as_bytes())
        .map(|h| h.to_string())
        .unwrap_or_default()
});

/// Checks a username and password. Unknown users and users without a password
/// take as long as a wrong password, and both give `None`.
pub async fn check_password(
    state: &AppState,
    username: &str,
    password: String,
) -> Result<Option<crate::model::User>, Error> {
    let found = state.store.password_login(username).await?;
    let (user, hash) = match found {
        Some((u, h)) => (Some(u), h),
        None => (None, DUMMY_HASH.clone()),
    };
    let ok = verify_password(password, hash).await;
    Ok(user.filter(|_| ok))
}

/// The caller, if a valid bearer token was given. An invalid token is an error,
/// not a silent fallback to guest access.
pub struct Viewer(pub Option<Principal>);

/// A caller that must be authenticated.
#[derive(Clone)]
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

    #[tokio::test]
    async fn passwords_round_trip() {
        let h = hash_password("correct horse battery".into()).await.unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery".into(), h.clone()).await);
        assert!(!verify_password("wrong password!".into(), h.clone()).await);
        assert!(!verify_password("x".into(), "not a hash".into()).await);
        assert!(validate_password("short").is_err());
    }
}
