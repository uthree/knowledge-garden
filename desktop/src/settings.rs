//! What the app remembers between runs: the Gardens it connects to, their tokens and the
//! person's preferences.
//!
//! `settings.json` holds everything but secrets. Tokens live beside it in `tokens.json`, which
//! only the owner can read (mode 0600 on Unix), so a settings file shared to ask for help leaks
//! nothing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The folder for this app's own files inside the platform's configuration folder.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("KG_DESKTOP_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("knowledge-garden-desktop")
}

/// The folder for the local Garden's database and backups.
pub fn default_data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("KG_DESKTOP_DATA_DIR") {
        return PathBuf::from(dir);
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("knowledge-garden-desktop")
}

/// The id of the connection to the Garden this app runs.
pub const LOCAL_CONNECTION_ID: &str = "local";

/// How to run the Garden on this computer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalServerSettings {
    pub port: u16,
    /// Listen on every network interface, so other devices on the LAN can connect.
    pub lan: bool,
    /// The public base URL entry URIs are built from; `http://localhost:<port>` when unset.
    pub base_url: Option<String>,
    /// Start the server when the app starts (once it has been set up).
    pub autostart: bool,
    /// Where the database lives; [`default_data_dir`] when unset.
    pub data_dir: Option<PathBuf>,
    pub name: String,
    /// Seconds between pulls from approved peer Gardens; 0 pulls only on demand.
    pub sync_interval_secs: u64,
}

impl Default for LocalServerSettings {
    fn default() -> Self {
        Self {
            port: 8787,
            lan: false,
            base_url: None,
            autostart: true,
            data_dir: None,
            name: "手元の Garden".into(),
            sync_interval_secs: 300,
        }
    }
}

impl LocalServerSettings {
    pub fn data_dir(&self) -> PathBuf {
        self.data_dir.clone().unwrap_or_else(default_data_dir)
    }

    pub fn database_path(&self) -> PathBuf {
        self.data_dir().join("garden.db")
    }

    /// Where the Garden keeps attached files.
    pub fn files_dir(&self) -> PathBuf {
        self.data_dir().join("files")
    }

    /// The Garden's public base URL.
    pub fn base_url(&self) -> String {
        match self.base_url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
            Some(url) => url.trim_end_matches('/').to_string(),
            None => format!("http://localhost:{}", self.port),
        }
    }

    /// Where this app reaches the server: always the loopback address, which works whether or
    /// not the server also listens on the LAN.
    pub fn client_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn bind_addr(&self) -> std::net::SocketAddr {
        let ip = if self.lan {
            std::net::Ipv4Addr::UNSPECIFIED
        } else {
            std::net::Ipv4Addr::LOCALHOST
        };
        (ip, self.port).into()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionKind {
    /// A Garden somewhere else: a NAS, a server.
    #[default]
    Remote,
    /// The Garden this app runs on this computer.
    Local,
}

/// A Garden the app connects to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    pub id: String,
    /// The name shown in the switcher; the Garden's own name until the person renames it.
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub kind: ConnectionKind,
    /// The user the saved token belongs to, for showing who is signed in.
    #[serde(default)]
    pub username: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppearanceChoice {
    /// Follow the system's light or dark mode.
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub connections: Vec<Connection>,
    /// The id of the connection in use.
    pub current: Option<String>,
    pub appearance: AppearanceChoice,
    /// Atelier's direction: `glass` (the adopted one), `studio` or `mono`.
    pub direction: String,
    pub local: LocalServerSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            connections: Vec::new(),
            current: None,
            appearance: AppearanceChoice::System,
            direction: "glass".into(),
            local: LocalServerSettings::default(),
        }
    }
}

impl Settings {
    pub fn current(&self) -> Option<&Connection> {
        let id = self.current.as_deref()?;
        self.connections.iter().find(|c| c.id == id)
    }

    /// Adds `connection`, or replaces the one with the same id or base URL, and makes it current.
    pub fn upsert(&mut self, connection: Connection) {
        let id = connection.id.clone();
        match self
            .connections
            .iter_mut()
            .find(|c| c.id == connection.id || c.base_url == connection.base_url)
        {
            Some(existing) => {
                let id = existing.id.clone();
                *existing = Connection { id, ..connection };
                self.current = Some(existing.id.clone());
            }
            None => {
                self.connections.push(connection);
                self.current = Some(id);
            }
        }
    }

    /// The connection to the Garden this app runs, once it has been set up.
    pub fn local_connection(&self) -> Option<&Connection> {
        self.connections.iter().find(|c| c.kind == ConnectionKind::Local)
    }

    pub fn remove(&mut self, id: &str) {
        self.connections.retain(|c| c.id != id);
        if self.current.as_deref() == Some(id) {
            self.current = self.connections.first().map(|c| c.id.clone());
        }
    }
}

/// Settings and tokens in one folder.
#[derive(Debug, Clone)]
pub struct SettingsStore {
    dir: PathBuf,
}

