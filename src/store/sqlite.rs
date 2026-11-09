use std::str::FromStr;

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use super::*;
use crate::error::Error;

pub struct SqliteStore {
    pool: SqlitePool,
}

/// `tokens.last_used_at` is refreshed at most this often.
const LAST_USED_PRECISION_SECS: i64 = 60;

fn now() -> String {
    ts(Utc::now())
}

fn ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

fn parse<T: FromStr<Err = String>>(s: &str) -> Result<T> {
    s.parse().map_err(Error::Internal)
}

fn parse_json(s: &str) -> Result<serde_json::Value> {
    serde_json::from_str(s).map_err(Error::internal)
}

fn parse_tags(s: &str) -> Result<Vec<String>> {
    serde_json::from_str(s).map_err(Error::internal)
}

impl SqliteStore {
    /// Opens (creating if needed) the database at `url`, e.g. `sqlite://garden.db`
    /// or `sqlite::memory:`, and applies migrations.
    pub async fn connect(url: &str) -> Result<Self> {
        let opts = SqliteConnectOptions::from_str(url)?
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            // Safe with WAL (a power loss can only drop the latest commits) and
            // avoids an fsync on every commit.
            .synchronous(SqliteSynchronous::Normal)
            .with_regexp();
        // An in-memory database exists per connection, so it must use exactly one.
        let max = if url.contains(":memory:") { 1 } else { 8 };
        let pool = SqlitePoolOptions::new().max_connections(max).connect_with(opts).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }
}

// ---------------------------------------------------------------- rows

#[derive(sqlx::FromRow)]
struct UserRow {
    id: String,
    username: String,
    display_name: String,
    role: String,
    created_at: DateTime<Utc>,
}

