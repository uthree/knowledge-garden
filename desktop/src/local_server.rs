//! The Garden this app runs on this computer, for using Knowledge Garden without a separate
//! server. It is the same server as `knowledge-garden serve`, run inside the app on the tokio
//! runtime: starting it opens the database, stopping it (or quitting the app) finishes the
//! requests in flight and closes it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{Context, EventEmitter, Task};
use knowledge_garden::AppState;
use knowledge_garden::config::Config;
use knowledge_garden::store::AccountStore;
use knowledge_garden::store::sqlite::SqliteStore;

use crate::logs::{self, LogBuffer};
use crate::runtime;
use crate::settings::LocalServerSettings;

/// The name of the human token this app keeps for the local Garden.
const TOKEN_NAME: &str = "desktop";

/// How long a stop waits for requests in flight before cutting them off.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// A token for the app to use the local Garden with, issued when it had none.
#[derive(Debug, Clone)]
pub struct Access {
    pub username: String,
    pub secret: String,
}

/// A running server, as the tokio side sees it.
struct Running {
    store: SqliteStore,
    shutdown: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

/// Opens the database, makes sure the app can sign in, binds the port and serves.
async fn start(
    local: LocalServerSettings,
    username: Option<String>,
    need_token: bool,
) -> Result<(Running, Option<Access>), String> {
    let dir = local.data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{} を作れませんでした: {e}", dir.display()))?;
    let path = local.database_path();
    let store = SqliteStore::open(&path)
        .await
        .map_err(|e| format!("{} を開けませんでした: {e}", path.display()))?;

    let access = if need_token {
        Some(
            ensure_admin(&store, username.as_deref())
                .await
                .map_err(|e| format!("管理者を用意できませんでした: {e}"))?,
        )
    } else {
        None
    };

    let addr = local.bind_addr();
    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| match e.kind() {
        std::io::ErrorKind::AddrInUse => format!(
            "ポート {} は他のプログラムが使っています。設定でポートを変えてください",
            local.port
        ),
        std::io::ErrorKind::PermissionDenied => format!("ポート {} で待ち受ける権限がありません", local.port),
        _ => format!("{addr} で待ち受けられませんでした: {e}"),
    })?;

    let mut config = Config::new(&local.base_url(), &local.name);
    config.files_dir = local.files_dir();
    let state = AppState::new(store.clone(), config);
    let (shutdown, stopped) = tokio::sync::oneshot::channel::<()>();
    let sync = Duration::from_secs(local.sync_interval_secs);
    let task = tokio::spawn(knowledge_garden::server::serve(state, listener, Some(sync), async {
        let _ = stopped.await;
    }));
    tracing::info!(target: "knowledge_garden::desktop", "listening on {addr} ({})", path.display());
    Ok((Running { store, shutdown, task }, access))
}

/// The admin the app signs in as, with a fresh token: the one it used before, or one named
/// after the person's account on this computer, created on first start.
async fn ensure_admin(store: &SqliteStore, username: Option<&str>) -> knowledge_garden::error::Result<Access> {
    let candidates = username.map(str::to_string).into_iter().chain([os_username()]);
    for name in candidates {
        if let Some(user) = store.find_user(&name).await? {
            let secret = knowledge_garden::server::issue_human_token(store, &user, TOKEN_NAME).await?;
            tracing::info!(target: "knowledge_garden::desktop", "issued a new app token for {name}");
            return Ok(Access { username: name, secret });
        }
    }
    let name = os_username();
    let (user, secret) = knowledge_garden::server::bootstrap_admin(store, &name, &name, TOKEN_NAME).await?;
    tracing::info!(target: "knowledge_garden::desktop", "created the admin {}", user.username);
    Ok(Access {
        username: user.username,
        secret,
    })
}

/// The person's account name on this computer, as a Garden username.
pub fn os_username() -> String {
    let raw = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    username_from(&raw)
}

fn username_from(raw: &str) -> String {
    let mut name: String = raw
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    name = name.trim_matches('-').chars().take(32).collect();
    if knowledge_garden::model::validate_username(&name).is_ok() {
        name
    } else {
        "admin".into()
    }
}

