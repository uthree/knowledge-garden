//! Storage abstraction. The HTTP layer depends only on these traits, so a
//! PostgreSQL implementation can be added next to [`sqlite::SqliteStore`].
//!
//! Stores do not make permission decisions on single records; callers check
//! with [`crate::policy`] first. Listing methods take a [`VisibilityFilter`].

pub mod sqlite;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::Result;
use crate::model::*;
use crate::policy::VisibilityFilter;

pub struct NewUser {
    pub username: String,
    pub display_name: String,
    pub role: Role,
}

pub struct NewToken {
    pub user_id: String,
    pub name: String,
    pub kind: TokenKind,
    pub scopes: Vec<Scope>,
    pub token_hash: String,
}

pub struct NewSession {
    pub id_hash: String,
    pub user_id: String,
    pub token_id: String,
    pub csrf: String,
    pub expires_at: DateTime<Utc>,
}

/// A signed-in browser session.
pub struct Session {
    pub principal: Principal,
    pub csrf: String,
}

pub struct NewInvite {
    pub code_hash: String,
    pub role: Role,
    pub created_by: String,
    pub note: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Validated content of one entry version.
pub struct VersionContent {
    pub title: String,
    pub summary: Option<String>,
    pub body: String,
    pub lang: Option<String>,
    pub provenance: serde_json::Value,
}

pub struct NewEntry {
    pub id: String,
    pub uri: String,
    pub origin: String,
    pub kind: EntryKind,
    pub visibility: Visibility,
    pub status: Status,
    pub author_id: String,
    pub token_id: String,
    pub tags: Vec<String>,
    pub content: VersionContent,
}

#[derive(Default)]
pub struct EntryMetaUpdate {
    pub visibility: Option<Visibility>,
    pub status: Option<Status>,
    pub tags: Option<Vec<String>>,
}

pub struct NewLink {
    pub source_id: String,
    pub target_uri: String,
    pub target_id: Option<String>,
    pub target_version: Option<i64>,
    pub relation: Relation,
    pub note: Option<String>,
    pub created_by: String,
}

pub struct NewComment {
    pub entry_id: String,
    pub parent_id: Option<String>,
    pub author_id: String,
    pub token_id: String,
    pub kind: CommentKind,
    pub body: String,
    pub result: Option<ReproResult>,
    pub environment: Option<serde_json::Value>,
    pub entry_version: Option<i64>,
}

pub struct NewAttachment {
    pub entry_id: String,
    pub name: String,
    pub content_type: String,
    pub size: i64,
    pub sha256: String,
    pub token_id: String,
}

/// An attachment listed in a peer's feed item (its bytes are fetched separately).
#[derive(Debug, Clone)]
pub struct MirrorAttachment {
    pub name: String,
    pub content_type: String,
    pub size: i64,
    pub sha256: String,
}

#[derive(Default)]
pub struct EntryFilter {
    pub kind: Option<EntryKind>,
    pub status: Option<Status>,
    pub author: Option<String>,
    /// All of these tags must be present.
    pub tags_all: Vec<String>,
    /// At least one of these tags must be present (ignored when empty).
    pub tags_any: Vec<String>,
    /// Only entries with at least one reproduction report with this result.
    pub has_reproduction: Option<ReproResult>,
    /// `local`, `remote` (mirrored from other Gardens), or a Garden's base URL.
    pub origin: Option<String>,
    /// Only entries created at or after this time.
    pub since: Option<DateTime<Utc>>,
    /// Only entries created before this time.
    pub until: Option<DateTime<Utc>>,
    pub limit: i64,
    pub offset: i64,
}

/// Which fields a regular expression is matched against.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegexField {
    Title,
    Body,
    /// Title, summary and body.
    #[default]
    All,
}

/// A validated search. All conditions are ANDed.
#[derive(Default)]
pub struct SearchQuery {
    /// Terms matched through the trigram index (at least 3 characters each).
    pub indexed_terms: Vec<String>,
    /// Terms matched by substring (shorter than 3 characters).
    pub short_terms: Vec<String>,
    /// Entries containing any of these are excluded.
    pub exclude: Vec<String>,
    pub regex: Option<String>,
    pub regex_field: RegexField,
    pub filter: EntryFilter,
}