impl UserRow {
    fn into_user(self) -> Result<User> {
        Ok(User {
            id: self.id,
            username: self.username,
            display_name: self.display_name,
            role: parse(&self.role)?,
            created_at: self.created_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct TokenRow {
    id: String,
    user_id: String,
    name: String,
    kind: String,
    scopes: String,
    created_at: DateTime<Utc>,
    last_used_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
}

impl TokenRow {
    fn into_token(self) -> Result<Token> {
        let scopes: Vec<String> = serde_json::from_str(&self.scopes).map_err(Error::internal)?;
        Ok(Token {
            id: self.id,
            user_id: self.user_id,
            name: self.name,
            kind: parse(&self.kind)?,
            scopes: scopes.iter().map(|s| parse(s)).collect::<Result<_>>()?,
            created_at: self.created_at,
            last_used_at: self.last_used_at,
            revoked_at: self.revoked_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct InviteRow {
    id: String,
    role: String,
    created_by: String,
    note: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    used_by: Option<String>,
    used_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

impl InviteRow {
    fn into_invite(self) -> Result<Invite> {
        Ok(Invite {
            id: self.id,
            role: parse(&self.role)?,
            created_by: self.created_by,
            note: self.note,
            expires_at: self.expires_at,
            used_by: self.used_by,
            used_at: self.used_at,
            created_at: self.created_at,
        })
    }
}

/// Columns shared by entry detail and summary queries.
const ENTRY_COLUMNS: &str = "
    e.id, e.uri, e.origin, e.is_local, e.kind, e.visibility, e.status, e.author_id,
    u.username AS author_username, u.display_name AS author_display_name,
    e.current_version, e.created_at, e.updated_at,
    v.version, v.title, v.summary, v.created_at AS version_created_at,
    t.name AS token_name, t.kind AS token_kind,
    (SELECT json_group_array(tag) FROM (SELECT tag FROM entry_tags WHERE entry_id = e.id ORDER BY tag)) AS tags,
    (SELECT count(*) FROM comments c WHERE c.entry_id = e.id AND c.result = 'reproduced') AS repro_reproduced,
    (SELECT count(*) FROM comments c WHERE c.entry_id = e.id AND c.result = 'partial') AS repro_partial,
    (SELECT count(*) FROM comments c WHERE c.entry_id = e.id AND c.result = 'failed') AS repro_failed";

const ENTRY_JOINS: &str = "
    FROM entries e
    JOIN users u ON u.id = e.author_id
    JOIN entry_versions v ON v.entry_id = e.id AND v.version = e.current_version
    JOIN tokens t ON t.id = v.token_id";

#[derive(sqlx::FromRow)]
struct EntryRow {
    id: String,
    uri: String,
    origin: String,
    is_local: i64,
    kind: String,
    visibility: String,
    status: String,
    author_id: String,
    author_username: String,
    author_display_name: String,
    current_version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    version: i64,
    title: String,
    summary: Option<String>,
    version_created_at: DateTime<Utc>,
    token_name: String,
    token_kind: String,
    tags: String,
    repro_reproduced: i64,
    repro_partial: i64,
    repro_failed: i64,
}

#[derive(sqlx::FromRow)]
struct EntryDetailRow {
    #[sqlx(flatten)]
    base: EntryRow,
    body: String,
    lang: Option<String>,
    provenance: String,
}

impl EntryRow {
    fn author(&self) -> AuthorRef {
        AuthorRef {
            username: self.author_username.clone(),
            display_name: self.author_display_name.clone(),
        }
    }
    fn posted_via(&self) -> Result<PostedVia> {
        Ok(PostedVia {
            name: self.token_name.clone(),
            kind: parse(&self.token_kind)?,
        })
    }
    fn repro(&self) -> ReproStats {
        ReproStats {
            reproduced: self.repro_reproduced,
            partial: self.repro_partial,
            failed: self.repro_failed,
        }
    }

    fn into_summary(self) -> Result<EntrySummary> {
        Ok(EntrySummary {
            author: self.author(),
            posted_via: self.posted_via()?,
            reproductions: self.repro(),
            tags: parse_tags(&self.tags)?,
            kind: parse(&self.kind)?,
            visibility: parse(&self.visibility)?,
            status: parse(&self.status)?,
            id: self.id,
            uri: self.uri,
            current_version: self.current_version,
            title: self.title,
            summary: self.summary,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

impl EntryDetailRow {
    fn into_entry(self) -> Result<Entry> {
        let b = self.base;
        Ok(Entry {
            author: b.author(),
            posted_via: b.posted_via()?,
            reproductions: b.repro(),
            tags: parse_tags(&b.tags)?,
            kind: parse(&b.kind)?,
            visibility: parse(&b.visibility)?,
            status: parse(&b.status)?,
            provenance: parse_json(&self.provenance)?,
            id: b.id,
            uri: b.uri,
            origin: b.origin,
            is_local: b.is_local != 0,
            author_id: b.author_id,
            version: b.version,
            current_version: b.current_version,
            title: b.title,
            summary: b.summary,
            body: self.body,
            lang: self.lang,
            created_at: b.created_at,
            updated_at: b.updated_at,
            version_created_at: b.version_created_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct CommentRow {
    id: String,
    entry_id: String,
    parent_id: Option<String>,
    author_username: String,
    author_display_name: String,
    token_name: String,
    token_kind: String,
    kind: String,
    body: String,
    result: Option<String>,
    environment: Option<String>,
    entry_version: Option<i64>,
    created_at: DateTime<Utc>,
}

const COMMENT_SELECT: &str = "
    SELECT c.id, c.entry_id, c.parent_id,
           u.username AS author_username, u.display_name AS author_display_name,
           t.name AS token_name, t.kind AS token_kind,
           c.kind, c.body, c.result, c.environment, c.entry_version, c.created_at
    FROM comments c
    JOIN users u ON u.id = c.author_id
    JOIN tokens t ON t.id = c.token_id";

impl CommentRow {
    fn into_comment(self) -> Result<Comment> {
        Ok(Comment {
            id: self.id,
            entry_id: self.entry_id,
            parent_id: self.parent_id,
            author: AuthorRef {
                username: self.author_username,
                display_name: self.author_display_name,
            },
            posted_via: PostedVia {
                name: self.token_name,
                kind: parse(&self.token_kind)?,
            },
            kind: parse(&self.kind)?,
            body: self.body,
            result: self.result.as_deref().map(parse).transpose()?,
            environment: self.environment.as_deref().map(parse_json).transpose()?,
            entry_version: self.entry_version,
            created_at: self.created_at,
        })
    }
}

/// Pushes `(alias.author_id = ? OR alias.visibility IN (...))`.
fn push_visibility(qb: &mut QueryBuilder<Sqlite>, alias: &str, vis: &VisibilityFilter) {
    qb.push("(");
    let mut any = false;
    if let Some(viewer) = &vis.viewer_id {
        qb.push(format!("{alias}.author_id = ")).push_bind(viewer.clone());
        any = true;
    }
    if !vis.visible.is_empty() {
        if any {
            qb.push(" OR ");
        }
        qb.push(format!("{alias}.visibility IN ("));
        let mut sep = qb.separated(", ");
        for v in &vis.visible {
            sep.push_bind(v.as_str());
        }
        sep.push_unseparated(")");
        any = true;
    }
    if !any {
        qb.push("0");
    }
    qb.push(")");
}

fn push_entry_filter(qb: &mut QueryBuilder<Sqlite>, f: &EntryFilter, vis: &VisibilityFilter) {
    qb.push(" WHERE ");
    push_visibility(qb, "e", vis);
    if let Some(kind) = f.kind {
        qb.push(" AND e.kind = ").push_bind(kind.as_str());
    }
    if let Some(status) = f.status {
        qb.push(" AND e.status = ").push_bind(status.as_str());
    }
    if let Some(author) = &f.author {
        qb.push(" AND u.username = ").push_bind(author.clone());
    }
    for tag in &f.tags_all {
        qb.push(" AND EXISTS (SELECT 1 FROM entry_tags et WHERE et.entry_id = e.id AND et.tag = ")
            .push_bind(tag.clone())
            .push(")");
    }
    if !f.tags_any.is_empty() {
        qb.push(" AND EXISTS (SELECT 1 FROM entry_tags et WHERE et.entry_id = e.id AND et.tag IN (");
        let mut sep = qb.separated(", ");
        for tag in &f.tags_any {
            sep.push_bind(tag.clone());
        }
        sep.push_unseparated("))");
    }
    if let Some(result) = f.has_reproduction {
        qb.push(" AND EXISTS (SELECT 1 FROM comments c WHERE c.entry_id = e.id AND c.result = ")
            .push_bind(result.as_str())
            .push(")");
    }
    if let Some(since) = f.since {
        qb.push(" AND e.created_at >= ").push_bind(ts(since));
    }
    if let Some(until) = f.until {
        qb.push(" AND e.created_at < ").push_bind(ts(until));
    }
}

/// Rebuilds the full-text row of an entry from its current version and tags.
async fn reindex(conn: &mut sqlx::SqliteConnection, entry_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM entry_fts WHERE rowid = (SELECT seq FROM entries WHERE id = ?)")
        .bind(entry_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "INSERT INTO entry_fts (rowid, title, summary, body, tags)
         SELECT e.seq, v.title, coalesce(v.summary, ''), v.body,
                coalesce((SELECT group_concat(tag, ' ') FROM (SELECT tag FROM entry_tags WHERE entry_id = e.id ORDER BY tag)), '')
         FROM entries e
         JOIN entry_versions v ON v.entry_id = e.id AND v.version = e.current_version
         WHERE e.id = ?",
    )
    .bind(entry_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn insert_tags(conn: &mut sqlx::SqliteConnection, entry_id: &str, tags: &[String]) -> Result<()> {
    for tag in tags {
        sqlx::query("INSERT INTO entry_tags (entry_id, tag) VALUES (?, ?)")
            .bind(entry_id)
            .bind(tag)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

async fn insert_version(
    conn: &mut sqlx::SqliteConnection,
    entry_id: &str,
    version: i64,
    token_id: &str,
    c: &VersionContent,
    at: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO entry_versions (entry_id, version, title, summary, body, lang, provenance, token_id, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(entry_id)
    .bind(version)
    .bind(&c.title)
    .bind(&c.summary)
    .bind(&c.body)
    .bind(&c.lang)
    .bind(c.provenance.to_string())
    .bind(token_id)
    .bind(at)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------- accounts

#[async_trait]
impl AccountStore for SqliteStore {
    async fn create_user(&self, user: NewUser) -> Result<User> {
        let id = new_id();
        sqlx::query("INSERT INTO users (id, username, display_name, role, created_at) VALUES (?, ?, ?, ?, ?)")
            .bind(&id)
            .bind(&user.username)
            .bind(&user.display_name)
            .bind(user.role.as_str())
            .bind(now())
            .execute(&self.pool)
            .await
            .map_err(|e| match Error::from(e) {
                Error::Conflict(_) => Error::Conflict("username is already taken".into()),
                other => other,
            })?;
        self.find_user(&user.username).await?.ok_or(Error::NotFound)
    }

    async fn find_user(&self, username: &str) -> Result<Option<User>> {
        sqlx::query_as::<_, UserRow>("SELECT * FROM users WHERE username = ?")
            .bind(username)
            .fetch_optional(&self.pool)
            .await?
            .map(UserRow::into_user)
            .transpose()
    }

    async fn create_token(&self, token: NewToken) -> Result<Token> {
        let id = new_id();
        let scopes: Vec<&str> = token.scopes.iter().map(|s| s.as_str()).collect();
        sqlx::query(
            "INSERT INTO tokens (id, user_id, name, kind, scopes, token_hash, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&token.user_id)
        .bind(&token.name)
        .bind(token.kind.as_str())
        .bind(serde_json::to_string(&scopes).map_err(Error::internal)?)
        .bind(&token.token_hash)
        .bind(now())
        .execute(&self.pool)
        .await
        .map_err(|e| match Error::from(e) {
            Error::Conflict(_) => Error::Conflict(format!("a token named '{}' already exists", token.name)),
            other => other,
        })?;
        sqlx::query_as::<_, TokenRow>("SELECT * FROM tokens WHERE id = ?")
            .bind(&id)
            .fetch_one(&self.pool)
            .await?
            .into_token()
    }

    async fn list_tokens(&self, user_id: &str) -> Result<Vec<Token>> {
        sqlx::query_as::<_, TokenRow>("SELECT * FROM tokens WHERE user_id = ? ORDER BY created_at")
            .bind(user_id)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(TokenRow::into_token)
            .collect()
    }

    async fn revoke_token(&self, user_id: &str, token_id: &str) -> Result<bool> {
        let r = sqlx::query("UPDATE tokens SET revoked_at = ? WHERE id = ? AND user_id = ? AND revoked_at IS NULL")
            .bind(now())
            .bind(token_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() == 1)
    }

    async fn authenticate(&self, token_hash: &str) -> Result<Option<Principal>> {
        let Some(token) =
            sqlx::query_as::<_, TokenRow>("SELECT * FROM tokens WHERE token_hash = ? AND revoked_at IS NULL")
                .bind(token_hash)
                .fetch_optional(&self.pool)
                .await?
        else {
            return Ok(None);
        };
        // Recording every use would make each authenticated read take the
        // database write lock; a minute of precision is enough.
        let stale = token
            .last_used_at
            .is_none_or(|t| Utc::now() - t > chrono::Duration::seconds(LAST_USED_PRECISION_SECS));
        if stale {
            sqlx::query("UPDATE tokens SET last_used_at = ? WHERE id = ?")
                .bind(now())
                .bind(&token.id)
                .execute(&self.pool)
                .await?;
        }
        let user = sqlx::query_as::<_, UserRow>("SELECT * FROM users WHERE id = ?")
            .bind(&token.user_id)
            .fetch_one(&self.pool)
            .await?
            .into_user()?;
        Ok(Some(Principal {
            user,
            token: token.into_token()?,
        }))
    }

    async fn create_invite(&self, invite: NewInvite) -> Result<Invite> {
        let id = new_id();
        sqlx::query(
            "INSERT INTO invites (id, code_hash, role, created_by, note, expires_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&invite.code_hash)
        .bind(invite.role.as_str())
        .bind(&invite.created_by)
        .bind(&invite.note)
        .bind(invite.expires_at.map(ts))
        .bind(now())
        .execute(&self.pool)
        .await?;
        sqlx::query_as::<_, InviteRow>("SELECT * FROM invites WHERE id = ?")
            .bind(&id)
            .fetch_one(&self.pool)
            .await?
            .into_invite()
    }

    async fn list_invites(&self) -> Result<Vec<Invite>> {
        sqlx::query_as::<_, InviteRow>("SELECT * FROM invites ORDER BY created_at DESC")
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(InviteRow::into_invite)
            .collect()
    }

    async fn redeem_invite(&self, code_hash: &str, username: &str, display_name: &str) -> Result<User> {
        let mut tx = self.pool.begin().await?;
        let at = now();
        let invite = sqlx::query_as::<_, InviteRow>(
            "SELECT * FROM invites WHERE code_hash = ? AND used_by IS NULL AND (expires_at IS NULL OR expires_at > ?)",
        )
        .bind(code_hash)
        .bind(&at)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| Error::Forbidden("invite code is invalid, used or expired".into()))?;

        let user_id = new_id();
        sqlx::query("INSERT INTO users (id, username, display_name, role, created_at) VALUES (?, ?, ?, ?, ?)")
            .bind(&user_id)
            .bind(username)
            .bind(display_name)
            .bind(&invite.role)
            .bind(&at)
            .execute(&mut *tx)
            .await
            .map_err(|e| match Error::from(e) {
                Error::Conflict(_) => Error::Conflict("username is already taken".into()),
                other => other,
            })?;
        sqlx::query("UPDATE invites SET used_by = ?, used_at = ? WHERE id = ?")
            .bind(&user_id)
            .bind(&at)
            .bind(&invite.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.find_user(username).await?.ok_or(Error::NotFound)
    }
}

// ---------------------------------------------------------------- entries

#[async_trait]
impl EntryStore for SqliteStore {
    async fn create_entry(&self, entry: NewEntry) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let at = now();
        sqlx::query(
            "INSERT INTO entries (id, uri, origin, is_local, kind, visibility, status, author_id, current_version, created_at, updated_at)
             VALUES (?, ?, ?, 1, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(&entry.id)
        .bind(&entry.uri)
        .bind(&entry.origin)
        .bind(entry.kind.as_str())
        .bind(entry.visibility.as_str())
        .bind(entry.status.as_str())
        .bind(&entry.author_id)
        .bind(&at)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
        insert_version(&mut tx, &entry.id, 1, &entry.token_id, &entry.content, &at).await?;
        insert_tags(&mut tx, &entry.id, &entry.tags).await?;
        reindex(&mut tx, &entry.id).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn entry_access(&self, id: &str) -> Result<Option<EntryAccess>> {
        let row: Option<(String, String, String, String, i64)> =
            sqlx::query_as("SELECT id, uri, visibility, author_id, current_version FROM entries WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        row.map(|(id, uri, visibility, author_id, current_version)| {
            Ok(EntryAccess {
                id,
                uri,
                visibility: parse(&visibility)?,
                author_id,
                current_version,
            })
        })
        .transpose()
    }

    async fn entry_id_by_uri(&self, uri: &str) -> Result<Option<String>> {
        Ok(sqlx::query_scalar("SELECT id FROM entries WHERE uri = ?")
            .bind(uri)
            .fetch_optional(&self.pool)
            .await?)
    }

    async fn get_entry(&self, id: &str, version: Option<i64>) -> Result<Option<Entry>> {
        let joins = ENTRY_JOINS.replace(
            "v.version = e.current_version",
            "v.version = COALESCE(?, e.current_version)",
        );
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT ");
        qb.push(ENTRY_COLUMNS).push(", v.body, v.lang, v.provenance ");
        // The joins contain one placeholder for the requested version.
        let (before, after) = joins.split_once('?').expect("placeholder");
        qb.push(before).push_bind(version).push(after);
        qb.push(" WHERE e.id = ").push_bind(id.to_string());
        qb.build_query_as::<EntryDetailRow>()
            .fetch_optional(&self.pool)
            .await?
            .map(EntryDetailRow::into_entry)
            .transpose()
    }

    async fn list_entries(&self, filter: &EntryFilter, vis: &VisibilityFilter) -> Result<Page<EntrySummary>> {
        let mut count = QueryBuilder::<Sqlite>::new("SELECT count(*) ");
        count.push(ENTRY_JOINS);
        push_entry_filter(&mut count, filter, vis);
        let total: i64 = count.build_query_scalar().fetch_one(&self.pool).await?;

        let mut qb = QueryBuilder::<Sqlite>::new("SELECT ");
        qb.push(ENTRY_COLUMNS).push(ENTRY_JOINS);
        push_entry_filter(&mut qb, filter, vis);
        qb.push(" ORDER BY e.created_at DESC, e.id DESC LIMIT ")
            .push_bind(filter.limit)
            .push(" OFFSET ")
            .push_bind(filter.offset);
        let items = qb
            .build_query_as::<EntryRow>()
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(EntryRow::into_summary)
            .collect::<Result<_>>()?;
        Ok(Page {
            items,
            total,
            limit: filter.limit,
            offset: filter.offset,
        })
    }

    async fn list_versions(&self, id: &str) -> Result<Vec<VersionInfo>> {
        let rows: Vec<(i64, String, String, String, DateTime<Utc>)> = sqlx::query_as(
            "SELECT v.version, v.title, t.name, t.kind, v.created_at
             FROM entry_versions v JOIN tokens t ON t.id = v.token_id
             WHERE v.entry_id = ? ORDER BY v.version",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(version, title, name, kind, created_at)| {
                Ok(VersionInfo {
                    version,
                    title,
                    posted_via: PostedVia {
                        name,
                        kind: parse(&kind)?,
                    },
                    created_at,
                })
            })
            .collect()
    }

    async fn add_version(&self, id: &str, token_id: &str, content: VersionContent) -> Result<i64> {
        let mut tx = self.pool.begin().await?;
        let at = now();
        // The UPDATE takes the write lock first, so concurrent revisions get distinct numbers.
        let version: i64 = sqlx::query_scalar(
            "UPDATE entries SET current_version = current_version + 1, updated_at = ? WHERE id = ? RETURNING current_version",
        )
        .bind(&at)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(Error::NotFound)?;
        insert_version(&mut tx, id, version, token_id, &content, &at).await?;
        reindex(&mut tx, id).await?;
        tx.commit().await?;
        Ok(version)
    }

    async fn update_entry_meta(&self, id: &str, update: EntryMetaUpdate) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let mut qb = QueryBuilder::<Sqlite>::new("UPDATE entries SET updated_at = ");
        qb.push_bind(now());
        if let Some(v) = update.visibility {
            qb.push(", visibility = ").push_bind(v.as_str());
        }
        if let Some(s) = update.status {
            qb.push(", status = ").push_bind(s.as_str());
        }
        qb.push(" WHERE id = ").push_bind(id.to_string());
        if qb.build().execute(&mut *tx).await?.rows_affected() == 0 {
            return Err(Error::NotFound);
        }
        if let Some(tags) = update.tags {
            sqlx::query("DELETE FROM entry_tags WHERE entry_id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            insert_tags(&mut tx, id, &tags).await?;
            reindex(&mut tx, id).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn add_link(&self, link: NewLink) -> Result<String> {
        let id = new_id();
        sqlx::query(
            "INSERT INTO entry_links (id, source_id, target_uri, target_id, target_version, relation, note, created_by, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&link.source_id)
        .bind(&link.target_uri)
        .bind(&link.target_id)
        .bind(link.target_version)
        .bind(link.relation.as_str())
        .bind(&link.note)
        .bind(&link.created_by)
        .bind(now())
        .execute(&self.pool)
        .await
        .map_err(|e| match Error::from(e) {
            Error::Conflict(_) => Error::Conflict("this link already exists".into()),
            other => other,
        })?;
        Ok(id)
    }

    async fn links(&self, id: &str, vis: &VisibilityFilter) -> Result<Links> {
        let mut out = QueryBuilder::<Sqlite>::new(
            "SELECT l.id, l.relation, l.target_uri, l.target_id, l.target_version, l.note, l.created_at,
                    CASE WHEN te.id IS NOT NULL AND ",
        );
        push_visibility(&mut out, "te", vis);
        out.push(
            " THEN tv.title END AS target_title
             FROM entry_links l
             LEFT JOIN entries te ON te.id = l.target_id
             LEFT JOIN entry_versions tv ON tv.entry_id = te.id AND tv.version = te.current_version
             WHERE l.source_id = ",
        )
        .push_bind(id.to_string())
        .push(" ORDER BY l.created_at, l.id");
        type OutRow = (
            String,
            String,
            String,
            Option<String>,
            Option<i64>,
            Option<String>,
            DateTime<Utc>,
            Option<String>,
        );
        let outgoing = out
            .build_query_as::<OutRow>()
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(
                |(id, relation, target_uri, target_id, target_version, note, created_at, target_title)| {
                    Ok(OutgoingLink {
                        id,
                        relation: parse(&relation)?,
                        target_uri,
                        target_id,
                        target_version,
                        target_title,
                        note,
                        created_at,
                    })
                },
            )
            .collect::<Result<_>>()?;

        let mut inc = QueryBuilder::<Sqlite>::new(
            "SELECT l.id, l.relation, e.id, e.uri, v.title, l.target_version, l.note, l.created_at
             FROM entry_links l
             JOIN entries e ON e.id = l.source_id
             JOIN entry_versions v ON v.entry_id = e.id AND v.version = e.current_version
             WHERE l.target_id = ",
        );
        inc.push_bind(id.to_string()).push(" AND ");
        push_visibility(&mut inc, "e", vis);
        inc.push(" ORDER BY l.created_at, l.id");
        type InRow = (
            String,
            String,
            String,
            String,
            String,
            Option<i64>,
            Option<String>,
            DateTime<Utc>,
        );
        let incoming = inc
            .build_query_as::<InRow>()
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(
                |(id, relation, source_id, source_uri, source_title, target_version, note, created_at)| {
                    Ok(IncomingLink {
                        id,
                        relation: parse(&relation)?,
                        source_id,
                        source_uri,
                        source_title,
                        target_version,
                        note,
                        created_at,
                    })
                },
            )
            .collect::<Result<_>>()?;
        Ok(Links { outgoing, incoming })
    }

    async fn tag_counts(&self, vis: &VisibilityFilter, limit: i64) -> Result<Vec<TagCount>> {
        let mut qb = QueryBuilder::<Sqlite>::new(
            "SELECT et.tag, count(*) AS n FROM entry_tags et JOIN entries e ON e.id = et.entry_id WHERE ",
        );
        push_visibility(&mut qb, "e", vis);
        qb.push(" GROUP BY et.tag ORDER BY n DESC, et.tag LIMIT ")
            .push_bind(limit);
        Ok(qb
            .build_query_as::<(String, i64)>()
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|(tag, count)| TagCount { tag, count })
            .collect())
    }
}

// ---------------------------------------------------------------- comments

#[async_trait]
impl CommentStore for SqliteStore {
    async fn add_comment(&self, c: NewComment) -> Result<Comment> {
        let id = new_id();
        sqlx::query(
            "INSERT INTO comments (id, entry_id, parent_id, author_id, token_id, kind, body, result, environment, entry_version, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&c.entry_id)
        .bind(&c.parent_id)
        .bind(&c.author_id)
        .bind(&c.token_id)
        .bind(c.kind.as_str())
        .bind(&c.body)
        .bind(c.result.map(|r| r.as_str()))
        .bind(c.environment.map(|e| e.to_string()))
        .bind(c.entry_version)
        .bind(now())
        .execute(&self.pool)
        .await?;
        self.get_comment(&id).await?.ok_or(Error::NotFound)
    }

    async fn get_comment(&self, id: &str) -> Result<Option<Comment>> {
        sqlx::query_as::<_, CommentRow>(sqlx::AssertSqlSafe(format!("{COMMENT_SELECT} WHERE c.id = ?")))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(CommentRow::into_comment)
            .transpose()
    }

    async fn list_comments(&self, entry_id: &str) -> Result<Vec<Comment>> {
        sqlx::query_as::<_, CommentRow>(sqlx::AssertSqlSafe(format!(
            "{COMMENT_SELECT} WHERE c.entry_id = ? ORDER BY c.created_at, c.id"
        )))
        .bind(entry_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(CommentRow::into_comment)
        .collect()
    }
}

// ---------------------------------------------------------------- search

#[derive(sqlx::FromRow)]
struct SearchDbRow {
    #[sqlx(flatten)]
    base: EntryRow,
    search_body: String,
    score: Option<f64>,
}

fn push_search_where(qb: &mut QueryBuilder<Sqlite>, q: &SearchQuery, vis: &VisibilityFilter) {
    use crate::search::{fts_match_expr, like_pattern};

    push_entry_filter(qb, &q.filter, vis);
    if !q.indexed_terms.is_empty() {
        qb.push(" AND entry_fts MATCH ")
            .push_bind(fts_match_expr(&q.indexed_terms));
    }
    let any_field_like = |qb: &mut QueryBuilder<Sqlite>, term: &str| {
        let pattern = like_pattern(term);
        qb.push("(");
        for (i, col) in ["f.title", "f.summary", "f.body", "f.tags"].iter().enumerate() {
            if i > 0 {
                qb.push(" OR ");
            }
            qb.push(format!("{col} LIKE "))
                .push_bind(pattern.clone())
                .push(" ESCAPE '\\'");
        }
        qb.push(")");
    };
    for term in &q.short_terms {
        qb.push(" AND ");
        any_field_like(qb, term);
    }
    for term in &q.exclude {
        qb.push(" AND NOT ");
        any_field_like(qb, term);
    }
    if let Some(re) = &q.regex {
        let cols: &[&str] = match q.regex_field {
            RegexField::Title => &["f.title"],
            RegexField::Body => &["f.body"],
            RegexField::All => &["f.title", "f.summary", "f.body"],
        };
        qb.push(" AND (");
        for (i, col) in cols.iter().enumerate() {
            if i > 0 {
                qb.push(" OR ");
            }
            qb.push(format!("{col} REGEXP ")).push_bind(re.clone());
        }
        qb.push(")");
    }
}

#[async_trait]
impl SearchStore for SqliteStore {
    async fn search(&self, q: &SearchQuery, vis: &VisibilityFilter) -> Result<Page<SearchRow>> {
        let joins = ENTRY_JOINS.replacen(
            "FROM entries e",
            "FROM entries e JOIN entry_fts f ON f.rowid = e.seq",
            1,
        );
        let ranked = !q.indexed_terms.is_empty();

        let mut count = QueryBuilder::<Sqlite>::new("SELECT count(*) ");
        count.push(&joins);
        push_search_where(&mut count, q, vis);
        let total: i64 = count.build_query_scalar().fetch_one(&self.pool).await?;

        let mut qb = QueryBuilder::<Sqlite>::new("SELECT ");
        qb.push(ENTRY_COLUMNS).push(", f.body AS search_body, ");
        // bm25 weights follow the column order: title, summary, body, tags. Lower is better.
        qb.push(if ranked {
            "-bm25(entry_fts, 10.0, 5.0, 1.0, 3.0) AS score "
        } else {
            "NULL AS score "
        });
        qb.push(&joins);
        push_search_where(&mut qb, q, vis);
        qb.push(if ranked {
            " ORDER BY score DESC, e.updated_at DESC"
        } else {
            " ORDER BY e.updated_at DESC, e.id DESC"
        });
        qb.push(" LIMIT ")
            .push_bind(q.filter.limit)
            .push(" OFFSET ")
            .push_bind(q.filter.offset);

        let items = qb
            .build_query_as::<SearchDbRow>()
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|r| {
                Ok(SearchRow {
                    entry: r.base.into_summary()?,
                    score: r.score,
                    body: r.search_body,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Page {
            items,
            total,
            limit: q.filter.limit,
            offset: q.filter.offset,
        })
    }
}
