//! Files attached to entries: uploads, downloads and who may see them.

mod common;

use axum::http::{Method, StatusCode};
use common::Garden;
use serde_json::json;

async fn entry_with(g: &Garden, token: &str, visibility: &str, body: &str) -> String {
    g.entry(
        token,
        json!({ "kind": "research_note", "title": "実験", "body": body, "visibility": visibility }),
    )
    .await["entry"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn download(
    g: &Garden,
    token: Option<&str>,
    path: &str,
    extra: &[(&str, &str)],
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let auth = token.map(|t| format!("Bearer {t}"));
    let mut headers: Vec<(&str, &str)> = extra.to_vec();
    if let Some(a) = &auth {
        headers.push(("authorization", a));
    }
    g.raw(Method::GET, path, &headers, Vec::new()).await
}

#[tokio::test]
async fn upload_list_download_and_delete() {
    let g = Garden::new().await;
    let alice = g.member("alice").await;
    let id = entry_with(&g, &alice, "public", "![曲線](attachment:loss.png)").await;

    let png = b"\x89PNG fake image bytes".to_vec();
    let (s, a) = g.upload(&alice, &id, "loss.png", &png).await;
    assert_eq!(s, StatusCode::CREATED, "{a}");
    assert_eq!(a["content_type"], "image/png");
    assert_eq!(a["preview"], "image");
    assert_eq!(a["size"], png.len());
    assert_eq!(a["url"], format!("{}/entries/{id}/files/loss.png", common::BASE));
    assert_eq!(a["markdown"], "![loss.png](attachment:loss.png)");
    assert_eq!(a["posted_via"]["name"], "default");

    // The entry lists its files.
    let (_, e) = g.get(&format!("/api/v1/entries/{id}"), None).await;
    assert_eq!(e["attachments"][0]["name"], "loss.png");
    let (_, list) = g.get(&format!("/api/v1/entries/{id}/attachments"), None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);

    // Download, at both URLs.
    for path in [
        format!("/entries/{id}/files/loss.png"),
        format!("/api/v1/entries/{id}/attachments/loss.png"),
    ] {
        let (s, h, body) = download(&g, None, &path, &[]).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(body, png);
        assert_eq!(h["content-type"], "image/png");
        assert_eq!(h["x-content-type-options"], "nosniff");
        assert!(h["content-disposition"].to_str().unwrap().starts_with("inline;"));
        assert!(h["content-security-policy"].to_str().unwrap().contains("sandbox"));
    }

    // Ranges (for seeking in audio and video) and revalidation.
    let path = format!("/entries/{id}/files/loss.png");
    let (s, h, body) = download(&g, None, &path, &[("range", "bytes=1-3")]).await;
    assert_eq!(s, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body, b"PNG");
    assert_eq!(h["content-range"], format!("bytes 1-3/{}", png.len()));
    let (s, _, _) = download(&g, None, &path, &[("range", "bytes=999-")]).await;
    assert_eq!(s, StatusCode::RANGE_NOT_SATISFIABLE);
    let etag = h["etag"].to_str().unwrap().to_string();
    let (s, _, body) = download(&g, None, &path, &[("if-none-match", &etag)]).await;
    assert_eq!(s, StatusCode::NOT_MODIFIED);
    assert!(body.is_empty());
    let (_, h, _) = download(&g, None, &format!("{path}?download=1"), &[]).await;
    assert!(h["content-disposition"].to_str().unwrap().starts_with("attachment;"));

    // Same name again: conflict, unless replacing.
    let (s, _) = g.upload(&alice, &id, "loss.png", b"other").await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, a) = g.upload(&alice, &id, "loss.png&replace=true", b"second").await;
    assert_eq!(s, StatusCode::CREATED, "{a}");
    let (_, _, body) = download(&g, None, &path, &[]).await;
    assert_eq!(body, b"second");

    // Delete.
    let auth = format!("Bearer {alice}");
    let (s, _, _) = g
        .raw(
            Method::DELETE,
            &format!("/api/v1/entries/{id}/attachments/loss.png"),
            &[("authorization", &auth)],
            vec![],
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(download(&g, None, &path, &[]).await.0, StatusCode::NOT_FOUND);
    let (_, e) = g.get(&format!("/api/v1/entries/{id}"), None).await;
    assert_eq!(e["attachments"], json!([]));
}

#[tokio::test]
async fn files_follow_the_entry_visibility() {
    let g = Garden::new().await;
    let alice = g.member("alice").await;
    let bob = g.member("bob").await;

    for (visibility, guest, member) in [
        ("public", true, true),
        ("unlisted", true, true),
        ("members", false, true),
        ("federated", false, true),
        ("private", false, false),
    ] {
        let id = entry_with(&g, &alice, visibility, "x").await;
        let (s, _) = g.upload(&alice, &id, "data.csv", b"a,b\n1,2\n").await;
        assert_eq!(s, StatusCode::CREATED);
        let path = format!("/entries/{id}/files/data.csv");
        let ok = |b: bool| if b { StatusCode::OK } else { StatusCode::NOT_FOUND };
        assert_eq!(download(&g, None, &path, &[]).await.0, ok(guest), "guest {visibility}");
        assert_eq!(
            download(&g, Some(&bob), &path, &[]).await.0,
            ok(member),
            "member {visibility}"
        );
        assert_eq!(
            download(&g, Some(&alice), &path, &[]).await.0,
            StatusCode::OK,
            "author {visibility}"
        );
        let list = format!("/api/v1/entries/{id}/attachments");
        assert_eq!(g.get(&list, None).await.0, ok(guest), "list {visibility}");
    }

    // Narrowing the entry hides its files at once.
    let id = entry_with(&g, &alice, "public", "x").await;
    g.upload(&alice, &id, "a.txt", b"hello").await;
    let path = format!("/entries/{id}/files/a.txt");
    assert_eq!(download(&g, None, &path, &[]).await.0, StatusCode::OK);
    g.call(
        Method::PATCH,
        &format!("/api/v1/entries/{id}"),
        Some(&alice),
        Some(json!({ "visibility": "private" })),
    )
    .await;
    assert_eq!(download(&g, None, &path, &[]).await.0, StatusCode::NOT_FOUND);
    assert_eq!(download(&g, Some(&bob), &path, &[]).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn only_editors_upload_and_names_and_types_are_checked() {
    let g = Garden::new().await;
    let alice = g.member("alice").await;
    let bob = g.member("bob").await;
    let id = entry_with(&g, &alice, "public", "x").await;

    // Another member cannot attach; the admin can.
    assert_eq!(g.upload(&bob, &id, "x.txt", b"x").await.0, StatusCode::FORBIDDEN);
    assert_eq!(g.upload(&g.admin, &id, "admin.txt", b"x").await.0, StatusCode::CREATED);
    // Guests cannot either.
    let (s, _, _) = g
        .raw(
            Method::POST,
            &format!("/api/v1/entries/{id}/attachments?name=x.txt"),
            &[],
            b"x".to_vec(),
        )
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // A missing entry is 404.
    assert_eq!(g.upload(&alice, "nope", "x.txt", b"x").await.0, StatusCode::NOT_FOUND);

    for bad in ["", ".env", "a%2Fb.txt", "a%20b.txt"] {
        let (s, e) = g.upload(&alice, &id, bad, b"x").await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}: {e}");
    }

    // HTML is never served as HTML.
    let (s, a) = g.upload(&alice, &id, "page.html", b"<script>alert(1)</script>").await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(a["content_type"], "application/octet-stream");
    assert_eq!(a["preview"], "none");
    assert_eq!(a["markdown"], "[page.html](attachment:page.html)");
    let (_, h, _) = download(&g, None, &format!("/entries/{id}/files/page.html"), &[]).await;
    assert_eq!(h["content-type"], "application/octet-stream");
    assert!(h["content-disposition"].to_str().unwrap().starts_with("attachment;"));

    // The declared type is used when the extension says nothing.
    let auth = format!("Bearer {alice}");
    let (s, _, body) = g
        .raw(
            Method::POST,
            &format!("/api/v1/entries/{id}/attachments?name=recording"),
            &[("authorization", &auth), ("content-type", "audio/mpeg")],
            b"ID3".to_vec(),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let a: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(a["content_type"], "audio/mpeg");
    assert_eq!(a["preview"], "audio");

    // Japanese names work and are percent-encoded in URLs.
    let (s, a) = g.upload(&alice, &id, "%E6%90%8D%E5%A4%B1.png", b"png").await;
    assert_eq!(s, StatusCode::CREATED, "{a}");
    assert_eq!(a["name"], "損失.png");
    let (s, _, _) = download(&g, None, &format!("/entries/{id}/files/%E6%90%8D%E5%A4%B1.png"), &[]).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn uploads_respect_the_size_limit() {
    let mut config = knowledge_garden::config::Config::new(common::BASE, "test");
    config.max_attachment_bytes = 10;
    let g = Garden::with_config(config).await;
    let id = entry_with(&g, &g.admin.clone(), "public", "x").await;
    assert_eq!(
        g.upload(&g.admin, &id, "ok.txt", b"0123456789").await.0,
        StatusCode::CREATED
    );
    let (s, e) = g.upload(&g.admin, &id, "big.txt", b"0123456789A").await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE, "{e}");
    assert_eq!(e["error"]["code"], "too_large");
    let (_, list) = g.get(&format!("/api/v1/entries/{id}/attachments"), None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
}
