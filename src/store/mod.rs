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

#[derive(Default)]
pub struct EntryFilter {
    pub kind: Option<EntryKind>,
    pub status: Option<Status>,
    pub author: Option<String>,
    /// All of these tags must be present.
    pub tags_all: Vec<String>,
    /// At least one of these tags must be present (ignored when empty).
    pub tags_any: Vec<String>,
    pub limit: i64,
    pub offset: i64,
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

pub trait Store: AccountStore + EntryStore + CommentStore {}
impl<T: AccountStore + EntryStore + CommentStore> Store for T {}
