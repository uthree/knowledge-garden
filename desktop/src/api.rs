//! A client for a Garden's REST API (`/api/v1`), local or remote.
//!
//! Every read and write goes through the API, also for the Garden this app runs itself, so the
//! permission rules and input checks stay in one place: the server.

use std::time::Duration;

use knowledge_garden::model::{
    Comment, CommentKind, Entry, EntryKind, EntrySummary, Links, Page, ReproResult, Status, TagCount, Token, User,
    VersionInfo, Visibility,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// What went wrong talking to a Garden, in words for the person.
#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    /// The Garden could not be reached at all.
    Unreachable(String),
    /// The Garden answered with an error.
    Status { status: u16, code: String, message: String },
    /// The answer was not what the API promises.
    Invalid(String),
}

impl ApiError {
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, ApiError::Status { status: 401, .. })
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Unreachable(e) => write!(f, "Garden に接続できません（{e}）"),
            ApiError::Status { status: 401, .. } => {
                f.write_str("ログインが必要です。トークンが失効しているかもしれません")
            }
            ApiError::Status { status: 404, .. } => f.write_str("見つかりません（削除されたか、読む権限がありません）"),
            ApiError::Status {
                status: 429, message, ..
            } => write!(f, "回数の上限に達しました: {message}"),
            ApiError::Status { message, .. } => f.write_str(message),
            ApiError::Invalid(e) => write!(f, "Garden の応答を読めませんでした（{e}）"),
        }
    }
}

impl std::error::Error for ApiError {}

pub type ApiResult<T> = Result<T, ApiError>;

/// Settings of a Garden (`GET /api/v1/garden`).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct GardenInfo {
    pub name: String,
    pub base_url: String,
    pub guest_read: bool,
    pub registration: String,
    pub default_visibility: Visibility,
}

/// The caller (`GET /api/v1/me`).
#[derive(Debug, Clone, Deserialize)]
pub struct Me {
    pub user: User,
    pub token: Token,
}

/// A user and the secret of a token issued to them.
#[derive(Debug, Clone, Deserialize)]
pub struct Issued {
    pub user: User,
    pub secret: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IssuedToken {
    pub token: Token,
    pub secret: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IssuedInvite {
    pub code: String,
}

/// A written entry and the server's advice about it (missing recommended sections, provenance).
#[derive(Debug, Clone, Deserialize)]
pub struct Written {
    pub entry: Entry,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchHit {
    pub entry: EntrySummary,
    /// An excerpt of the body around the first match.
    pub snippet: Option<String>,
}

/// Filters shared by the list and the search.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Filter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<EntryKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Comma-separated; all must match.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// `local`, `remote` or a Garden's base URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
}

/// A search: terms with `"phrases"` and `-exclusions`, or a regular expression.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SearchQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
    #[serde(flatten)]
    pub filter: Filter,
}

/// A new entry (`POST /api/v1/entries`).
#[derive(Debug, Clone, Serialize)]
pub struct NewEntry {
    pub kind: EntryKind,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub body: String,
    pub tags: Vec<String>,
    pub visibility: Visibility,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
}

/// A new comment or reproduction report.
#[derive(Debug, Clone, Serialize)]
pub struct NewComment {
    pub kind: CommentKind,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ReproResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<Value>,
}

/// A connection to one Garden, with or without a token.
#[derive(Clone)]
pub struct Client {
    base_url: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.base_url)
            .field("token", &self.token.as_ref().map(|_| "…"))
            .finish()
    }
}

