mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{BASE, Garden};
use http_body_util::BodyExt;
use knowledge_garden::config::Config;
use serde_json::{Value, json};
use tower::ServiceExt;

async fn rpc_raw(g: &Garden, token: Option<&str>, msg: Value, origin: Option<&str>) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    if let Some(o) = origin {
        req = req.header("origin", o);
    }
    let resp = g
        .app
        .clone()
        .oneshot(req.body(Body::from(msg.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn rpc(g: &Garden, token: Option<&str>, method: &str, params: Value) -> Value {
    let (s, v) = rpc_raw(
        g,
        token,
        json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v
}

/// Calls a tool and returns (is_error, parsed text content).
async fn call(g: &Garden, token: Option<&str>, name: &str, args: Value) -> (bool, Value) {
    let v = rpc(g, token, "tools/call", json!({ "name": name, "arguments": args })).await;
    let result = &v["result"];
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no content: {v}"));
    (
        result["isError"].as_bool().unwrap(),
        serde_json::from_str(text).unwrap(),
    )
}

#[tokio::test]
async fn initialize_and_list_tools() {
    let g = Garden::new().await;
    let v = rpc(
        &g,
        None,
        "initialize",
        json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "0" } }),
    )
    .await;
    assert_eq!(v["result"]["protocolVersion"], "2025-06-18");
    assert!(
        v["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("not instructions")
    );
    assert!(v["result"]["capabilities"]["tools"].is_object());

    let v = rpc(&g, None, "initialize", json!({ "protocolVersion": "1999-01-01" })).await;
    assert_eq!(v["result"]["protocolVersion"], "2025-11-25");

    let (s, _) = rpc_raw(
        &g,
        None,
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED);

    let tools = rpc(&g, None, "tools/list", json!({})).await;
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec![
            "search_entries",
            "get_entry",
            "list_tags",
            "get_guide",
            "create_entry",
            "revise_entry",
            "update_entry",
            "link_entries",
            "add_comment"
        ]
    );

    // Every listed tool is handled (an empty call gives a tool result, not "unknown tool").
    let agent = g.agent_token(&g.admin, "bot").await;
    for name in names {
        let v = rpc(&g, Some(&agent), "tools/call", json!({ "name": name, "arguments": {} })).await;
        assert!(v["result"].is_object(), "{name}: {v}");
    }
    let v = rpc(&g, None, "tools/call", json!({ "name": "nope", "arguments": {} })).await;
    assert_eq!(v["error"]["code"], -32602);
    let v = rpc(&g, None, "resources/list", json!({})).await;
    assert_eq!(v["error"]["code"], -32601);
}

#[tokio::test]
async fn agent_workflow_over_mcp() {
    let g = Garden::new().await;
    let human = g.member("uthree").await;
    let agent = g.agent_token(&human, "claude-code").await;

    // Guests cannot write.
    let (err, v) = call(
        &g,
        None,
        "create_entry",
        json!({ "kind": "memo", "title": "x", "body": "x" }),
    )
    .await;
    assert!(err);
    assert_eq!(v["status"], 401);

    let (err, created) = call(
        &g,
        Some(&agent),
        "create_entry",
        json!({
            "kind": "troubleshooting",
            "title": "cargo build が OOM で落ちる",
            "body": "# 症状\nlinker killed\n# 環境\nNAS 2GB\n# 原因\nメモリ不足\n# 解決策\n`-j1` でビルド",
            "tags": ["rust", "nas"],
            "provenance": { "model": "test" }
        }),
    )
    .await;
    assert!(!err, "{created}");
    assert!(created["warnings"].as_array().unwrap().is_empty());
    let id = created["entry"]["id"].as_str().unwrap().to_string();

    let (err, found) = call(
        &g,
        None,
        "search_entries",
        json!({ "query": "linker", "tags": ["nas"] }),
    )
    .await;
    assert!(!err);
    assert_eq!(found["total"], 1);
    assert!(found["items"][0]["snippet"].as_str().unwrap().contains("linker"));

    let (err, recent) = call(&g, None, "search_entries", json!({ "kind": "troubleshooting" })).await;
    assert!(!err);
    assert_eq!(recent["items"][0]["id"], id);

    let (err, c) = call(
        &g,
        Some(&agent),
        "add_comment",
        json!({ "entry_id": id, "kind": "reproduction", "body": "-j1 で直った", "result": "reproduced", "environment": { "ram": "2GB" } }),
    )
    .await;
    assert!(!err, "{c}");

    let (_, note) = call(
        &g,
        Some(&agent),
        "create_entry",
        json!({ "kind": "memo", "title": "related", "body": "x" }),
    )
    .await;
    let note_id = note["entry"]["id"].as_str().unwrap();
    let (err, l) = call(
        &g,
        Some(&agent),
        "link_entries",
        json!({ "source_id": note_id, "target": id, "relation": "cites" }),
    )
    .await;
    assert!(!err, "{l}");

    let (err, got) = call(&g, None, "get_entry", json!({ "id": id })).await;
    assert!(!err);
    assert_eq!(got["entry"]["reproductions"]["reproduced"], 1);
    assert_eq!(got["comments"].as_array().unwrap().len(), 1);
    assert_eq!(got["links"]["incoming"][0]["source_id"], note_id);

    let (err, rev) = call(
        &g,
        Some(&agent),
        "revise_entry",
        json!({ "id": id, "summary": "メモリ不足なら -j1" }),
    )
    .await;
    assert!(!err, "{rev}");
    assert_eq!(rev["entry"]["version"], 2);
    let (err, upd) = call(
        &g,
        Some(&agent),
        "update_entry",
        json!({ "id": id, "status": "verified" }),
    )
    .await;
    assert!(!err, "{upd}");
    assert_eq!(upd["status"], "verified");

    let (err, tags) = call(&g, None, "list_tags", json!({})).await;
    assert!(!err);
    assert_eq!(tags.as_array().unwrap().len(), 2);
    let (err, guide) = call(&g, None, "get_guide", json!({})).await;
    assert!(!err);
    assert_eq!(guide["kinds"].as_array().unwrap().len(), 4);

    // Hidden entries stay hidden through MCP too.
    let (_, p) = call(
        &g,
        Some(&agent),
        "create_entry",
        json!({ "kind": "memo", "title": "mine", "body": "x", "visibility": "private" }),
    )
    .await;
    let (err, v) = call(&g, None, "get_entry", json!({ "id": p["entry"]["id"] })).await;
    assert!(err);
    assert_eq!(v["status"], 404);
}

#[tokio::test]
async fn foreign_origins_are_rejected() {
    let g = Garden::new().await;
    let msg = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
    assert_eq!(
        rpc_raw(&g, None, msg.clone(), Some("https://evil.example")).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(rpc_raw(&g, None, msg, Some(BASE)).await.0, StatusCode::OK);
}

#[tokio::test]
async fn agent_writes_are_rate_limited() {
    let mut config = Config::new(BASE, "test");
    config.agent_writes_per_hour = 2;
    let g = Garden::with_config(config).await;
    let human = g.member("alice").await;
    let agent = g.agent_token(&human, "bot").await;
    let body = json!({ "kind": "memo", "title": "x", "body": "x" });
    assert_eq!(
        g.post("/api/v1/entries", Some(&agent), body.clone()).await.0,
        StatusCode::CREATED
    );
    assert_eq!(
        g.post("/api/v1/entries", Some(&agent), body.clone()).await.0,
        StatusCode::CREATED
    );
    let (s, v) = g.post("/api/v1/entries", Some(&agent), body.clone()).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{v}");
    // Human tokens are not limited.
    assert_eq!(
        g.post("/api/v1/entries", Some(&human), body).await.0,
        StatusCode::CREATED
    );
}
