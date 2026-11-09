mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use common::Garden;
use http_body_util::BodyExt;
use knowledge_garden::config::Config;
use knowledge_garden::error::{Error, Result};
use knowledge_garden::federation::{PeerClient, PeerResponse};
use serde_json::{Value, json};
use tower::ServiceExt;

const A: &str = "http://a.test";
const B: &str = "http://b.test";

/// Routes requests for `http://<host>` to the in-memory Garden registered for it.
#[derive(Default)]
struct Network(Mutex<HashMap<String, Router>>);

#[async_trait]
impl PeerClient for Network {
    async fn get(&self, url: &str, headers: Vec<(String, String)>) -> Result<PeerResponse> {
        let parsed = url::Url::parse(url).unwrap();
        let base = format!("{}://{}", parsed.scheme(), parsed.host_str().unwrap());
        let app = self.0.lock().unwrap().get(&base).cloned();
        let app = app.ok_or_else(|| Error::BadRequest(format!("could not reach {url}")))?;
        let path = &url[base.len()..];
        let mut req = Request::builder().method(Method::GET).uri(path);
        for (k, v) in headers {
            req = req.header(k, v);
        }
        let resp = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        Ok(PeerResponse {
            status,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        })
    }
}

async fn pair() -> (Garden, Garden, Arc<Network>) {
    let net = Arc::new(Network::default());
    let a = Garden::with_peer_client(Config::new(A, "Garden A"), Some(net.clone())).await;
    let b = Garden::with_peer_client(Config::new(B, "Garden B"), Some(net.clone())).await;
    net.0.lock().unwrap().insert(A.into(), a.app.clone());
    net.0.lock().unwrap().insert(B.into(), b.app.clone());
    (a, b, net)
}

