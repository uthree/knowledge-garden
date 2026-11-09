mod common;

use axum::http::{Method, StatusCode};
use common::{BASE, Garden};
use knowledge_garden::config::Config;
use serde_json::{Value, json};

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

async fn search(g: &Garden, query: &str, token: Option<&str>) -> Value {
    let (s, v) = g.get(&format!("/api/v1/search?{query}"), token).await;
    assert_eq!(s, StatusCode::OK, "{query}: {v}");
    v
}

fn hit_titles(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["entry"]["title"].as_str().unwrap().into())
        .collect()
}

fn q(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

async fn seed_search(g: &Garden, token: &str) {
    for (kind, title, body, tags) in [
        (
            "troubleshooting",
            "sqlx のコンパイルエラー",
            "# 症状\nerror[E0107]: struct takes 0 lifetime arguments\n# 解決策\nQueryBuilder<Sqlite> と書く",
            vec!["rust", "sqlx"],
        ),
        (
            "research_note",
            "全文検索の調査",
            "SQLite FTS5 の trigram トークナイザで日本語の検索を試した。",
            vec!["sqlite", "search"],
        ),
        (
            "memo",
            "Python の型ヒント",
            "mypy と pyright の違いについてのメモ。検索とは無関係。",
            vec!["python"],
        ),
    ] {
        g.entry(
            token,
            json!({ "kind": kind, "title": title, "body": body, "tags": tags }),
        )
        .await;
    }
}

#[tokio::test]
async fn search_terms_phrases_and_exclusions() {
    let g = Garden::new().await;
    let a = g.member("alice").await;
    seed_search(&g, &a).await;

    let v = search(&g, &format!("q={}", q("trigram")), None).await;
    assert_eq!(hit_titles(&v), vec!["全文検索の調査"]);
    let hit = &v["items"][0];
    assert!(hit["score"].as_f64().unwrap() > 0.0);
    assert!(hit["snippet"].as_str().unwrap().contains("trigram"));
    assert_eq!(hit["matched_in"], json!(["body"]));

    // Japanese 3+ character term via the trigram index.
    assert_eq!(
        hit_titles(&search(&g, &format!("q={}", q("トークナイザ")), None).await),
        vec!["全文検索の調査"]
    );
    // Two-character Japanese term falls back to substring matching.
    let mut both = hit_titles(&search(&g, &format!("q={}", q("検索")), None).await);
    both.sort();
    assert_eq!(both, vec!["Python の型ヒント", "全文検索の調査"]);
    // Exclusion.
    assert_eq!(
        hit_titles(&search(&g, &format!("q={}", q("検索 -mypy")), None).await),
        vec!["全文検索の調査"]
    );
    // Phrase and case-insensitivity.
    assert_eq!(
        hit_titles(&search(&g, &format!("q={}", q("\"lifetime ARGUMENTS\"")), None).await),
        vec!["sqlx のコンパイルエラー"]
    );
    // All terms must match.
    assert_eq!(search(&g, &format!("q={}", q("trigram mypy")), None).await["total"], 0);
}

#[tokio::test]
async fn search_by_regex_and_filters() {
    let g = Garden::new().await;
    let a = g.member("alice").await;
    seed_search(&g, &a).await;

    let v = search(&g, &format!("regex={}", q(r"error\[E\d{4}\]")), None).await;
    assert_eq!(hit_titles(&v), vec!["sqlx のコンパイルエラー"]);
    assert!(v["items"][0]["snippet"].as_str().unwrap().contains("E0107"));
    assert_eq!(
        search(&g, &format!("regex={}&regex_field=title", q(r"E\d{4}")), None).await["total"],
        0
    );

    assert_eq!(
        search(&g, &format!("q={}&kind=memo", q("検索")), None).await["total"],
        1
    );
    assert_eq!(
        search(&g, &format!("q={}&tag=sqlite&limit=5", q("検索")), None).await["total"],
        1
    );

    let (s, _) = g.get(&format!("/api/v1/search?regex={}", q("(unclosed")), None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = g.get("/api/v1/search", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn search_respects_visibility_and_follows_revisions() {
    let g = Garden::new().await;
    let alice = g.member("alice").await;
    let bob = g.member("bob").await;
    let e = g
        .entry(
            &alice,
            json!({ "kind": "memo", "title": "secret plan", "body": "original wording", "visibility": "private" }),
        )
        .await;
    let id = e["entry"]["id"].as_str().unwrap();
    g.entry(
        &alice,
        json!({ "kind": "memo", "title": "unlisted plan", "body": "x", "visibility": "unlisted" }),
    )
    .await;

    assert_eq!(search(&g, "q=plan", None).await["total"], 0);
    assert_eq!(search(&g, "q=plan", Some(&bob)).await["total"], 0);
    assert_eq!(search(&g, "q=plan", Some(&alice)).await["total"], 2);

    // The index follows the current version and tag changes.
    g.post(
        &format!("/api/v1/entries/{id}/versions"),
        Some(&alice),
        json!({ "body": "revised wording" }),
    )
    .await;
    assert_eq!(search(&g, "q=original", Some(&alice)).await["total"], 0);
    assert_eq!(search(&g, "q=revised", Some(&alice)).await["total"], 1);
    g.call(
        Method::PATCH,
        &format!("/api/v1/entries/{id}"),
        Some(&alice),
        Some(json!({ "tags": ["strategy"], "visibility": "public" })),
    )
    .await;
    let v = search(&g, "q=strategy", None).await;
    assert_eq!(v["total"], 1);
    assert_eq!(v["items"][0]["matched_in"], json!(["tags"]));
}

#[tokio::test]
async fn list_filters_by_reproduction_and_date() {
    let g = Garden::new().await;
    let a = g.member("alice").await;
    let e = g
        .entry(&a, json!({ "kind": "research_note", "title": "exp", "body": "x" }))
        .await;
    g.entry(&a, json!({ "kind": "research_note", "title": "other", "body": "x" }))
        .await;
    let id = e["entry"]["id"].as_str().unwrap();
    g.post(
        &format!("/api/v1/entries/{id}/comments"),
        Some(&a),
        json!({ "kind": "reproduction", "body": "ok", "result": "reproduced" }),
    )
    .await;

    assert_eq!(
        g.get("/api/v1/entries?has_reproduction=reproduced", None).await.1["total"],
        1
    );
    assert_eq!(
        g.get("/api/v1/entries?has_reproduction=failed", None).await.1["total"],
        0
    );
    assert_eq!(g.get("/api/v1/entries?since=2000-01-01", None).await.1["total"], 2);
    assert_eq!(g.get("/api/v1/entries?until=2000-01-01", None).await.1["total"], 0);
    assert_eq!(
        g.get("/api/v1/entries?since=yesterday", None).await.0,
        StatusCode::BAD_REQUEST
    );
}
