use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use knowledge_garden::auth::{TOKEN_PREFIX, generate_secret, hash_secret};
use knowledge_garden::config::Config;
use knowledge_garden::model::{DEFAULT_SCOPES, Role, Scope, TokenKind};
use knowledge_garden::store::sqlite::SqliteStore;
use knowledge_garden::store::{AccountStore, NewToken, NewUser};
use knowledge_garden::{AppState, router};
use serde_json::{Value, json};
use tower::ServiceExt;

const BASE: &str = "http://garden.test";

struct Garden {
    app: Router,
    admin: String,
}

impl Garden {
    async fn new() -> Self {
        Self::with_config(Config::new(BASE, "test")).await
    }

    async fn with_config(config: Config) -> Self {
        let store = SqliteStore::connect("sqlite::memory:").await.unwrap();
        let user = store
            .create_user(NewUser {
                username: "admin".into(),
                display_name: "Admin".into(),
                role: Role::Admin,
            })
            .await
            .unwrap();
        let secret = generate_secret(TOKEN_PREFIX);
        let mut scopes = DEFAULT_SCOPES.to_vec();
        scopes.push(Scope::Admin);
        store
            .create_token(NewToken {
                user_id: user.id,
                name: "default".into(),
                kind: TokenKind::Human,
                scopes,
                token_hash: hash_secret(&secret),
            })
            .await
            .unwrap();
        Self {
            app: router(AppState::new(store, config)),
            admin: secret,
        }
    }

    async fn call(&self, method: Method, uri: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value)
    }

    async fn get(&self, uri: &str, token: Option<&str>) -> (StatusCode, Value) {
        self.call(Method::GET, uri, token, None).await
    }

    async fn post(&self, uri: &str, token: Option<&str>, body: Value) -> (StatusCode, Value) {
        self.call(Method::POST, uri, token, Some(body)).await
    }

    /// Invites and registers a member; returns their human token.
    async fn member(&self, username: &str) -> String {
        let (s, inv) = self.post("/api/v1/invites", Some(&self.admin), json!({})).await;
        assert_eq!(s, StatusCode::CREATED, "{inv}");
        let (s, reg) = self
            .post(
                "/api/v1/register",
                None,
                json!({ "invite_code": inv["code"], "username": username }),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED, "{reg}");
        reg["secret"].as_str().unwrap().to_string()
    }

    async fn agent_token(&self, human: &str, name: &str) -> String {
        let (s, t) = self.post("/api/v1/tokens", Some(human), json!({ "name": name })).await;
        assert_eq!(s, StatusCode::CREATED, "{t}");
        t["secret"].as_str().unwrap().to_string()
    }

    async fn entry(&self, token: &str, body: Value) -> Value {
        let (s, e) = self.post("/api/v1/entries", Some(token), body).await;
        assert_eq!(s, StatusCode::CREATED, "{e}");
        e
    }
}

fn memo(title: &str, visibility: &str) -> Value {
    json!({ "kind": "memo", "title": title, "body": "body", "visibility": visibility })
}

