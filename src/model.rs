//! Domain types shared by the store and the HTTP API.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

macro_rules! string_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident => $s:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $($(#[$vmeta])* #[serde(rename = $s)] $variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $s),+ }
            }
        }

        impl std::str::FromStr for $name {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, String> {
                match s {
                    $($s => Ok($name::$variant),)+
                    _ => Err(format!("invalid {}: {s}", stringify!($name))),
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum!(Role { Admin => "admin", Member => "member" });
string_enum!(TokenKind { Human => "human", Agent => "agent" });
string_enum!(
    /// What a token is allowed to do.
    Scope {
        Read => "read",
        EntriesWrite => "entries:write",
        CommentsWrite => "comments:write",
        Admin => "admin",
    }
);
string_enum!(EntryKind {
    Memo => "memo",
    ResearchNote => "research_note",
    ResearchReport => "research_report",
    Troubleshooting => "troubleshooting",
});
string_enum!(
    /// Who can read an entry.
    Visibility {
        /// Anyone, including guests. Listed, searchable and mirrored to federated Gardens.
        Public => "public",
        /// Anyone with the link. Not listed or searched (except by the author).
        Unlisted => "unlisted",
        /// Members of this Garden and (once federation exists) approved peer Gardens.
        Federated => "federated",
        /// Members of this Garden only.
        Members => "members",
        /// The author only.
        Private => "private",
    }
);
string_enum!(Status {
    Idea => "idea",
    Investigating => "investigating",
    Preliminary => "preliminary",
    Verified => "verified",
    Superseded => "superseded",
    Retracted => "retracted",
});
string_enum!(Relation {
    Cites => "cites",
    Uses => "uses",
    Supports => "supports",
    Contradicts => "contradicts",
    Extends => "extends",
    Reproduces => "reproduces",
    Revises => "revises",
    Supersedes => "supersedes",
});
string_enum!(CommentKind { Comment => "comment", Reproduction => "reproduction" });
string_enum!(ReproResult { Reproduced => "reproduced", Partial => "partial", Failed => "failed" });

pub const DEFAULT_SCOPES: &[Scope] = &[Scope::Read, Scope::EntriesWrite, Scope::CommentsWrite];

/// The scopes of a user's own human tokens: the defaults, plus `admin` for admins.
pub fn scopes_for_role(role: Role) -> Vec<Scope> {
    let mut scopes = DEFAULT_SCOPES.to_vec();
    if role == Role::Admin {
        scopes.push(Scope::Admin);
    }
    scopes
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub role: Role,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub kind: TokenKind,
    pub scopes: Vec<Scope>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// An authenticated caller: a user acting through one of their tokens.
#[derive(Debug, Clone)]
pub struct Principal {
    pub user: User,
    pub token: Token,
}

impl Principal {
    pub fn has(&self, scope: Scope) -> bool {
        self.token.scopes.contains(&scope)
    }

    pub fn is_admin(&self) -> bool {
        self.user.role == Role::Admin && self.has(Scope::Admin)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invite {
    pub id: String,
    pub role: Role,
    pub created_by: String,
    pub note: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub used_by: Option<String>,
    pub used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorRef {
    pub username: String,
    pub display_name: String,
}

/// The token that produced a version or comment, so readers can tell AI from human posts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostedVia {
    pub name: String,
    pub kind: TokenKind,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReproStats {
    pub reproduced: i64,
    pub partial: i64,
    pub failed: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub uri: String,
    pub origin: String,
    pub is_local: bool,
    pub kind: EntryKind,
    pub visibility: Visibility,
    pub status: Status,
    pub author: AuthorRef,
    #[serde(skip)]
    pub author_id: String,
    pub tags: Vec<String>,
    pub version: i64,
    pub current_version: i64,
    pub title: String,
    pub summary: Option<String>,
    pub body: String,
    pub lang: Option<String>,
    pub provenance: serde_json::Value,
    pub posted_via: PostedVia,
    pub reproductions: ReproStats,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version_created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntrySummary {
    pub id: String,
    pub uri: String,
    pub origin: String,
    pub is_local: bool,
    pub kind: EntryKind,
    pub visibility: Visibility,
    pub status: Status,
    pub author: AuthorRef,
    pub posted_via: PostedVia,
    pub tags: Vec<String>,
    pub current_version: i64,
    pub title: String,
    pub summary: Option<String>,
    pub reproductions: ReproStats,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: i64,
    pub title: String,
    pub posted_via: PostedVia,
    pub created_at: DateTime<Utc>,
}

/// Minimal facts needed for permission checks.
#[derive(Debug, Clone)]
pub struct EntryAccess {
    pub id: String,
    pub uri: String,
    pub visibility: Visibility,
    pub author_id: String,
    pub current_version: i64,
    /// False for entries mirrored from another Garden, which are read-only here.
    pub is_local: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingLink {
    pub id: String,
    pub relation: Relation,
    pub target_uri: String,
    pub target_id: Option<String>,
    pub target_version: Option<i64>,
    /// Only present when the target is local and visible to the caller.
    pub target_title: Option<String>,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingLink {
    pub id: String,
    pub relation: Relation,
    pub source_id: String,
    pub source_uri: String,
    pub source_title: String,
    pub target_version: Option<i64>,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Links {
    pub outgoing: Vec<OutgoingLink>,
    pub incoming: Vec<IncomingLink>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub entry_id: String,
    pub parent_id: Option<String>,
    pub author: AuthorRef,
    pub posted_via: PostedVia,
    pub kind: CommentKind,
    pub body: String,
    pub result: Option<ReproResult>,
    pub environment: Option<serde_json::Value>,
    pub entry_version: Option<i64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagCount {
    pub tag: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

/// A recommended (not required) section of an entry body.
pub struct Section {
    pub label: &'static str,
    /// Heading texts that count as this section (matched case-insensitively).
    pub aliases: &'static [&'static str],
}

const fn s(label: &'static str, aliases: &'static [&'static str]) -> Section {
    Section { label, aliases }
}

impl Visibility {
    /// Whether entries with this visibility are sent to approved peer Gardens.
    pub fn is_shareable(self) -> bool {
        matches!(self, Visibility::Public | Visibility::Federated)
    }
}

impl EntryKind {
    pub fn default_status(self) -> Status {
        match self {
            EntryKind::Memo => Status::Idea,
            EntryKind::ResearchNote => Status::Investigating,
            EntryKind::ResearchReport | EntryKind::Troubleshooting => Status::Preliminary,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            EntryKind::Memo => "アイデア、疑問、雑記、途中経過。形式は自由。",
            EntryKind::ResearchNote => "調査、試行錯誤、軽い実験の記録。",
            EntryKind::ResearchReport => "まとまった検証可能な成果。他の投稿を引用してまとめる。",
            EntryKind::Troubleshooting => "遭遇したエラーや問題と、その原因・解決策の記録。",
        }
    }

    pub fn recommended_sections(self) -> &'static [Section] {
        const NOTE: &[Section] = &[
            s("概要 / Summary", &["概要", "summary", "abstract"]),
            s(
                "背景・問題 / Background",
                &["背景", "問題", "background", "problem", "motivation"],
            ),
            s(
                "試したこと / What I tried",
                &["試したこと", "方法", "手順", "what i tried", "method", "approach"],
            ),
            s(
                "分かったこと / Findings",
                &["分かったこと", "わかったこと", "結果", "findings", "results", "learned"],
            ),
            s(
                "次にやること / Next steps",
                &["次にやること", "今後", "next steps", "next", "todo"],
            ),
        ];
        const REPORT: &[Section] = &[
            s(
                "問題・仮説 / Problem & hypothesis",
                &["問題", "仮説", "problem", "hypothesis", "introduction"],
            ),
            s("方法 / Method", &["方法", "手法", "method", "methods", "methodology"]),
            s("結果 / Results", &["結果", "results", "result"]),
            s("考察 / Discussion", &["考察", "discussion", "analysis"]),
            s("限界 / Limitations", &["限界", "制約", "limitations", "limitation"]),
            s(
                "今後の課題 / Future work",
                &["今後の課題", "今後", "future work", "open questions", "未解決"],
            ),
            s(
                "参考文献 / References",
                &["参考文献", "参考", "references", "bibliography"],
            ),
        ];
        const TROUBLE: &[Section] = &[
            s(
                "症状 / Symptom",
                &["症状", "現象", "エラー", "symptom", "problem", "error"],
            ),
            s("環境 / Environment", &["環境", "environment"]),
            s("原因 / Cause", &["原因", "cause", "root cause"]),
            s(
                "解決策 / Solution",
                &["解決策", "解決", "対処", "solution", "fix", "workaround"],
            ),
        ];
        match self {
            EntryKind::Memo => &[],
            EntryKind::ResearchNote => NOTE,
            EntryKind::ResearchReport => REPORT,
            EntryKind::Troubleshooting => TROUBLE,
        }
    }

    /// Advisory warnings for missing recommended sections or provenance.
    /// Entries are never rejected for these; short posts are welcome.
    pub fn content_warnings(self, body: &str, provenance: &serde_json::Value) -> Vec<String> {
        let headings: Vec<String> = body
            .lines()
            .map(str::trim_start)
            .filter(|l| l.starts_with('#'))
            .map(|l| l.trim_start_matches('#').trim().to_lowercase())
            .collect();
        let mut warnings: Vec<String> = self
            .recommended_sections()
            .iter()
            .filter(|sec| !headings.iter().any(|h| sec.aliases.iter().any(|a| h.contains(a))))
            .map(|sec| format!("recommended section not found: {}", sec.label))
            .collect();
        let empty_provenance = provenance.as_object().is_none_or(|o| o.is_empty());
        if self != EntryKind::Memo && empty_provenance {
            warnings.push(
                "provenance is empty: recording model, tools, commit, seed, command or environment helps others reproduce this"
                    .into(),
            );
        }
        warnings
    }
}

pub const MAX_TITLE_CHARS: usize = 300;
pub const MAX_SUMMARY_CHARS: usize = 2_000;
pub const MAX_BODY_BYTES: usize = 1_000_000;
pub const MAX_TAGS: usize = 20;
pub const MAX_TAG_CHARS: usize = 50;

/// Normalizes tags: trims, lowercases, turns inner whitespace into `-`, and removes duplicates.
pub fn normalize_tags(tags: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for raw in tags {
        let tag = raw.split_whitespace().collect::<Vec<_>>().join("-").to_lowercase();
        if tag.is_empty() {
            continue;
        }
        if tag.chars().count() > MAX_TAG_CHARS {
            return Err(format!("tag is longer than {MAX_TAG_CHARS} characters: {raw}"));
        }
        if tag.chars().any(|c| c.is_control() || c == ',') {
            return Err(format!("tag contains invalid characters: {raw}"));
        }
        if !out.contains(&tag) {
            out.push(tag);
        }
    }
    if out.len() > MAX_TAGS {
        return Err(format!("at most {MAX_TAGS} tags are allowed"));
    }
    out.sort();
    Ok(out)
}

pub fn validate_title(title: &str) -> Result<String, String> {
    let t = title.trim();
    if t.is_empty() {
        return Err("title must not be empty".into());
    }
    if t.chars().count() > MAX_TITLE_CHARS {
        return Err(format!("title is longer than {MAX_TITLE_CHARS} characters"));
    }
    Ok(t.to_string())
}

pub fn validate_summary(summary: Option<String>) -> Result<Option<String>, String> {
    match summary.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        Some(s) if s.chars().count() > MAX_SUMMARY_CHARS => {
            Err(format!("summary is longer than {MAX_SUMMARY_CHARS} characters"))
        }
        other => Ok(other),
    }
}

pub fn validate_body(body: &str) -> Result<(), String> {
    if body.trim().is_empty() {
        return Err("body must not be empty".into());
    }
    if body.len() > MAX_BODY_BYTES {
        return Err(format!("body is larger than {MAX_BODY_BYTES} bytes"));
    }
    Ok(())
}

/// Usernames are used in URLs, so they are restricted to `[a-z0-9_-]`, 2..=32 characters.
pub fn validate_username(username: &str) -> Result<(), String> {
    let ok_len = (2..=32).contains(&username.len());
    let ok_chars = username
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if ok_len && ok_chars {
        Ok(())
    } else {
        Err("username must be 2-32 characters of a-z, 0-9, '_' or '-'".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tags_are_normalized() {
        let tags = normalize_tags(&[" Rust ".into(), "rust".into(), "Async  Await".into(), "".into()]).unwrap();
        assert_eq!(tags, vec!["async-await", "rust"]);
    }

    #[test]
    fn too_many_tags_are_rejected() {
        let tags: Vec<String> = (0..21).map(|i| format!("t{i}")).collect();
        assert!(normalize_tags(&tags).is_err());
    }

    #[test]
    fn warnings_list_missing_sections() {
        let body = "# 症状\nfoo\n## 原因\nbar\n## 解決策\nbaz";
        let w = EntryKind::Troubleshooting.content_warnings(body, &json!({"os": "linux"}));
        assert_eq!(w, vec!["recommended section not found: 環境 / Environment"]);
    }

    #[test]
    fn memo_has_no_warnings() {
        assert!(
            EntryKind::Memo
                .content_warnings("just a thought", &json!({}))
                .is_empty()
        );
    }

    #[test]
    fn username_rules() {
        assert!(validate_username("uthree").is_ok());
        assert!(validate_username("Uthree").is_err());
        assert!(validate_username("a").is_err());
    }
}
