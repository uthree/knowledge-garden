//! The web UI: content negotiation, signing in, forms and their protections.

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use common::Garden;
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;

struct Resp {
    status: StatusCode,
    location: Option<String>,
    cookie: Option<String>,
    body: String,
}

/// A browser: sends Accept: text/html and keeps the session cookie.
struct Browser<'a> {
    garden: &'a Garden,
    cookie: Option<String>,
}

impl<'a> Browser<'a> {
    fn new(garden: &'a Garden) -> Self {
        Self { garden, cookie: None }
    }

    async fn send(&mut self, method: Method, uri: &str, form: Option<&[(&str, &str)]>, origin: Option<&str>) -> Resp {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::ACCEPT, "text/html,application/xhtml+xml")
            .header(header::HOST, "garden.test");
        if let Some(c) = &self.cookie {
            req = req.header(header::COOKIE, c);
        }
        if let Some(o) = origin {
            req = req.header(header::ORIGIN, o);
        }
        let req = match form {
            Some(f) => {
                let body = url::form_urlencoded::Serializer::new(String::new())
                    .extend_pairs(f)
                    .finish();
                req.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(body))
            }
            None => req.body(Body::empty()),
        }
        .unwrap();
        let resp = self.garden.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let location = resp
            .headers()
            .get(header::LOCATION)
            .map(|v| v.to_str().unwrap().to_string());
        let cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string());
        if let Some(c) = &cookie {
            self.cookie = c.strip_suffix('=').is_none().then(|| c.clone());
        }
        let body = String::from_utf8_lossy(&resp.into_body().collect().await.unwrap().to_bytes()).into_owned();
        Resp {
            status,
            location,
            cookie,
            body,
        }
    }

    async fn get(&mut self, uri: &str) -> Resp {
        self.send(Method::GET, uri, None, None).await
    }

    async fn post(&mut self, uri: &str, form: &[(&str, &str)]) -> Resp {
        self.send(Method::POST, uri, Some(form), Some("http://garden.test"))
            .await
    }

    /// The CSRF token embedded in the settings page.
    async fn csrf(&mut self) -> String {
        let page = self.get("/settings").await;
        assert_eq!(page.status, StatusCode::OK, "{}", page.body);
        let marker = "name=\"csrf\" value=\"";
        let start = page.body.find(marker).expect("csrf field") + marker.len();
        page.body[start..start + 64].to_string()
    }
}

async fn invite_code(g: &Garden) -> String {
    let (s, inv) = g.post("/api/v1/invites", Some(&g.admin), json!({})).await;
    assert_eq!(s, StatusCode::CREATED);
    inv["code"].as_str().unwrap().to_string()
}