async fn stop(running: Running) -> std::io::Result<()> {
    let Running {
        store: _,
        shutdown,
        mut task,
    } = running;
    let _ = shutdown.send(());
    match tokio::time::timeout(STOP_GRACE, &mut task).await {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => Err(std::io::Error::other(e)),
        Err(_) => {
            task.abort();
            tracing::warn!(target: "knowledge_garden::desktop", "requests still open after {STOP_GRACE:?}; cut off");
            Ok(())
        }
    }
}

/// Where a backup made now goes.
fn backup_path(dir: &Path) -> PathBuf {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    dir.join("backups").join(format!("garden-{stamp}.db"))
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServerStatus {
    Stopped,
    Starting,
    Running {
        /// The address it listens on, e.g. `127.0.0.1:8787`.
        addr: String,
        since: chrono::DateTime<chrono::Local>,
    },
    Stopping,
    Failed(String),
}

#[derive(Debug, Clone)]
pub enum ServerEvent {
    /// The server is up. `access` is set when a new token was issued for the app.
    Started {
        access: Option<Access>,
    },
    Failed(String),
    Stopped,
}

/// The local server, shared by the window and the server page.
pub struct LocalServer {
    status: ServerStatus,
    running: Option<Running>,
    /// The settings it was started with, to tell the page when a change needs a restart.
    started_with: Option<LocalServerSettings>,
    logs: LogBuffer,
    _watch_logs: Task<()>,
}

impl EventEmitter<ServerEvent> for LocalServer {}

impl LocalServer {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let logs = logs::buffer().clone();
        let watched = logs.clone();
        let watch = cx.spawn(async move |this, cx| {
            loop {
                watched.changed().await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
                // At most a few redraws a second while requests pour in.
                cx.background_executor().timer(Duration::from_millis(200)).await;
            }
        });
        Self {
            status: ServerStatus::Stopped,
            running: None,
            started_with: None,
            logs,
            _watch_logs: watch,
        }
    }

    pub fn status(&self) -> &ServerStatus {
        &self.status
    }

    pub fn is_running(&self) -> bool {
        matches!(self.status, ServerStatus::Running { .. })
    }

    /// Starting, running or stopping: not something to start again.
    pub fn is_busy(&self) -> bool {
        !matches!(self.status, ServerStatus::Stopped | ServerStatus::Failed(_))
    }

    pub fn started_with(&self) -> Option<&LocalServerSettings> {
        self.started_with.as_ref()
    }

    pub fn logs(&self) -> &LogBuffer {
        &self.logs
    }

    /// Starts the server. `username` is the admin the app signed in as before; `need_token`
    /// asks for a new token because the app has none saved.
    pub fn start(
        &mut self,
        local: LocalServerSettings,
        username: Option<String>,
        need_token: bool,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() {
            return;
        }
        self.status = ServerStatus::Starting;
        cx.notify();
        let settings = local.clone();
        cx.spawn(async move |this, cx| {
            let result = runtime::run(start(local, username, need_token)).await;
            this.update(cx, |this, cx| match result {
                Ok((running, access)) => {
                    this.status = ServerStatus::Running {
                        addr: settings.bind_addr().to_string(),
                        since: chrono::Local::now(),
                    };
                    this.running = Some(running);
                    this.started_with = Some(settings);
                    cx.emit(ServerEvent::Started { access });
                    cx.notify();
                }
                Err(message) => {
                    tracing::error!(target: "knowledge_garden::desktop", "{message}");
                    this.status = ServerStatus::Failed(message.clone());
                    cx.emit(ServerEvent::Failed(message));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Stops the server; the task finishes once it has.
    pub fn stop(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let Some(running) = self.running.take() else {
            return Task::ready(());
        };
        self.status = ServerStatus::Stopping;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = runtime::run(stop(running)).await;
            this.update(cx, |this, cx| {
                this.status = match result {
                    Ok(()) => {
                        tracing::info!(target: "knowledge_garden::desktop", "stopped");
                        ServerStatus::Stopped
                    }
                    Err(e) => ServerStatus::Failed(format!("止まるときにエラーが起きました: {e}")),
                };
                this.started_with = None;
                cx.emit(ServerEvent::Stopped);
                cx.notify();
            })
            .ok();
        })
    }

    /// Stops and starts again, picking up changed settings.
    pub fn restart(
        &mut self,
        local: LocalServerSettings,
        username: Option<String>,
        need_token: bool,
        cx: &mut Context<Self>,
    ) {
        let stopped = self.stop(cx);
        cx.spawn(async move |this, cx| {
            stopped.await;
            this.update(cx, |this, cx| this.start(local, username, need_token, cx))
                .ok();
        })
        .detach();
    }

    /// Writes a copy of the database into the data folder's `backups`, and returns its path.
    pub fn backup(&self, cx: &mut Context<Self>) -> Task<Result<PathBuf, String>> {
        let (Some(running), Some(settings)) = (&self.running, &self.started_with) else {
            return Task::ready(Err("サーバーが止まっています".into()));
        };
        let store = running.store.clone();
        let path = backup_path(&settings.data_dir());
        cx.background_executor().spawn(async move {
            let target = path.clone();
            runtime::run(async move {
                store
                    .backup_to(&target.to_string_lossy())
                    .await
                    .map_err(|e| format!("バックアップできませんでした: {e}"))
            })
            .await?;
            tracing::info!(target: "knowledge_garden::desktop", "backed up to {}", path.display());
            Ok(path)
        })
    }

    /// Sets the password `username` signs in to the web UI with, here or from another device.
    pub fn set_password(&self, username: String, password: String, cx: &mut Context<Self>) -> Task<Result<(), String>> {
        let Some(running) = &self.running else {
            return Task::ready(Err("サーバーが止まっています".into()));
        };
        if let Err(e) = knowledge_garden::auth::validate_password(&password) {
            return Task::ready(Err(e));
        }
        let store = running.store.clone();
        cx.background_executor().spawn(runtime::run(async move {
            let user = store
                .find_user(&username)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("ユーザー {username} が見つかりません"))?;
            let hash = knowledge_garden::auth::hash_password(password)
                .await
                .map_err(|e| e.to_string())?;
            store
                .set_password(&user.id, Some(&hash))
                .await
                .map_err(|e| e.to_string())?;
            store.delete_user_sessions(&user.id).await.map_err(|e| e.to_string())?;
            Ok(())
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames_come_from_the_account_on_this_computer() {
        assert_eq!(username_from("uthree"), "uthree");
        assert_eq!(username_from("Taro Yamada"), "taro-yamada");
        assert_eq!(username_from("  "), "admin");
        assert_eq!(username_from("山田"), "admin");
        assert_eq!(username_from(&"a".repeat(40)).len(), 32);
    }

    fn settings(dir: &Path) -> LocalServerSettings {
        LocalServerSettings {
            port: 0,
            data_dir: Some(dir.join("data dir")),
            sync_interval_secs: 0,
            ..LocalServerSettings::default()
        }
    }

    #[test]
    fn starts_signs_the_app_in_and_stops() {
        let dir = tempfile::tempdir().unwrap();
        runtime::runtime().block_on(async {
            let local = settings(dir.path());
            let (running, access) = start(local.clone(), None, true).await.unwrap();
            let access = access.unwrap();
            assert_eq!(access.username, os_username());
            assert!(local.database_path().exists());

            let backup = backup_path(&local.data_dir());
            running.store.backup_to(&backup.to_string_lossy()).await.unwrap();
            assert!(backup.exists());
            stop(running).await.unwrap();

            // Started again with the token lost: the same admin gets a new one.
            let (running, again) = start(local.clone(), Some(access.username.clone()), true).await.unwrap();
            let again = again.unwrap();
            assert_eq!(again.username, access.username);
            assert_ne!(again.secret, access.secret);
            let (still, none) = start(local, None, false).await.unwrap();
            assert!(none.is_none());
            stop(still).await.unwrap();
            stop(running).await.unwrap();
        });
    }

    #[test]
    fn a_port_in_use_is_reported_plainly() {
        let dir = tempfile::tempdir().unwrap();
        runtime::runtime().block_on(async {
            let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let local = LocalServerSettings {
                port: taken.local_addr().unwrap().port(),
                ..settings(dir.path())
            };
            let error = start(local, None, false).await.err().unwrap();
            assert!(error.contains("他のプログラムが使っています"), "{error}");
        });
    }
}
