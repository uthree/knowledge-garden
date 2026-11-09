//! The web UI for people: reading, searching and posting in a browser.
//!
//! Pages are rendered on the server. Every read and write goes through the
//! same handlers as the REST API (called directly, with the signed-in user as
//! the caller), so permissions and validation are identical.
//!
//! People sign in with a password; the browser keeps an HttpOnly session
//! cookie. Writes made in the browser act through the user's `web` token.
//! Forms carry a per-session CSRF token and must come from the same origin.

mod markdown;
mod pages;
mod view;

use axum::Router;
use axum::extract::{FromRequestParts, Path};
use axum::http::header::{
    ACCEPT, CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, COOKIE, HOST, LOCATION, ORIGIN, REFERRER_POLICY,
    SET_COOKIE, VARY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::map_response;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use maud::{Markup, html};

use crate::AppState;
use crate::auth::{Auth, hash_secret};
use crate::error::{Error, Result};
use crate::model::Principal;
use crate::policy::VisibilityFilter;
use crate::store::Session;

pub use view::Current;

pub const SESSION_COOKIE: &str = "kg_session";
const SESSION_DAYS: i64 = 30;

/// Routes for browsers. `/` and `/entries/{id}` answer JSON to clients that
/// do not ask for HTML, as they did before the UI existed.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(pages::home))
        .route("/entries/{id}", get(pages::entry))
        .route("/entries/{id}/edit", get(pages::edit_form).post(pages::edit))
        .route("/entries/{id}/comments", post(pages::comment))
        .route("/new", get(pages::new_form).post(pages::create))
        .route("/login", get(pages::login_form).post(pages::login))
        .route("/logout", post(pages::logout))
        .route("/register", get(pages::register_form).post(pages::register))
        .route("/settings", get(pages::settings))
        .route("/settings/password", post(pages::change_password))
        .route("/settings/tokens", post(pages::create_token))
        .route("/settings/tokens/{id}/revoke", post(pages::revoke_token))
        .route("/settings/invites", post(pages::create_invite))
        .route("/assets/{*path}", get(asset))
        .layer(map_response(security_headers))
        .with_state(state)
}