/// Normalises what the person typed as a Garden's address: a scheme is added when missing
/// (`https://`, or `http://` for localhost and `.local` names), and the trailing slash dropped.
pub fn normalize_base_url(input: &str) -> Result<String, String> {
    let input = input.trim().trim_end_matches('/');
    if input.is_empty() {
        return Err("URL を入力してください".into());
    }
    let with_scheme = if input.contains("://") {
        input.to_string()
    } else {
        let host = input.split(['/', ':']).next().unwrap_or("");
        let local = host == "localhost" || host.ends_with(".local") || host.parse::<std::net::IpAddr>().is_ok();
        format!("{}://{input}", if local { "http" } else { "https" })
    };
    let url = url::Url::parse(&with_scheme).map_err(|e| format!("URL として読めません: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("http:// か https:// で始まる URL を入力してください".into());
    }
    Ok(with_scheme)
}

impl Client {
    pub fn new(base_url: &str, token: Option<String>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("knowledge-garden-desktop/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("an HTTP client");
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            http,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    pub fn with_token(&self, token: Option<String>) -> Self {
        Self { token, ..self.clone() }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/v1{path}", self.base_url)
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut req = self
            .http
            .request(method, self.url(path))
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
        req
    }

    async fn send<T: DeserializeOwned>(&self, req: reqwest::RequestBuilder) -> ApiResult<T> {
        let resp = req.send().await.map_err(|e| ApiError::Unreachable(short_error(&e)))?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| ApiError::Unreachable(short_error(&e)))?;
        if !status.is_success() {
            let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            return Err(ApiError::Status {
                status: status.as_u16(),
                code: body["error"]["code"].as_str().unwrap_or("").to_string(),
                message: body["error"]["message"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("HTTP {status}")),
            });
        }
        serde_json::from_slice(&bytes).map_err(|e| ApiError::Invalid(e.to_string()))
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> ApiResult<T> {
        self.send(self.request(reqwest::Method::GET, path)).await
    }

    async fn get_query<T: DeserializeOwned>(&self, path: &str, query: &impl Serialize) -> ApiResult<T> {
        self.send(self.request(reqwest::Method::GET, path).query(query)).await
    }

    async fn post<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> ApiResult<T> {
        self.send(self.request(reqwest::Method::POST, path).json(body)).await
    }

    pub async fn garden(&self) -> ApiResult<GardenInfo> {
        self.get("/garden").await
    }

    pub async fn me(&self) -> ApiResult<Me> {
        self.get("/me").await
    }

    /// Signs in with a password and receives a human token named `desktop`.
    pub async fn login(&self, username: &str, password: &str) -> ApiResult<Issued> {
        self.post(
            "/login",
            &json!({ "username": username, "password": password, "token_name": "desktop" }),
        )
        .await
    }

    pub async fn entries(&self, filter: &Filter) -> ApiResult<Page<EntrySummary>> {
        self.get_query("/entries", filter).await
    }

    pub async fn search(&self, query: &SearchQuery) -> ApiResult<Page<SearchHit>> {
        self.get_query("/search", query).await
    }

    pub async fn entry(&self, id: &str, version: Option<i64>) -> ApiResult<Entry> {
        match version {
            Some(v) => self.get(&format!("/entries/{id}/versions/{v}")).await,
            None => self.get(&format!("/entries/{id}")).await,
        }
    }

    pub async fn versions(&self, id: &str) -> ApiResult<Vec<VersionInfo>> {
        self.get(&format!("/entries/{id}/versions")).await
    }

    pub async fn links(&self, id: &str) -> ApiResult<Links> {
        self.get(&format!("/entries/{id}/links")).await
    }

    pub async fn comments(&self, id: &str) -> ApiResult<Vec<Comment>> {
        self.get(&format!("/entries/{id}/comments")).await
    }

    pub async fn tags(&self) -> ApiResult<Vec<TagCount>> {
        self.get("/tags").await
    }

    pub async fn create_entry(&self, entry: &NewEntry) -> ApiResult<Written> {
        self.post("/entries", entry).await
    }

    /// Adds a version with the new content.
    pub async fn revise(&self, id: &str, title: &str, summary: Option<&str>, body: &str) -> ApiResult<Written> {
        self.post(
            &format!("/entries/{id}/versions"),
            &json!({ "title": title, "summary": summary, "body": body }),
        )
        .await
    }

    /// Changes visibility, status and tags, which are not part of a version.
    pub async fn patch(
        &self,
        id: &str,
        visibility: Option<Visibility>,
        status: Option<Status>,
        tags: Option<&[String]>,
    ) -> ApiResult<Entry> {
        let mut body = serde_json::Map::new();
        if let Some(v) = visibility {
            body.insert("visibility".into(), json!(v));
        }
        if let Some(s) = status {
            body.insert("status".into(), json!(s));
        }
        if let Some(t) = tags {
            body.insert("tags".into(), json!(t));
        }
        self.send(
            self.request(reqwest::Method::PATCH, &format!("/entries/{id}"))
                .json(&Value::Object(body)),
        )
        .await
    }

    pub async fn comment(&self, id: &str, comment: &NewComment) -> ApiResult<Comment> {
        self.post(&format!("/entries/{id}/comments"), comment).await
    }

    pub async fn tokens(&self) -> ApiResult<Vec<Token>> {
        self.get("/tokens").await
    }

    /// Issues an agent token with the given scopes (`read`, `entries:write`, `comments:write`).
    pub async fn create_agent_token(&self, name: &str, scopes: &[&str]) -> ApiResult<IssuedToken> {
        self.post("/tokens", &json!({ "name": name, "kind": "agent", "scopes": scopes }))
            .await
    }

    pub async fn revoke_token(&self, id: &str) -> ApiResult<()> {
        let resp = self
            .request(reqwest::Method::DELETE, &format!("/tokens/{id}"))
            .send()
            .await
            .map_err(|e| ApiError::Unreachable(short_error(&e)))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(ApiError::Status {
                status: resp.status().as_u16(),
                code: String::new(),
                message: format!("HTTP {}", resp.status()),
            })
        }
    }

    pub async fn create_invite(&self, role: &str, note: Option<&str>) -> ApiResult<IssuedInvite> {
        self.post("/invites", &json!({ "role": role, "note": note })).await
    }
}

/// The innermost cause of a request error, without the URL (which the screen already shows).
fn short_error(e: &reqwest::Error) -> String {
    let mut source: &dyn std::error::Error = e;
    while let Some(next) = source.source() {
        source = next;
    }
    if e.is_timeout() {
        "時間切れ".into()
    } else {
        source.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use knowledge_garden::store::AccountStore;

    #[test]
    fn base_urls_get_a_scheme_and_lose_the_trailing_slash() {
        assert_eq!(normalize_base_url("garden.example/").unwrap(), "https://garden.example");
        assert_eq!(normalize_base_url("nas.local:8080").unwrap(), "http://nas.local:8080");
        assert_eq!(normalize_base_url("localhost:8787").unwrap(), "http://localhost:8787");
        assert_eq!(
            normalize_base_url("192.168.1.20:8080").unwrap(),
            "http://192.168.1.20:8080"
        );
        assert_eq!(
            normalize_base_url(" http://garden.example/kg/ ").unwrap(),
            "http://garden.example/kg"
        );
        assert!(normalize_base_url("").is_err());
        assert!(normalize_base_url("ftp://garden.example").is_err());
    }

    #[test]
    fn search_queries_flatten_their_filters() {
        let q = SearchQuery {
            q: Some("sqlx -python".into()),
            regex: None,
            filter: Filter {
                kind: Some(EntryKind::Troubleshooting),
                tag: Some("rust".into()),
                ..Default::default()
            },
        };
        let v = serde_json::to_value(&q).unwrap();
        assert_eq!(
            v,
            json!({ "q": "sqlx -python", "kind": "troubleshooting", "tag": "rust" })
        );
    }

    /// A Garden on a free local port, with an admin `uthree` whose password is
    /// `a long password`. Returns its base URL and the admin's token.
    async fn garden() -> (String, String) {
        let store = knowledge_garden::store::sqlite::SqliteStore::connect("sqlite::memory:")
            .await
            .unwrap();
        let (user, secret) = knowledge_garden::server::bootstrap_admin(&store, "uthree", "uthree", "default")
            .await
            .unwrap();
        let hash = knowledge_garden::auth::hash_password("a long password".into())
            .await
            .unwrap();
        store.set_password(&user.id, Some(&hash)).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = knowledge_garden::AppState::new(store, knowledge_garden::config::Config::new(&base, "Test"));
        tokio::spawn(knowledge_garden::server::serve(
            state,
            listener,
            None,
            std::future::pending(),
        ));
        (base, secret)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_client_reads_and_writes_a_real_garden() {
        let (base, _) = garden().await;
        let guest = Client::new(&base, None);
        assert_eq!(guest.garden().await.unwrap().name, "Test");
        assert!(
            guest
                .login("uthree", "wrong password")
                .await
                .unwrap_err()
                .is_unauthorized()
        );

        let issued = guest.login("uthree", "a long password").await.unwrap();
        let me = guest.with_token(Some(issued.secret.clone()));
        assert_eq!(me.me().await.unwrap().user.username, "uthree");

        let written = me
            .create_entry(&NewEntry {
                kind: EntryKind::Troubleshooting,
                title: "QueryBuilder の型引数".into(),
                summary: None,
                body: "# 症状\n型エラー".into(),
                tags: vec!["rust".into(), "sqlx".into()],
                visibility: Visibility::Public,
                status: None,
            })
            .await
            .unwrap();
        assert!(!written.warnings.is_empty(), "missing sections are reported");
        let id = written.entry.id.clone();

        let found = guest
            .search(&SearchQuery {
                q: Some("QueryBuilder".into()),
                regex: None,
                filter: Filter {
                    kind: Some(EntryKind::Troubleshooting),
                    tag: Some("rust".into()),
                    ..Default::default()
                },
            })
            .await
            .unwrap();
        assert_eq!(found.total, 1);
        let listed = guest
            .entries(&Filter {
                kind: Some(EntryKind::Memo),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(listed.total, 0, "filters reach the server");

        let revised = me
            .revise(&id, "QueryBuilder の型引数が必須に", None, "# 症状\n型エラー")
            .await
            .unwrap();
        assert_eq!(revised.entry.version, 2);
        let patched = me.patch(&id, None, Some(Status::Verified), None).await.unwrap();
        assert_eq!(patched.status, Status::Verified);
        assert_eq!(me.versions(&id).await.unwrap().len(), 2);
        assert_eq!(guest.entry(&id, Some(1)).await.unwrap().title, "QueryBuilder の型引数");

        me.comment(
            &id,
            &NewComment {
                kind: CommentKind::Reproduction,
                body: "直った".into(),
                parent_id: None,
                result: Some(ReproResult::Reproduced),
                environment: Some(json!({ "os": "linux" })),
            },
        )
        .await
        .unwrap();
        assert_eq!(guest.comments(&id).await.unwrap().len(), 1);
        assert_eq!(guest.entry(&id, None).await.unwrap().reproductions.reproduced, 1);
        assert_eq!(guest.tags().await.unwrap().len(), 2);

        let key = me.create_agent_token("claude-code", &["read"]).await.unwrap();
        let agent = guest.with_token(Some(key.secret));
        assert!(agent.me().await.is_ok());
        me.revoke_token(&key.token.id).await.unwrap();
        assert!(agent.me().await.unwrap_err().is_unauthorized());
        assert!(
            me.create_invite("member", None)
                .await
                .unwrap()
                .code
                .starts_with("kginv_")
        );
    }
}
