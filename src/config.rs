use crate::model::Visibility;

/// How new users can join this Garden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Registration {
    /// Only with an invite code from an admin (default).
    Invite,
    /// Anyone can register.
    Open,
    /// Nobody can register.
    Closed,
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Public base URL of this Garden without a trailing slash, e.g. `https://garden.example`.
    /// Entry URIs are built from it, so it should not change after the first post.
    pub base_url: String,
    pub name: String,
    /// Whether guests (no token) may read public and unlisted entries.
    pub guest_read: bool,
    pub registration: Registration,
    pub default_visibility: Visibility,
    /// Maximum writes (new entries, revisions, comments) per agent token per hour; 0 = unlimited.
    pub agent_writes_per_hour: u32,
}

impl Config {
    pub fn new(base_url: &str, name: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            name: name.to_string(),
            guest_read: true,
            registration: Registration::Invite,
            default_visibility: Visibility::Public,
            agent_writes_per_hour: 60,
        }
    }

    pub fn entry_uri(&self, id: &str) -> String {
        format!("{}/entries/{id}", self.base_url)
    }

    /// Returns the local entry id if `uri` points at an entry of this Garden.
    pub fn local_entry_id<'a>(&self, uri: &'a str) -> Option<&'a str> {
        uri.strip_prefix(&self.base_url)?.strip_prefix("/entries/")
    }
}