pub struct SearchRow {
    pub entry: EntrySummary,
    /// Relevance (higher is better); present when indexed terms were used.
    pub score: Option<f64>,
    pub body: String,
}

#[async_trait]
pub trait AccountStore: Send + Sync {
    async fn create_user(&self, user: NewUser) -> Result<User>;
    async fn find_user(&self, username: &str) -> Result<Option<User>>;
    async fn create_token(&self, token: NewToken) -> Result<Token>;
    async fn list_tokens(&self, user_id: &str) -> Result<Vec<Token>>;
    /// Returns false when the token does not exist or belongs to another user.
    async fn revoke_token(&self, user_id: &str, token_id: &str) -> Result<bool>;
    /// Looks up an unrevoked token by hash and records its use.
    async fn authenticate(&self, token_hash: &str) -> Result<Option<Principal>>;
    async fn create_invite(&self, invite: NewInvite) -> Result<Invite>;
    async fn list_invites(&self) -> Result<Vec<Invite>>;
    /// Atomically consumes an invite and creates the user with the invite's role.
    async fn redeem_invite(&self, code_hash: &str, username: &str, display_name: &str) -> Result<User>;

    /// Sets (or with `None` clears) a local user's password hash.
    async fn set_password(&self, user_id: &str, password_hash: Option<&str>) -> Result<()>;
    /// A local user and their password hash, if they have one.
    async fn password_login(&self, username: &str) -> Result<Option<(User, String)>>;
    /// The user's unrevoked human token named `web` (or `web-2`, ...), which
    /// browser sessions act through; created with `scopes` when missing.
    async fn web_token(&self, user_id: &str, scopes: &[Scope]) -> Result<Token>;
    async fn create_session(&self, session: NewSession) -> Result<()>;
    /// An unexpired session whose token is not revoked.
    async fn session(&self, id_hash: &str) -> Result<Option<Session>>;
    async fn delete_session(&self, id_hash: &str) -> Result<()>;
    async fn delete_user_sessions(&self, user_id: &str) -> Result<()>;
}

#[async_trait]
pub trait EntryStore: Send + Sync {
    async fn create_entry(&self, entry: NewEntry) -> Result<()>;
    async fn entry_access(&self, id: &str) -> Result<Option<EntryAccess>>;
    async fn entry_id_by_uri(&self, uri: &str) -> Result<Option<String>>;
    /// `version: None` means the current version.
    async fn get_entry(&self, id: &str, version: Option<i64>) -> Result<Option<Entry>>;
    async fn list_entries(&self, filter: &EntryFilter, vis: &VisibilityFilter) -> Result<Page<EntrySummary>>;
    async fn list_versions(&self, id: &str) -> Result<Vec<VersionInfo>>;
    /// Appends a new immutable version and returns its number.
    async fn add_version(&self, id: &str, token_id: &str, content: VersionContent) -> Result<i64>;
    async fn update_entry_meta(&self, id: &str, update: EntryMetaUpdate) -> Result<()>;
    async fn add_link(&self, link: NewLink) -> Result<String>;
    async fn links(&self, id: &str, vis: &VisibilityFilter) -> Result<Links>;
    async fn tag_counts(&self, vis: &VisibilityFilter, limit: i64) -> Result<Vec<TagCount>>;
}

#[async_trait]
pub trait CommentStore: Send + Sync {
    async fn add_comment(&self, comment: NewComment) -> Result<Comment>;
    async fn get_comment(&self, id: &str) -> Result<Option<Comment>>;
    async fn list_comments(&self, entry_id: &str) -> Result<Vec<Comment>>;
}