impl SettingsStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn default_location() -> Self {
        Self::new(config_dir())
    }

    fn settings_path(&self) -> PathBuf {
        self.dir.join("settings.json")
    }

    fn tokens_path(&self) -> PathBuf {
        self.dir.join("tokens.json")
    }

    /// The saved settings; defaults when there are none yet or the file cannot be read (a
    /// broken file is kept aside as `settings.json.broken` rather than overwritten silently).
    pub fn load(&self) -> Settings {
        let path = self.settings_path();
        let Ok(bytes) = std::fs::read(&path) else {
            return Settings::default();
        };
        match serde_json::from_slice(&bytes) {
            Ok(settings) => settings,
            Err(_) => {
                let _ = std::fs::rename(&path, self.dir.join("settings.json.broken"));
                Settings::default()
            }
        }
    }

    pub fn save(&self, settings: &Settings) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let json = serde_json::to_vec_pretty(settings).map_err(std::io::Error::other)?;
        write_atomic(&self.settings_path(), &json, false)
    }

    fn tokens(&self) -> BTreeMap<String, String> {
        std::fs::read(self.tokens_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// The saved token of a connection.
    pub fn token(&self, connection_id: &str) -> Option<String> {
        self.tokens().remove(connection_id)
    }

    /// Saves (or with `None` forgets) a connection's token.
    pub fn set_token(&self, connection_id: &str, token: Option<&str>) -> std::io::Result<()> {
        let mut tokens = self.tokens();
        match token {
            Some(t) => tokens.insert(connection_id.to_string(), t.to_string()),
            None => tokens.remove(connection_id),
        };
        std::fs::create_dir_all(&self.dir)?;
        let json = serde_json::to_vec_pretty(&tokens).map_err(std::io::Error::other)?;
        write_atomic(&self.tokens_path(), &json, true)
    }
}

/// Writes through a temporary file and a rename, so a crash never leaves half a file.
fn write_atomic(path: &Path, bytes: &[u8], private: bool) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        #[cfg(not(unix))]
        let _ = private;
        let mut file = options.open(&tmp)?;
        std::io::Write::write_all(&mut file, bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// A short random id for a new connection.
pub fn new_id() -> String {
    let n: u64 = rand_u64();
    format!("{n:016x}")
}

fn rand_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default(),
    );
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(id: &str, url: &str) -> Connection {
        Connection {
            id: id.into(),
            name: "NAS".into(),
            base_url: url.into(),
            kind: ConnectionKind::Remote,
            username: None,
        }
    }

    #[test]
    fn settings_and_tokens_round_trip_and_tokens_are_private() {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(dir.path());
        assert_eq!(store.load(), Settings::default());

        let mut settings = Settings::default();
        settings.upsert(remote("a", "http://nas.local:8080"));
        store.save(&settings).unwrap();
        store.set_token("a", Some("kg_secret")).unwrap();

        assert_eq!(store.load(), settings);
        assert_eq!(store.token("a").as_deref(), Some("kg_secret"));
        let saved = std::fs::read_to_string(dir.path().join("settings.json")).unwrap();
        assert!(!saved.contains("kg_secret"), "secrets stay out of settings.json");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("tokens.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "only the owner can read the tokens");
        }
        store.set_token("a", None).unwrap();
        assert_eq!(store.token("a"), None);
    }

    #[test]
    fn a_broken_settings_file_is_kept_aside() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), b"{ not json").unwrap();
        let store = SettingsStore::new(dir.path());
        assert_eq!(store.load(), Settings::default());
        assert!(dir.path().join("settings.json.broken").exists());
    }

    #[test]
    fn adding_the_same_garden_again_replaces_it() {
        let mut settings = Settings::default();
        settings.upsert(remote("a", "http://nas.local:8080"));
        settings.upsert(remote("b", "https://garden.example"));
        settings.upsert(Connection {
            name: "Home".into(),
            ..remote("c", "http://nas.local:8080")
        });
        assert_eq!(settings.connections.len(), 2);
        assert_eq!(settings.current().unwrap().name, "Home");
        assert_eq!(
            settings.current().unwrap().id,
            "a",
            "keeps the id the token is saved under"
        );
        settings.remove("a");
        assert_eq!(settings.current().unwrap().id, "b");
    }

    #[test]
    fn the_local_server_defaults_to_this_computer_only() {
        let local = LocalServerSettings::default();
        assert_eq!(local.bind_addr().to_string(), "127.0.0.1:8787");
        assert_eq!(local.base_url(), "http://localhost:8787");
        assert_eq!(local.client_url(), "http://127.0.0.1:8787");
        let lan = LocalServerSettings {
            lan: true,
            base_url: Some(" http://studio.local:8787/ ".into()),
            ..local
        };
        assert_eq!(lan.bind_addr().to_string(), "0.0.0.0:8787");
        assert_eq!(lan.base_url(), "http://studio.local:8787");
        // Settings saved before the local server existed still load.
        let old: Settings = serde_json::from_str(r#"{"connections":[],"direction":"glass"}"#).unwrap();
        assert_eq!(old.local, LocalServerSettings::default());
    }
}
