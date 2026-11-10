#![allow(dead_code)]
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

pub const BASE: &str = "http://garden.test";

pub struct Garden {
    pub app: Router,
    pub admin: String,
}

impl Garden {
    pub async fn new() -> Self {
        Self::with_config(Config::new(BASE, "test")).await
    }

    pub async fn with_config(config: Config) -> Self {
        Self::with_peer_client(config, None).await
    }

    /// Builds a Garden whose outgoing requests to other Gardens go through `client`.
    pub async fn with_peer_client(
        mut config: Config,
        client: Option<std::sync::Arc<dyn knowledge_garden::federation::PeerClient>>,
    ) -> Self {
        config.files_dir = std::env::temp_dir().join(format!("kg-test-files-{}", uuid::Uuid::now_v7()));
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
        let mut state = AppState::new(store, config);
        if let Some(client) = client {
            state.peer_client = client;
        }
        Self {
            app: router(state),
            admin: secret,
        }
    }

    pub async fn call(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
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

    /// Sends a raw body (a file upload) and returns the response with its body bytes.
    pub async fn raw(
        &self,
        method: Method,
        uri: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let mut req = Request::builder().method(method).uri(uri);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let resp = self
            .app
            .clone()
            .oneshot(req.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes().to_vec();
        (status, headers, bytes)
    }

    /// Uploads `bytes` as attachment `name` of entry `id`.
    pub async fn upload(&self, token: &str, id: &str, name: &str, bytes: &[u8]) -> (StatusCode, Value) {
        let auth = format!("Bearer {token}");
        let (s, _, body) = self
            .raw(
                Method::POST,
                &format!("/api/v1/entries/{id}/attachments?name={name}"),
                &[("authorization", &auth)],
                bytes.to_vec(),
            )
            .await;
        (s, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    pub async fn get(&self, uri: &str, token: Option<&str>) -> (StatusCode, Value) {
        self.call(Method::GET, uri, token, None).await
    }

    pub async fn post(&self, uri: &str, token: Option<&str>, body: Value) -> (StatusCode, Value) {
        self.call(Method::POST, uri, token, Some(body)).await
    }

    /// Invites and registers a member; returns their human token.
    pub async fn member(&self, username: &str) -> String {
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

    pub async fn agent_token(&self, human: &str, name: &str) -> String {
        let (s, t) = self.post("/api/v1/tokens", Some(human), json!({ "name": name })).await;
        assert_eq!(s, StatusCode::CREATED, "{t}");
        t["secret"].as_str().unwrap().to_string()
    }

    pub async fn entry(&self, token: &str, body: Value) -> Value {
        let (s, e) = self.post("/api/v1/entries", Some(token), body).await;
        assert_eq!(s, StatusCode::CREATED, "{e}");
        e
    }
}