async fn approve(g: &Garden, peer: &str) -> String {
    let (s, p) = g
        .post(
            "/api/v1/peers",
            Some(&g.admin),
            json!({ "base_url": format!("{peer}/") }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{p}");
    assert_eq!(p["base_url"], peer);
    p["id"].as_str().unwrap().to_string()
}

async fn sync(g: &Garden, peer_id: &str) -> Value {
    let (s, r) = g
        .post(&format!("/api/v1/peers/{peer_id}/sync"), Some(&g.admin), json!({}))
        .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    r
}

fn titles(v: &Value) -> Vec<String> {
    let mut t: Vec<String> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e.get("entry").unwrap_or(e)["title"].as_str().unwrap().to_string())
        .collect();
    t.sort();
    t
}

#[tokio::test]
async fn well_known_describes_the_garden() {
    let (a, _, _) = pair().await;
    let (s, v) = a.get("/.well-known/knowledge-garden", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["base_url"], A);
    assert_eq!(v["name"], "Garden A");
    // The key is stable across calls.
    let (_, again) = a.get("/.well-known/knowledge-garden", None).await;
    assert_eq!(v["public_key"], again["public_key"]);
}

#[tokio::test]
async fn feed_requires_an_approved_signed_peer() {
    let (a, b, _) = pair().await;
    // Unsigned.
    let (s, _) = a.get("/federation/v1/feed", None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // B approves A and tries to pull, but A has not approved B.
    let a_on_b = approve(&b, A).await;
    let (s, r) = b
        .post(&format!("/api/v1/peers/{a_on_b}/sync"), Some(&b.admin), json!({}))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{r}");
    assert!(r["error"]["message"].as_str().unwrap().contains("403"), "{r}");
    let (_, peers) = b.get("/api/v1/peers", Some(&b.admin)).await;
    assert!(peers[0]["last_error"].as_str().unwrap().contains("not approved"));

    // Only admins manage peers.
    let member = b.member("bob").await;
    assert_eq!(b.get("/api/v1/peers", Some(&member)).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn mirrors_shared_entries_and_follows_changes() {
    let (a, b, _) = pair().await;
    let alice = a.member("alice").await;
    let agent = a.agent_token(&alice, "claude-code").await;
    let public = a
        .entry(
            &agent,
            json!({ "kind": "troubleshooting", "title": "NAS で cargo が落ちる", "body": "# 症状\nlinker が OOM", "tags": ["rust"] }),
        )
        .await["entry"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let federated = a
        .entry(
            &alice,
            json!({ "kind": "memo", "title": "仲間内のメモ", "body": "federated body", "visibility": "federated" }),
        )
        .await["entry"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    a.entry(
        &alice,
        json!({ "kind": "memo", "title": "members only", "body": "x", "visibility": "members" }),
    )
    .await;
    a.entry(
        &alice,
        json!({ "kind": "memo", "title": "private", "body": "x", "visibility": "private" }),
    )
    .await;
    // A link from the public entry to the federated one travels with it.
    a.post(
        &format!("/api/v1/entries/{public}/links"),
        Some(&alice),
        json!({ "target": federated, "relation": "cites" }),
    )
    .await;

    approve(&a, B).await;
    let a_on_b = approve(&b, A).await;
    let report = sync(&b, &a_on_b).await;
    assert_eq!(report["received"], 2, "{report}");
    assert_eq!(report["created"], 2);
    assert_eq!(report["rejected"], json!([]));

    // Guests on B see the public mirror; B's members also see the federated one (as members-only).
    let bob = b.member("bob").await;
    assert_eq!(
        titles(&b.get("/api/v1/entries?origin=remote", None).await.1),
        vec!["NAS で cargo が落ちる"]
    );
    assert_eq!(
        titles(&b.get("/api/v1/entries?origin=remote", Some(&bob)).await.1),
        vec!["NAS で cargo が落ちる", "仲間内のメモ"]
    );
    let (_, hits) = b.get("/api/v1/search?q=linker", None).await;
    assert_eq!(hits["total"], 1);
    let mirror = &hits["items"][0]["entry"];
    assert_eq!(mirror["is_local"], false);
    assert_eq!(mirror["origin"], A);
    assert_eq!(mirror["uri"], format!("{A}/entries/{public}"));
    assert_eq!(mirror["author"]["username"], "alice@a.test");
    assert_eq!(mirror["posted_via"], json!({ "name": "claude-code", "kind": "agent" }));
    let mirror_id = mirror["id"].as_str().unwrap().to_string();

    // The link resolved to the local mirror of its target.
    let (_, links) = b.get(&format!("/api/v1/entries/{mirror_id}/links"), Some(&bob)).await;
    assert_eq!(links["outgoing"][0]["target_title"], "仲間内のメモ");

    // Mirrors are read-only, even for B's admin, but B's members can comment locally.
    let (s, _) = b
        .post(
            &format!("/api/v1/entries/{mirror_id}/versions"),
            Some(&b.admin),
            json!({ "body": "changed" }),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = b
        .post(
            &format!("/api/v1/entries/{mirror_id}/comments"),
            Some(&bob),
            json!({ "kind": "reproduction", "body": "B でも再現", "result": "reproduced" }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);

    // Nothing new: an incremental sync receives nothing.
    assert_eq!(sync(&b, &a_on_b).await["received"], 0);

    // A revision and a visibility change on A reach B.
    a.post(
        &format!("/api/v1/entries/{public}/versions"),
        Some(&agent),
        json!({ "body": "# 症状\nlinker が OOM\n# 解決策\n-j1" }),
    )
    .await;
    a.call(
        Method::PATCH,
        &format!("/api/v1/entries/{federated}"),
        Some(&alice),
        Some(json!({ "visibility": "private" })),
    )
    .await;
    let report = sync(&b, &a_on_b).await;
    assert_eq!(report["updated"], 1, "{report}");
    assert_eq!(report["withdrawn"], 1, "{report}");
    let (_, e) = b.get(&format!("/api/v1/entries/{mirror_id}"), None).await;
    assert_eq!(e["current_version"], 2);
    assert!(e["body"].as_str().unwrap().contains("-j1"));
    assert_eq!(e["reproductions"]["reproduced"], 1);
    assert_eq!(
        titles(&b.get("/api/v1/entries?origin=remote", Some(&bob)).await.1),
        vec!["NAS で cargo が落ちる"]
    );

    // Mirrors are never passed on: B's feed to A carries only B's own entries.
    let (_, peers) = a.get("/api/v1/peers", Some(&a.admin)).await;
    let b_on_a = peers[0]["id"].as_str().unwrap().to_string();
    assert_eq!(sync(&a, &b_on_a).await["received"], 0);
    b.entry(&bob, json!({ "kind": "memo", "title": "from B", "body": "x" }))
        .await;
    let report = sync(&a, &b_on_a).await;
    assert_eq!(report["received"], 1, "{report}");
    assert_eq!(
        titles(&a.get("/api/v1/entries?origin=remote", None).await.1),
        vec!["from B"]
    );
}

#[tokio::test]
async fn blocked_peers_lose_access_and_tampering_is_rejected() {
    let (a, b, net) = pair().await;
    let alice = a.member("alice").await;
    a.entry(&alice, json!({ "kind": "memo", "title": "hello", "body": "x" }))
        .await;
    let b_on_a = approve(&a, B).await;
    let a_on_b = approve(&b, A).await;

    // A blocks B: B can no longer pull.
    let (s, _) = a
        .call(
            Method::PATCH,
            &format!("/api/v1/peers/{b_on_a}"),
            Some(&a.admin),
            Some(json!({ "status": "blocked" })),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = b
        .post(&format!("/api/v1/peers/{a_on_b}/sync"), Some(&b.admin), json!({}))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    a.call(
        Method::PATCH,
        &format!("/api/v1/peers/{b_on_a}"),
        Some(&a.admin),
        Some(json!({ "status": "approved" })),
    )
    .await;

    // An impostor serves A's feed with a modified title: the signature check rejects it.
    let real_a = net.0.lock().unwrap().get(A).cloned().unwrap();
    let tampering = Router::new().fallback(move |req: Request<Body>| {
        let real_a = real_a.clone();
        async move {
            let resp = real_a.oneshot(req).await.unwrap();
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            let mut v: Value = serde_json::from_slice(&bytes).unwrap();
            if let Some(items) = v["items"].as_array_mut() {
                for item in items {
                    item["entry"]["title"] = json!("tampered");
                }
            }
            axum::Json(v)
        }
    });
    net.0.lock().unwrap().insert(A.into(), tampering);
    let report = sync(&b, &a_on_b).await;
    assert_eq!(report["received"], 1);
    assert_eq!(report["created"], 0);
    assert!(
        report["rejected"][0].as_str().unwrap().contains("bad signature"),
        "{report}"
    );
    assert_eq!(b.get("/api/v1/entries?origin=remote", None).await.1["total"], 0);
}

#[tokio::test]
async fn adding_peers_is_validated() {
    let (a, _, _) = pair().await;
    let (s, _) = a.post("/api/v1/peers", Some(&a.admin), json!({ "base_url": A })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "self");
    let (s, _) = a
        .post(
            "/api/v1/peers",
            Some(&a.admin),
            json!({ "base_url": "http://nowhere.test" }),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "unreachable");
    let (s, _) = a
        .post("/api/v1/peers", Some(&a.admin), json!({ "base_url": "ftp://b.test" }))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "scheme");
    approve(&a, B).await;
    let (s, _) = a.post("/api/v1/peers", Some(&a.admin), json!({ "base_url": B })).await;
    assert_eq!(s, StatusCode::CONFLICT, "duplicate");
}