async fn security_headers(mut resp: Response) -> Response {
    let is_html = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/html"));
    let h = resp.headers_mut();
    h.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    if is_html {
        h.insert(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: https:; \
                 object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
            ),
        );
        h.insert(X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
        h.insert(REFERRER_POLICY, HeaderValue::from_static("same-origin"));
        h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    resp
}

const ASSETS: &[(&str, &str, &str)] = &[
    ("garden.css", "text/css", include_str!("../../assets/garden.css")),
    ("garden.js", "text/javascript", include_str!("../../assets/garden.js")),
    ("theme.js", "text/javascript", include_str!("../../assets/theme.js")),
    (
        "atelier/atelier.css",
        "text/css",
        include_str!("../../assets/atelier/atelier.css"),
    ),
    (
        "atelier/atelier.js",
        "text/javascript",
        include_str!("../../assets/atelier/atelier.js"),
    ),
    (
        "atelier/generated/tokens.css",
        "text/css",
        include_str!("../../assets/atelier/generated/tokens.css"),
    ),
    (
        "atelier/generated/icons.js",
        "text/javascript",
        include_str!("../../assets/atelier/generated/icons.js"),
    ),
];

async fn asset(Path(path): Path<String>) -> Response {
    match ASSETS.iter().find(|(p, _, _)| *p == path) {
        Some((_, ty, body)) => (
            [
                (CONTENT_TYPE, HeaderValue::from_static(ty)),
                (CACHE_CONTROL, HeaderValue::from_static("public, max-age=3600")),
            ],
            *body,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "no such asset").into_response(),
    }
}

/// Whether the client asked for a web page rather than JSON.
pub fn wants_html(headers: &HeaderMap) -> bool {
    headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("text/html"))
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// The request as the pages see it: who is signed in, and enough of the
/// request to check forms and build links.
pub struct Ctx {
    pub state: AppState,
    pub session: Option<Session>,
    pub headers: HeaderMap,
    /// Path and query of this request, for returning here after signing in.
    pub here: String,
}

impl FromRequestParts<AppState> for Ctx {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> std::result::Result<Self, Self::Rejection> {
        let session = match cookie_value(&parts.headers, SESSION_COOKIE) {
            Some(v) if !v.is_empty() => state.store.session(&hash_secret(v)).await.unwrap_or_else(|e| {
                tracing::warn!("session lookup failed: {e}");
                None
            }),
            _ => None,
        };
        Ok(Ctx {
            state: state.clone(),
            session,
            headers: parts.headers.clone(),
            here: parts
                .uri
                .path_and_query()
                .map_or_else(|| "/".to_string(), |p| p.as_str().to_string()),
        })
    }
}

impl Ctx {
    pub fn viewer(&self) -> Option<&Principal> {
        self.session.as_ref().map(|s| &s.principal)
    }

    /// The signed-in user as an API caller, or `Unauthorized` (which sends the browser to sign in).
    pub fn auth(&self) -> Result<Auth> {
        self.viewer()
            .cloned()
            .map(Auth)
            .ok_or_else(|| Error::Unauthorized("ログインしてください".into()))
    }

    pub fn vis(&self) -> VisibilityFilter {
        VisibilityFilter::for_viewer(&self.state.config, self.viewer())
    }

    /// Rejects form posts from other sites: the Origin (when sent) must match
    /// the Host, and signed-in users' forms must carry their CSRF token.
    pub fn check_form(&self, csrf: Option<&str>) -> Result<()> {
        if let Some(origin) = self.headers.get(ORIGIN).and_then(|v| v.to_str().ok()) {
            let origin_host = url::Url::parse(origin).ok().map(|u| view::host_of(u.as_str()));
            let host = self.headers.get(HOST).and_then(|v| v.to_str().ok());
            if origin_host.as_deref() != host {
                return Err(Error::forbidden("別のサイトから送信されたフォームは受け付けません"));
            }
        }
        if let Some(s) = &self.session
            && csrf.is_none_or(|c| !constant_time_eq(c.as_bytes(), s.csrf.as_bytes()))
        {
            return Err(Error::forbidden(
                "フォームの有効期限が切れました。ページを読み込み直してから、もう一度送信してください",
            ));
        }
        Ok(())
    }

    pub fn secure_cookies(&self) -> bool {
        self.state.config.base_url.starts_with("https://")
    }

    pub fn session_cookie(&self, value: &str) -> HeaderValue {
        let secure = if self.secure_cookies() { "; Secure" } else { "" };
        HeaderValue::from_str(&format!(
            "{SESSION_COOKIE}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{secure}",
            SESSION_DAYS * 24 * 3600
        ))
        .expect("valid cookie")
    }

    pub fn clear_cookie(&self) -> HeaderValue {
        HeaderValue::from_static("kg_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
    }

    /// Wraps `body` in the page chrome.
    pub async fn page(&self, title: &str, current: Current, query: &str, body: Markup) -> Response {
        let tags = self.state.store.tag_counts(&self.vis(), 30).await.unwrap_or_default();
        let chrome = view::Chrome {
            garden: &self.state.config.name,
            session: self.session.as_ref(),
            query,
            tags: &tags,
            current: &current,
        };
        html_response(StatusCode::OK, view::layout(&chrome, title, body))
    }

    /// Turns an error into a page: sign-in for `Unauthorized`, otherwise an error page.
    pub async fn finish(&self, r: Result<Response>) -> Response {
        match r {
            Ok(resp) => resp,
            Err(Error::Unauthorized(_)) if self.session.is_none() => {
                redirect(&format!("/login?next={}", view::urlencode(&self.here)))
            }
            Err(e) => self.error_page(e).await,
        }
    }

    pub async fn error_page(&self, e: Error) -> Response {
        let (status, title, message) = match &e {
            Error::NotFound => (
                StatusCode::NOT_FOUND,
                "見つかりません",
                "このページは存在しないか、閲覧する権限がありません。".to_string(),
            ),
            Error::BadRequest(m) => (StatusCode::BAD_REQUEST, "入力を確認してください", m.clone()),
            Error::Unauthorized(m) => (StatusCode::UNAUTHORIZED, "ログインが必要です", m.clone()),
            Error::Forbidden(m) => (StatusCode::FORBIDDEN, "この操作はできません", m.clone()),
            Error::Conflict(m) => (StatusCode::CONFLICT, "競合しています", m.clone()),
            Error::TooManyRequests(m) => (StatusCode::TOO_MANY_REQUESTS, "しばらく待ってください", m.clone()),
            Error::Internal(detail) => {
                tracing::error!("internal error: {detail}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "サーバーでエラーが起きました",
                    "時間をおいてもう一度試してください。続く場合は管理者に連絡してください。".to_string(),
                )
            }
        };
        let body = html! {
            div.kg-empty {
                h1.at-title { (title) }
                p.at-muted { (message) }
                a.at-button href="/" { "記事の一覧へ" }
            }
        };
        let mut resp = self.page(title, Current::default(), "", body).await;
        *resp.status_mut() = status;
        resp
    }
}

pub fn html_response(status: StatusCode, markup: Markup) -> Response {
    (
        status,
        [
            (CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8")),
            (VARY, HeaderValue::from_static("Accept, Cookie")),
        ],
        markup.into_string(),
    )
        .into_response()
}

/// A 303 redirect, so the browser follows a form post with a GET.
pub fn redirect(to: &str) -> Response {
    (
        StatusCode::SEE_OTHER,
        [(
            LOCATION,
            HeaderValue::from_str(to).unwrap_or_else(|_| HeaderValue::from_static("/")),
        )],
    )
        .into_response()
}

pub fn redirect_with_cookie(to: &str, cookie: HeaderValue) -> Response {
    let mut resp = redirect(to);
    resp.headers_mut().append(SET_COOKIE, cookie);
    resp
}

/// Only paths on this site, so a crafted `next` cannot send people elsewhere.
pub fn safe_next(next: Option<&str>) -> &str {
    match next {
        Some(n) if n.starts_with('/') && !n.starts_with("//") && !n.starts_with("/\\") => n,
        _ => "/",
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A 404 page for browsers that reach a path no route serves.
pub fn not_found_page(state: &AppState) -> Response {
    let body = html! {
        div.kg-empty {
            h1.at-title { "見つかりません" }
            p.at-muted { "このページは存在しません。" }
            a.at-button href="/" { (state.config.name) " の記事一覧へ" }
        }
    };
    html_response(StatusCode::NOT_FOUND, view::bare("見つかりません", body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_must_stay_on_site() {
        assert_eq!(safe_next(Some("/entries/1")), "/entries/1");
        assert_eq!(safe_next(Some("//evil.example")), "/");
        assert_eq!(safe_next(Some("/\\evil.example")), "/");
        assert_eq!(safe_next(Some("https://evil.example")), "/");
        assert_eq!(safe_next(None), "/");
    }

    #[test]
    fn reads_cookies() {
        let mut h = HeaderMap::new();
        h.insert(COOKIE, HeaderValue::from_static("a=1; kg_session=abc; b=2"));
        assert_eq!(cookie_value(&h, "kg_session"), Some("abc"));
        assert_eq!(cookie_value(&h, "c"), None);
    }
}