/// Registers `username` through the web form and returns the signed-in browser.
async fn signed_in<'a>(g: &'a Garden, username: &str) -> Browser<'a> {
    let code = invite_code(g).await;
    let mut b = Browser::new(g);
    let r = b
        .post(
            "/register",
            &[
                ("invite", &code),
                ("username", username),
                ("password", "a long password"),
                ("password2", "a long password"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert!(b.cookie.is_some());
    b
}

#[tokio::test]
async fn pages_and_json_share_urls() {
    let g = Garden::new().await;
    let e = g
        .entry(
            &g.admin,
            json!({ "kind": "memo", "title": "Hello <b>", "body": "**bold** <script>alert(1)</script>" }),
        )
        .await;
    let id = e["entry"]["id"].as_str().unwrap();

    // Agents and peers keep getting JSON.
    let (s, index) = g.get("/", None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(index["endpoints"].is_array());
    let (s, entry) = g.get(&format!("/entries/{id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(entry["title"], "Hello <b>");
    let (s, err) = g.get("/entries/nope", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(err["error"]["code"], "not_found");

    // Browsers get pages.
    let mut b = Browser::new(&g);
    let home = b.get("/").await;
    assert_eq!(home.status, StatusCode::OK);
    assert!(home.body.contains("Hello &lt;b&gt;"), "titles are escaped");
    let page = b.get(&format!("/entries/{id}")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("<strong>bold</strong>"));
    assert!(!page.body.contains("<script>alert"), "scripts in bodies are removed");
    let missing = b.get("/entries/nope").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert!(missing.body.contains("見つかりません"));
    let nowhere = b.get("/no/such/page").await;
    assert_eq!(nowhere.status, StatusCode::NOT_FOUND);
    assert!(nowhere.body.starts_with("<!DOCTYPE html>"));

    // Search through the page.
    let found = b.get("/?q=bold").await;
    assert!(found.body.contains(&format!("/entries/{id}")));
    let css = b.get("/assets/atelier/atelier.css").await;
    assert_eq!(css.status, StatusCode::OK);
    assert!(
        home.body.contains("/assets/app-icon.svg"),
        "the logo is the favicon and in the header"
    );
    for path in [
        "/assets/app-icon.svg",
        "/assets/app-icon.ico",
        "/assets/app-icon.png",
        "/favicon.ico",
    ] {
        assert_eq!(b.get(path).await.status, StatusCode::OK, "{path}");
    }
}

#[tokio::test]
async fn sign_in_post_and_sign_out() {
    let g = Garden::new().await;
    let mut b = Browser::new(&g);

    // Writing needs a session.
    let r = b.get("/new").await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.location.as_deref(), Some("/login?next=%2Fnew"));

    let mut b = signed_in(&g, "alice").await;
    let csrf = b.csrf().await;
    let r = b
        .post(
            "/new",
            &[
                ("csrf", &csrf),
                ("kind", "research_note"),
                ("title", "From the browser"),
                ("body", "## 目的\n\ntext"),
                ("tags", "web, UI"),
                ("status", ""),
                ("visibility", "public"),
                ("provenance", "{\"os\": \"linux\"}"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let path = r.location.unwrap();
    let (_, e) = g.get(&path, None).await;
    assert_eq!(e["author"]["username"], "alice");
    assert_eq!(e["posted_via"], json!({ "name": "web", "kind": "human" }));
    assert_eq!(e["tags"], json!(["ui", "web"]));
    assert_eq!(e["provenance"], json!({ "os": "linux" }));

    // Editing the body makes a version; changing only the status does not.
    let r = b
        .post(
            &format!("{path}/edit"),
            &[
                ("csrf", &csrf),
                ("title", "From the browser"),
                ("body", "## 目的\n\nrevised"),
                ("tags", "web, ui"),
                ("status", "verified"),
                ("visibility", "public"),
                ("provenance", "{\"os\": \"linux\"}"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let (_, e) = g.get(&path, None).await;
    assert_eq!(e["version"], 2);
    assert_eq!(e["status"], "verified");

    // A reproduction report with an environment.
    let r = b
        .post(
            &format!("{path}/comments"),
            &[
                ("csrf", &csrf),
                ("kind", "reproduction"),
                ("result", "partial"),
                ("environment", "{\"gpu\": \"none\"}"),
                ("body", "mostly"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let (_, comments) = g.get(&format!("/api/v1{path}/comments"), None).await;
    assert_eq!(comments[0]["result"], "partial");
    assert_eq!(comments[0]["entry_version"], 2);

    // A bad form shows the error and keeps what was typed.
    let r = b
        .post(
            &format!("{path}/comments"),
            &[("csrf", &csrf), ("kind", "reproduction"), ("body", "typed text")],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.body.contains("typed text"));

    // Signing out ends the session.
    let cookie = b.cookie.clone();
    let r = b.post("/logout", &[("csrf", &csrf)]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert!(r.cookie.unwrap().ends_with('='));
    b.cookie = cookie;
    assert_eq!(b.get("/settings").await.status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn forms_are_protected() {
    let g = Garden::new().await;
    let mut b = signed_in(&g, "bob").await;
    let csrf = b.csrf().await;
    let form = [("kind", "memo"), ("title", "t"), ("body", "b")];

    // No CSRF token.
    let r = b.post("/new", &form).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // Right token, but posted from another site.
    let mut with_csrf = form.to_vec();
    with_csrf.push(("csrf", &csrf));
    let r = b
        .send(Method::POST, "/new", Some(&with_csrf), Some("https://evil.example"))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // Both right.
    let r = b.post("/new", &with_csrf).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);

    // Someone else's entry cannot be edited.
    let e = g
        .entry(&g.admin, json!({ "kind": "memo", "title": "admin's", "body": "x" }))
        .await;
    let id = e["entry"]["id"].as_str().unwrap();
    assert_eq!(
        b.get(&format!("/entries/{id}/edit")).await.status,
        StatusCode::FORBIDDEN
    );
    let r = b
        .post(
            &format!("/entries/{id}/edit"),
            &[("csrf", &csrf), ("title", "taken"), ("body", "x")],
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let (_, after) = g.get(&format!("/api/v1/entries/{id}"), None).await;
    assert_eq!(after["title"], "admin's");
}

#[tokio::test]
async fn login_and_passwords() {
    let g = Garden::new().await;
    let mut b = signed_in(&g, "carol").await;
    let csrf = b.csrf().await;

    let mut fresh = Browser::new(&g);
    let bad = fresh
        .post("/login", &[("username", "carol"), ("password", "wrong password")])
        .await;
    assert_eq!(bad.status, StatusCode::UNAUTHORIZED);
    let unknown = fresh
        .post("/login", &[("username", "nobody"), ("password", "a long password")])
        .await;
    assert_eq!(unknown.status, StatusCode::UNAUTHORIZED);
    let ok = fresh
        .post(
            "/login",
            &[
                ("username", "carol"),
                ("password", "a long password"),
                ("next", "//evil.example"),
            ],
        )
        .await;
    assert_eq!(ok.status, StatusCode::SEE_OTHER);
    assert_eq!(ok.location.as_deref(), Some("/"), "next cannot leave the site");

    // Changing the password ends every session.
    let r = b
        .post(
            "/settings/password",
            &[
                ("csrf", &csrf),
                ("current", "a long password"),
                ("password", "another long one"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert_eq!(fresh.get("/settings").await.status, StatusCode::SEE_OTHER);
    let ok = fresh
        .post("/login", &[("username", "carol"), ("password", "another long one")])
        .await;
    assert_eq!(ok.status, StatusCode::SEE_OTHER);

    // Repeated failures are slowed down.
    let mut last = StatusCode::OK;
    for _ in 0..12 {
        last = Browser::new(&g)
            .post("/login", &[("username", "dave"), ("password", "guess guess")])
            .await
            .status;
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn settings_issue_agent_tokens_and_invites() {
    let g = Garden::new().await;
    let mut b = signed_in(&g, "erin").await;
    let csrf = b.csrf().await;

    let r = b
        .post(
            "/settings/tokens",
            &[
                ("csrf", &csrf),
                ("name", "claude-code"),
                ("read", "1"),
                ("comments_write", "1"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let start = r.body.find("kg_").unwrap();
    let secret = &r.body[start..start + 67];
    let (s, me) = g.get("/api/v1/me", Some(secret)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(me["token"]["kind"], "agent");
    assert_eq!(me["token"]["scopes"], json!(["comments:write", "read"]));

    // Members cannot invite; the page does not offer it and the action is refused.
    assert!(!r.body.contains("/settings/invites"));
    let r = b.post("/settings/invites", &[("csrf", &csrf)]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // Revoking from the page.
    let (_, tokens) = g.get("/api/v1/tokens", Some(secret)).await;
    let id = tokens
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "claude-code")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = b
        .post(&format!("/settings/tokens/{id}/revoke"), &[("csrf", &csrf)])
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(g.get("/api/v1/me", Some(secret)).await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn admins_invite_from_settings() {
    let g = Garden::new().await;
    // Promote a browser user to admin through an admin invite.
    let (_, inv) = g
        .post("/api/v1/invites", Some(&g.admin), json!({ "role": "admin" }))
        .await;
    let mut b = Browser::new(&g);
    let code = inv["code"].as_str().unwrap();
    let r = b
        .post(
            "/register",
            &[
                ("invite", code),
                ("username", "root2"),
                ("password", "a long password"),
                ("password2", "a long password"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let csrf = b.csrf().await;
    let r = b
        .post(
            "/settings/invites",
            &[
                ("csrf", &csrf),
                ("note", "for frank"),
                ("role", "member"),
                ("days", "7"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let marker = "/register?invite=";
    let start = r.body.find(marker).unwrap() + marker.len();
    let new_code = &r.body[start..start + 70];
    assert!(new_code.starts_with("kginv_"));

    // The used invite cannot be reused.
    let r = Browser::new(&g)
        .post(
            "/register",
            &[
                ("invite", code),
                ("username", "mallory"),
                ("password", "a long password"),
                ("password2", "a long password"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}