#[tokio::test]
async fn invite_register_and_agent_post_flow() {
    let g = Garden::new().await;
    let human = g.member("uthree").await;
    let agent = g.agent_token(&human, "claude-code").await;

    let created = g
        .entry(
            &agent,
            json!({
                "kind": "troubleshooting",
                "title": "sqlx 0.9 で QueryBuilder の型引数が変わった",
                "body": "# 症状\nコンパイルエラー\n# 原因\nライフタイム引数の削除\n# 解決策\n`QueryBuilder<Sqlite>` と書く",
                "tags": ["Rust", "sqlx"],
                "provenance": { "model": "test-model" }
            }),
        )
        .await;
    let entry = &created["entry"];
    assert_eq!(entry["status"], "preliminary");
    assert_eq!(entry["visibility"], "public");
    assert_eq!(entry["tags"], json!(["rust", "sqlx"]));
    assert_eq!(entry["author"]["username"], "uthree");
    assert_eq!(entry["posted_via"], json!({ "name": "claude-code", "kind": "agent" }));
    assert_eq!(
        created["warnings"],
        json!(["recommended section not found: 環境 / Environment"])
    );
    let id = entry["id"].as_str().unwrap();
    assert_eq!(entry["uri"], format!("{BASE}/entries/{id}"));

    // Guests can read public entries without a token.
    let (s, e) = g.get(&format!("/api/v1/entries/{id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(e["title"], entry["title"]);
}

#[tokio::test]
async fn registration_needs_a_single_use_invite() {
    let g = Garden::new().await;
    let (s, _) = g.post("/api/v1/register", None, json!({ "username": "nobody" })).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (_, inv) = g.post("/api/v1/invites", Some(&g.admin), json!({})).await;
    let code = inv["code"].clone();
    let (s, _) = g
        .post(
            "/api/v1/register",
            None,
            json!({ "invite_code": code, "username": "first" }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, _) = g
        .post(
            "/api/v1/register",
            None,
            json!({ "invite_code": code, "username": "second" }),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn only_admins_invite_and_only_humans_issue_tokens() {
    let g = Garden::new().await;
    let human = g.member("alice").await;
    let agent = g.agent_token(&human, "bot").await;

    let (s, _) = g.post("/api/v1/invites", Some(&human), json!({})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = g
        .post("/api/v1/tokens", Some(&agent), json!({ "name": "escalate" }))
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = g
        .post(
            "/api/v1/tokens",
            Some(&human),
            json!({ "name": "x", "scopes": ["admin"] }),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn invalid_and_revoked_tokens_are_rejected() {
    let g = Garden::new().await;
    let (s, _) = g.get("/api/v1/entries", Some("kg_nope")).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    let human = g.member("alice").await;
    let agent = g.agent_token(&human, "bot").await;
    let (_, me) = g.get("/api/v1/me", Some(&agent)).await;
    let token_id = me["token"]["id"].as_str().unwrap().to_string();
    let (s, _) = g
        .call(
            Method::DELETE,
            &format!("/api/v1/tokens/{token_id}"),
            Some(&human),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = g.get("/api/v1/me", Some(&agent)).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn visibility_rules_apply_to_reads_and_lists() {
    let g = Garden::new().await;
    let alice = g.member("alice").await;
    let bob = g.member("bob").await;

    let mut ids = std::collections::HashMap::new();
    for vis in ["public", "unlisted", "federated", "members", "private"] {
        let e = g.entry(&alice, memo(vis, vis)).await;
        ids.insert(vis, e["entry"]["id"].as_str().unwrap().to_string());
    }

    // (visibility, guest can read, bob can read)
    for (vis, guest, member) in [
        ("public", true, true),
        ("unlisted", true, true),
        ("federated", false, true),
        ("members", false, true),
        ("private", false, false),
    ] {
        let uri = format!("/api/v1/entries/{}", ids[vis]);
        let expect = |ok: bool| if ok { StatusCode::OK } else { StatusCode::NOT_FOUND };
        assert_eq!(g.get(&uri, None).await.0, expect(guest), "guest {vis}");
        assert_eq!(g.get(&uri, Some(&bob)).await.0, expect(member), "bob {vis}");
        assert_eq!(g.get(&uri, Some(&alice)).await.0, StatusCode::OK, "alice {vis}");
    }

    let titles = |v: &Value| {
        let mut t: Vec<String> = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["title"].as_str().unwrap().into())
            .collect();
        t.sort();
        t
    };
    assert_eq!(titles(&g.get("/api/v1/entries", None).await.1), vec!["public"]);
    assert_eq!(
        titles(&g.get("/api/v1/entries", Some(&bob)).await.1),
        vec!["federated", "members", "public"]
    );
    assert_eq!(
        titles(&g.get("/api/v1/entries", Some(&alice)).await.1),
        vec!["federated", "members", "private", "public", "unlisted"]
    );
}

#[tokio::test]
async fn guest_read_can_be_disabled() {
    let mut config = Config::new(BASE, "test");
    config.guest_read = false;
    let g = Garden::with_config(config).await;
    let e = g.entry(&g.admin, memo("hello", "public")).await;
    let id = e["entry"]["id"].as_str().unwrap();
    assert_eq!(
        g.get(&format!("/api/v1/entries/{id}"), None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(g.get("/api/v1/entries", None).await.1["total"], 0);
}

#[tokio::test]
async fn revisions_are_new_immutable_versions() {
    let g = Garden::new().await;
    let alice = g.member("alice").await;
    let bob = g.member("bob").await;
    let e = g.entry(&alice, memo("v1 title", "public")).await;
    let id = e["entry"]["id"].as_str().unwrap();

    let (s, r) = g
        .post(
            &format!("/api/v1/entries/{id}/versions"),
            Some(&alice),
            json!({ "body": "new body" }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{r}");
    assert_eq!(r["entry"]["version"], 2);
    assert_eq!(r["entry"]["title"], "v1 title");
    assert_eq!(r["entry"]["body"], "new body");

    let (_, v1) = g.get(&format!("/api/v1/entries/{id}?version=1"), None).await;
    assert_eq!(v1["body"], "body");
    assert_eq!(v1["current_version"], 2);
    let (_, versions) = g.get(&format!("/api/v1/entries/{id}/versions"), None).await;
    assert_eq!(versions.as_array().unwrap().len(), 2);

    // Others cannot revise; admins can change metadata.
    let (s, _) = g
        .post(
            &format!("/api/v1/entries/{id}/versions"),
            Some(&bob),
            json!({ "body": "x" }),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, p) = g
        .call(
            Method::PATCH,
            &format!("/api/v1/entries/{id}"),
            Some(&g.admin),
            Some(json!({ "status": "retracted" })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{p}");
    assert_eq!(p["status"], "retracted");
}

#[tokio::test]
async fn tags_filter_and_counts() {
    let g = Garden::new().await;
    let a = g.member("alice").await;
    for (title, tags) in [
        ("a", vec!["rust", "sqlite"]),
        ("b", vec!["rust"]),
        ("c", vec!["python"]),
    ] {
        g.entry(&a, json!({ "kind": "memo", "title": title, "body": "x", "tags": tags }))
            .await;
    }
    let (_, all_rust) = g.get("/api/v1/entries?tag=rust", None).await;
    assert_eq!(all_rust["total"], 2);
    let (_, both) = g.get("/api/v1/entries?tag=rust,SQLite", None).await;
    assert_eq!(both["total"], 1);
    let (_, any) = g.get("/api/v1/entries?any_tag=sqlite,python", None).await;
    assert_eq!(any["total"], 2);
    let (_, counts) = g.get("/api/v1/tags", None).await;
    assert_eq!(counts[0], json!({ "tag": "rust", "count": 2 }));
}

#[tokio::test]
async fn links_and_backlinks_respect_visibility() {
    let g = Garden::new().await;
    let alice = g.member("alice").await;
    let bob = g.member("bob").await;
    let target = g.entry(&alice, memo("target", "public")).await["entry"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let secret = g.entry(&alice, memo("secret", "private")).await["entry"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Bob cannot link to Alice's private entry.
    let (s, _) = g
        .post(
            "/api/v1/entries",
            Some(&bob),
            json!({ "kind": "memo", "title": "x", "body": "x", "links": [{ "target": secret, "relation": "cites" }] }),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // A public report reproducing the target, linked by URI, plus a remote citation.
    let report = g
        .entry(
            &bob,
            json!({
                "kind": "research_report", "title": "replication", "body": "x",
                "links": [
                    { "target": format!("{BASE}/entries/{target}"), "relation": "reproduces", "note": "same result" },
                    { "target": "https://other.garden/entries/123", "relation": "cites" }
                ]
            }),
        )
        .await;
    let report_id = report["entry"]["id"].as_str().unwrap();

    // Alice's private entry also links to the target; others must not see that backlink.
    g.post(
        &format!("/api/v1/entries/{secret}/links"),
        Some(&alice),
        json!({ "target": target, "relation": "extends" }),
    )
    .await;

    let (_, out) = g.get(&format!("/api/v1/entries/{report_id}/links"), None).await;
    assert_eq!(out["outgoing"].as_array().unwrap().len(), 2);
    assert_eq!(out["outgoing"][0]["target_title"], "target");
    assert_eq!(out["outgoing"][1]["target_id"], Value::Null);

    let (_, as_guest) = g.get(&format!("/api/v1/entries/{target}/links"), None).await;
    assert_eq!(as_guest["incoming"].as_array().unwrap().len(), 1);
    let (_, as_alice) = g.get(&format!("/api/v1/entries/{target}/links"), Some(&alice)).await;
    assert_eq!(as_alice["incoming"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn comments_and_reproduction_reports() {
    let g = Garden::new().await;
    let alice = g.member("alice").await;
    let bob = g.member("bob").await;
    let bob_agent = g.agent_token(&bob, "qwen").await;
    let id = g
        .entry(&alice, json!({ "kind": "research_note", "title": "exp", "body": "x" }))
        .await["entry"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let url = format!("/api/v1/entries/{id}/comments");

    let (s, c) = g
        .post(
            &url,
            Some(&bob_agent),
            json!({ "kind": "reproduction", "body": "再現できた", "result": "reproduced", "environment": { "os": "linux" } }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{c}");
    assert_eq!(c["entry_version"], 1);
    assert_eq!(c["posted_via"]["kind"], "agent");
    g.post(
        &url,
        Some(&alice),
        json!({ "kind": "reproduction", "body": "失敗", "result": "failed" }),
    )
    .await;

    let (s, _) = g
        .post(&url, Some(&bob), json!({ "kind": "reproduction", "body": "no result" }))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = g
        .post(&url, Some(&bob), json!({ "body": "plain", "result": "failed" }))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // One level of replies only.
    let parent = c["id"].as_str().unwrap();
    let (s, reply) = g
        .post(&url, Some(&alice), json!({ "body": "ありがとう", "parent_id": parent }))
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, _) = g
        .post(&url, Some(&bob), json!({ "body": "deeper", "parent_id": reply["id"] }))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let (_, entry) = g.get(&format!("/api/v1/entries/{id}"), None).await;
    assert_eq!(
        entry["reproductions"],
        json!({ "reproduced": 1, "partial": 0, "failed": 1 })
    );
    let (_, list) = g.get(&url, None).await;
    assert_eq!(list.as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn comments_on_hidden_entries_are_not_found() {
    let g = Garden::new().await;
    let alice = g.member("alice").await;
    let bob = g.member("bob").await;
    let id = g.entry(&alice, memo("secret", "private")).await["entry"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let url = format!("/api/v1/entries/{id}/comments");
    assert_eq!(
        g.post(&url, Some(&bob), json!({ "body": "hi" })).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(g.get(&url, None).await.0, StatusCode::NOT_FOUND);
}