#[async_trait]
pub trait AttachmentStore: Send + Sync {
    /// Adds a file to a local entry and marks the entry as changed. With
    /// `replace`, a file of the same name is replaced; its sha256 is returned.
    async fn add_attachment(&self, a: NewAttachment, replace: bool) -> Result<(Attachment, Option<String>)>;
    async fn list_attachments(&self, entry_id: &str) -> Result<Vec<Attachment>>;
    async fn get_attachment(&self, entry_id: &str, name: &str) -> Result<Option<Attachment>>;
    /// Removes a file and marks the entry as changed. Returns the file's sha256.
    async fn delete_attachment(&self, entry_id: &str, name: &str) -> Result<Option<String>>;
    /// Whether any attachment still uses this blob.
    async fn blob_in_use(&self, sha256: &str) -> Result<bool>;
    /// Whether a local entry that is shared with peers (public or federated) attaches this blob.
    async fn blob_shared(&self, sha256: &str) -> Result<bool>;
}

#[async_trait]
pub trait SearchStore: Send + Sync {
    /// Searches the current version of entries visible under `vis`.
    async fn search(&self, query: &SearchQuery, vis: &VisibilityFilter) -> Result<Page<SearchRow>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerStatus {
    /// We serve our feed to this Garden and mirror theirs.
    Approved,
    /// Neither; their mirrored entries are kept but no longer updated.
    Blocked,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Peer {
    pub id: String,
    pub base_url: String,
    pub name: String,
    pub public_key: String,
    pub status: PeerStatus,
    #[serde(skip)]
    pub cursor: Option<String>,
    pub last_sync_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A local entry that changed, for the federation feed.
pub struct FeedRow {
    pub id: String,
    pub uri: String,
    /// The stored `updated_at` string, used verbatim in feed cursors.
    pub updated_at: String,
    pub visibility: Visibility,
}

pub struct RawLink {
    pub relation: Relation,
    pub target_uri: String,
    pub target_version: Option<i64>,
    pub note: Option<String>,
}

/// A verified entry from a peer's feed, ready to be stored as a mirror.
pub struct MirrorEntry {
    pub origin: String,
    pub uri: String,
    pub signature: String,
    pub updated_at: DateTime<Utc>,
    pub kind: EntryKind,
    /// Already mapped to the local visibility (`public` or `members`).
    pub visibility: Visibility,
    pub status: Status,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub version_created_at: DateTime<Utc>,
    /// Username qualified with the origin host, e.g. `alice@garden.example`.
    pub author_username: String,
    pub author_display_name: String,
    pub posted_via: PostedVia,
    pub tags: Vec<String>,
    pub content: VersionContent,
    pub links: Vec<RawLink>,
    /// Replaces the mirror's attachments; their blobs must already be stored.
    pub attachments: Vec<MirrorAttachment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MirrorOutcome {
    Created,
    Updated,
}

#[async_trait]
pub trait FederationStore: Send + Sync {
    /// Returns this Garden's 32-byte signing key seed, creating it on first use.
    async fn garden_key_seed(&self) -> Result<[u8; 32]>;
    async fn add_peer(&self, base_url: &str, name: &str, public_key: &str) -> Result<Peer>;
    async fn list_peers(&self) -> Result<Vec<Peer>>;
    async fn get_peer(&self, id: &str) -> Result<Option<Peer>>;
    async fn find_peer(&self, base_url: &str) -> Result<Option<Peer>>;
    async fn set_peer_status(&self, id: &str, status: PeerStatus) -> Result<bool>;
    async fn record_sync(&self, id: &str, cursor: Option<&str>, error: Option<&str>) -> Result<()>;
    /// Local entries that were ever shareable and changed after `after` (`(updated_at, id)`).
    async fn feed_rows(&self, after: Option<(&str, &str)>, limit: i64) -> Result<Vec<FeedRow>>;
    async fn raw_links(&self, entry_id: &str) -> Result<Vec<RawLink>>;
    async fn apply_mirror(&self, entry: MirrorEntry) -> Result<MirrorOutcome>;
    /// Hides a mirrored entry that its origin no longer shares and drops its
    /// attachments. Returns false if unknown.
    async fn withdraw_mirror(&self, origin: &str, uri: &str) -> Result<bool>;
}

pub trait Store: AccountStore + EntryStore + CommentStore + SearchStore + FederationStore + AttachmentStore {}
impl<T: AccountStore + EntryStore + CommentStore + SearchStore + FederationStore + AttachmentStore> Store for T {}
