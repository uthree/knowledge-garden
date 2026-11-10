//! Updates from this repository's GitHub Releases, as Auris Studio does them: find the newest
//! `desktop-vX.Y.Z` release, download its archive for this platform in the background, check its
//! size and SHA-256 against what GitHub reports, and swap it in place of the running app. The
//! new version starts on the next launch, or at once when the person restarts from the notice.
//!
//! Only a copy installed from a release archive updates itself. A build under Cargo's `target`
//! or `~/.cargo/bin`, or one in a folder the app cannot write to, only points at the release
//! page.

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;
use gpui::{Context, Task};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::runtime;

const RELEASES_API: &str = "https://api.github.com/repos/uthree/knowledge-garden/releases?per_page=50";
pub const RELEASE_PAGE: &str = "https://github.com/uthree/knowledge-garden/releases";
const TAG_PREFIX: &str = "desktop-v";
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
/// The first check waits for the window to settle; later ones come this often.
const FIRST_CHECK: Duration = Duration::from_secs(15);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("the crate version is semver")
}

/// The archive of a release to download.
#[derive(Debug, Clone, PartialEq)]
pub struct ReleaseAsset {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

/// A newer release than this app.
#[derive(Debug, Clone, PartialEq)]
pub struct Available {
    pub version: Version,
    pub tag: String,
    /// `None` when this platform has no archive or the release gives no digest to check it by:
    /// then the person downloads it from the release page.
    pub asset: Option<ReleaseAsset>,
}

impl Available {
    pub fn page(&self) -> String {
        format!("{RELEASE_PAGE}/tag/{}", self.tag)
    }
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<AssetResponse>,
}

#[derive(Deserialize)]
struct AssetResponse {
    name: String,
    size: u64,
    digest: Option<String>,
    browser_download_url: String,
}

/// The archive name for this platform, or `None` where no archive is built.
fn platform_suffix() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("macos-aarch64.tar.gz")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("windows-x86_64.zip")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("linux-x86_64.tar.gz")
    } else {
        None
    }
}

pub fn archive_name(version: &Version, suffix: &str) -> String {
    format!("knowledge-garden-desktop-v{version}-{suffix}")
}

/// The newest stable desktop release, if it is newer than `current`.
fn newest(releases: &[Release], current: &Version, suffix: Option<&str>) -> Result<Option<Available>, String> {
    let newest = releases
        .iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| {
            let version = Version::parse(r.tag_name.strip_prefix(TAG_PREFIX)?).ok()?;
            version.pre.is_empty().then_some((version, r))
        })
        .max_by(|a, b| a.0.cmp(&b.0));
    let Some((version, release)) = newest else {
        return Ok(None);
    };
    if version <= *current {
        return Ok(None);
    }
    let asset = match suffix {
        Some(suffix) => select_asset(release, &version, suffix)?,
        None => None,
    };
    Ok(Some(Available {
        tag: release.tag_name.clone(),
        version,
        asset,
    }))
}

fn select_asset(release: &Release, version: &Version, suffix: &str) -> Result<Option<ReleaseAsset>, String> {
    let name = archive_name(version, suffix);
    let mut matching = release.assets.iter().filter(|a| a.name == name);
    let Some(asset) = matching.next() else {
        return Ok(None);
    };
    if matching.next().is_some() {
        return Err("リリースに同じ名前のファイルが複数あります".into());
    }
    let url = format!(
        "https://github.com/uthree/knowledge-garden/releases/download/{}/{name}",
        release.tag_name
    );
    if asset.browser_download_url != url {
        return Err("リリースのファイルが想定外の場所を指しています".into());
    }
    if asset.size == 0 || asset.size > MAX_ARCHIVE_BYTES {
        return Err("リリースのファイルの大きさが想定外です".into());
    }
    let Some(sha256) = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|d| d.len() == 64 && d.bytes().all(|b| b.is_ascii_hexdigit()))
    else {
        // Nothing to check the download against: leave it to the release page.
        return Ok(None);
    };
    Ok(Some(ReleaseAsset {
        name,
        url,
        size: asset.size,
        sha256: sha256.to_ascii_lowercase(),
    }))
}

fn http() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("knowledge-garden-desktop/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .https_only(true)
        .build()
        .map_err(|e| e.to_string())
}

/// Asks GitHub for the newest desktop release.
pub async fn check() -> Result<Option<Available>, String> {
    let response = http()?
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| format!("リリースを確認できませんでした: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("リリースを確認できませんでした（HTTP {}）", response.status()));
    }
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err("リリースの一覧が大きすぎます".into());
    }
    let releases: Vec<Release> =
        serde_json::from_slice(&bytes).map_err(|e| format!("リリースの一覧を読めませんでした: {e}"))?;
    newest(&releases, &current_version(), platform_suffix())
}

/// Where this app is installed, if it can replace itself there.
#[derive(Debug, Clone, PartialEq)]
pub struct InstallTarget {
    /// The executable, or on macOS the `.app` bundle holding it.
    pub path: PathBuf,
    /// The name of that file or bundle inside a release archive.
    pub archive_entry: String,
}

/// The installed app, or `None` for a development build or an unwritable install.
pub fn install_target() -> Option<InstallTarget> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".cargo")));
    target_for(&exe, cargo_home.as_deref()).filter(|t| t.path.parent().is_some_and(writable))
}

fn target_for(exe: &Path, cargo_home: Option<&Path>) -> Option<InstallTarget> {
    if exe.components().any(|c| c.as_os_str() == "target") {
        return None;
    }
    if cargo_home.is_some_and(|home| exe.starts_with(home)) {
        return None;
    }
    // macOS: …/Knowledge Garden.app/Contents/MacOS/knowledge-garden-desktop
    let bundle = exe.parent()?.parent()?.parent()?;
    if cfg!(target_os = "macos") && bundle.extension().is_some_and(|e| e == "app") {
        return Some(InstallTarget {
            archive_entry: bundle.file_name()?.to_string_lossy().into_owned(),
            path: bundle.to_path_buf(),
        });
    }
    Some(InstallTarget {
        archive_entry: exe.file_name()?.to_string_lossy().into_owned(),
        path: exe.to_path_buf(),
    })
}

fn writable(dir: &Path) -> bool {
    tempfile::NamedTempFile::new_in(dir).is_ok()
}

/// The old copy a swap leaves beside the new one.
fn old_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    target.with_file_name(name)
}

fn staging_dir(target: &Path) -> PathBuf {
    target.with_file_name(".knowledge-garden-update")
}

/// Removes what an earlier update left: the old copy and the staging folder.
pub fn clean_up() {
    if let Some(target) = install_target() {
        let _ = remove(&old_path(&target.path));
        let _ = remove(&staging_dir(&target.path));
    }
}

fn remove(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Downloads and checks the archive, then swaps it in for the installed app. `progress` gets
/// the bytes received so far.
pub async fn download_and_install(
    asset: ReleaseAsset,
    target: InstallTarget,
    progress: impl Fn(u64) + Send + 'static,
) -> Result<(), String> {
    let staging = staging_dir(&target.path);
    let _ = remove(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("{} を作れませんでした: {e}", staging.display()))?;
    let archive = staging.join(&asset.name);
    download(&asset, &archive, progress).await?;
    let extracted = staging.join("extracted");
    std::fs::create_dir_all(&extracted).map_err(|e| e.to_string())?;
    extract(&archive, &extracted).await?;
    let new = find(&extracted, &target.archive_entry)
        .ok_or_else(|| format!("アーカイブに {} が見つかりません", target.archive_entry))?;
    swap(&new, &target.path)?;
    let _ = remove(&staging);
    Ok(())
}

async fn download(asset: &ReleaseAsset, to: &Path, progress: impl Fn(u64)) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let response = http()?
        .get(&asset.url)
        .timeout(Duration::from_secs(30 * 60))
        .send()
        .await
        .map_err(|e| format!("ダウンロードできませんでした: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("ダウンロードできませんでした（HTTP {}）", response.status()));
    }
    let mut file = tokio::fs::File::create(to).await.map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|e| format!("ダウンロードが途中で切れました: {e}"))?;
        received += chunk.len() as u64;
        if received > asset.size {
            return Err("ダウンロードしたファイルが大きすぎます".into());
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        progress(received);
    }
    file.sync_all().await.map_err(|e| e.to_string())?;
    if received != asset.size || hex::encode(hasher.finalize()) != asset.sha256 {
        return Err("ダウンロードしたファイルの大きさか SHA-256 がリリースと合いません".into());
    }
    Ok(())
}

/// Unpacks with the system's `tar`, which reads `.tar.gz` everywhere and `.zip` on Windows
/// (bsdtar), and refuses entries that would land outside `into`.
async fn extract(archive: &Path, into: &Path) -> Result<(), String> {
    let status = tokio::process::Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .status()
        .await
        .map_err(|e| format!("tar を実行できませんでした: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("アーカイブを展開できませんでした（tar: {status}）"))
    }
}

/// The file or bundle named `name` at the top of `dir` or one folder down.
fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.exists() {
        return Some(direct);
    }
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path().join(name))
        .find(|p| p.exists())
}

/// Puts `new` where `target` is, keeping the running copy as `<target>.old` until the next
/// start. Renaming a running executable works on Unix and on Windows.
fn swap(new: &Path, target: &Path) -> Result<(), String> {
    let old = old_path(target);
    let _ = remove(&old);
    std::fs::rename(target, &old).map_err(|e| format!("今の版を退避できませんでした: {e}"))?;
    if let Err(e) = std::fs::rename(new, target) {
        let _ = std::fs::rename(&old, target);
        return Err(format!("新しい版を置けませんでした: {e}"));
    }
    Ok(())
}

/// Starts the installed (new) version.
pub fn relaunch(target: &InstallTarget) -> std::io::Result<()> {
    if cfg!(target_os = "macos") && target.path.extension().is_some_and(|e| e == "app") {
        std::process::Command::new("open").arg("-n").arg(&target.path).spawn()?;
    } else {
        std::process::Command::new(&target.path).spawn()?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    /// Newer, but this copy cannot update itself: the release page is offered.
    Manual(Available),
    Downloading {
        available: Available,
        received: u64,
    },
    /// Installed; it runs from the next start.
    Ready(Available),
    Failed(String),
}

/// Checks for and installs updates, and tells the window how it goes.
pub struct Updater {
    state: UpdateState,
    target: Option<InstallTarget>,
    _timer: Option<Task<()>>,
}

impl Updater {
    /// With `automatic`, checks shortly after start and then every few hours, and installs
    /// what it finds.
    pub fn new(automatic: bool, cx: &mut Context<Self>) -> Self {
        let target = install_target();
        let timer = automatic.then(|| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(FIRST_CHECK).await;
                loop {
                    if this.update(cx, |this, cx| this.check(cx)).is_err() {
                        break;
                    }
                    cx.background_executor().timer(CHECK_EVERY).await;
                }
            })
        });
        Self {
            state: UpdateState::Idle,
            target,
            _timer: timer,
        }
    }

    pub fn state(&self) -> &UpdateState {
        &self.state
    }

    /// Whether this copy can replace itself.
    pub fn installable(&self) -> bool {
        self.target.is_some()
    }

    pub fn check(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.state,
            UpdateState::Checking | UpdateState::Downloading { .. } | UpdateState::Ready(_)
        ) {
            return;
        }
        self.state = UpdateState::Checking;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = runtime::run(check()).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(None) => this.state = UpdateState::UpToDate,
                    Ok(Some(available)) => match (available.asset.clone(), this.target.clone()) {
                        (Some(asset), Some(target)) => this.install(available, asset, target, cx),
                        _ => this.state = UpdateState::Manual(available),
                    },
                    Err(e) => this.state = UpdateState::Failed(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn install(&mut self, available: Available, asset: ReleaseAsset, target: InstallTarget, cx: &mut Context<Self>) {
        self.state = UpdateState::Downloading {
            available: available.clone(),
            received: 0,
        };
        let (sender, mut receiver) = futures::channel::mpsc::unbounded::<u64>();
        cx.spawn(async move |this, cx| {
            while let Some(received) = receiver.next().await {
                let alive = this.update(cx, |this, cx| {
                    if let UpdateState::Downloading { received: r, .. } = &mut this.state {
                        *r = received;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
                cx.background_executor().timer(Duration::from_millis(250)).await;
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = runtime::run(download_and_install(asset, target, move |n| {
                sender.unbounded_send(n).ok();
            }))
            .await;
            this.update(cx, |this, cx| {
                this.state = match result {
                    Ok(()) => {
                        tracing::info!(target: "knowledge_garden::desktop", "installed version {}", available.version);
                        UpdateState::Ready(available)
                    }
                    Err(e) => UpdateState::Failed(e),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Starts the new version; the caller quits this one.
    pub fn relaunch(&self) -> Result<(), String> {
        let target = self.target.as_ref().ok_or("この版は自分で更新できません")?;
        relaunch(target).map_err(|e| format!("新しい版を起動できませんでした: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, assets: &[(&str, u64, Option<&str>)]) -> Release {
        Release {
            tag_name: tag.into(),
            draft: false,
            prerelease: false,
            assets: assets
                .iter()
                .map(|(name, size, digest)| AssetResponse {
                    name: name.to_string(),
                    size: *size,
                    digest: digest.map(str::to_string),
                    browser_download_url: format!(
                        "https://github.com/uthree/knowledge-garden/releases/download/{tag}/{name}"
                    ),
                })
                .collect(),
        }
    }

    const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789ABCDEF";

    #[test]
    fn finds_the_newest_stable_desktop_release_and_its_archive() {
        let current = Version::parse("0.1.0").unwrap();
        let suffix = "linux-x86_64.tar.gz";
        let mut beta = release("desktop-v0.4.0-beta.1", &[]);
        beta.prerelease = true;
        let releases = vec![
            // The server's own releases are not the desktop's.
            release("v9.0.0", &[]),
            beta,
            release(
                "desktop-v0.3.0",
                &[(
                    "knowledge-garden-desktop-v0.3.0-linux-x86_64.tar.gz",
                    1000,
                    Some(DIGEST),
                )],
            ),
            release("desktop-v0.2.0", &[]),
        ];
        let found = newest(&releases, &current, Some(suffix)).unwrap().unwrap();
        assert_eq!(found.version, Version::parse("0.3.0").unwrap());
        let asset = found.asset.unwrap();
        assert_eq!(asset.sha256, DIGEST[7..].to_ascii_lowercase());
        assert_eq!(found.tag, "desktop-v0.3.0");

        assert_eq!(
            newest(&releases, &Version::parse("0.3.0").unwrap(), Some(suffix)).unwrap(),
            None
        );
        // Another platform's archive only: the release page instead.
        let other = newest(&releases, &current, Some("windows-x86_64.zip"))
            .unwrap()
            .unwrap();
        assert_eq!(other.asset, None);
        assert_eq!(
            other.page(),
            "https://github.com/uthree/knowledge-garden/releases/tag/desktop-v0.3.0"
        );
    }

    #[test]
    fn distrusts_odd_release_metadata() {
        let current = Version::parse("0.1.0").unwrap();
        let name = "knowledge-garden-desktop-v0.2.0-linux-x86_64.tar.gz";
        let suffix = Some("linux-x86_64.tar.gz");
        let mut moved = release("desktop-v0.2.0", &[(name, 10, Some(DIGEST))]);
        moved.assets[0].browser_download_url = "https://example.com/evil.tar.gz".into();
        assert!(newest(&[moved], &current, suffix).is_err());
        let huge = release("desktop-v0.2.0", &[(name, MAX_ARCHIVE_BYTES + 1, Some(DIGEST))]);
        assert!(newest(&[huge], &current, suffix).is_err());
        let twice = release("desktop-v0.2.0", &[(name, 10, Some(DIGEST)), (name, 10, Some(DIGEST))]);
        assert!(newest(&[twice], &current, suffix).is_err());
        let undigested = release("desktop-v0.2.0", &[(name, 10, None)]);
        assert_eq!(newest(&[undigested], &current, suffix).unwrap().unwrap().asset, None);
    }

    #[test]
    fn only_a_released_copy_updates_itself() {
        let home = Path::new("/home/me/.cargo");
        assert_eq!(
            target_for(
                Path::new("/src/kg/desktop/target/release/knowledge-garden-desktop"),
                Some(home)
            ),
            None
        );
        assert_eq!(
            target_for(Path::new("/home/me/.cargo/bin/knowledge-garden-desktop"), Some(home)),
            None
        );
        let installed = target_for(Path::new("/opt/kg/knowledge-garden-desktop"), Some(home)).unwrap();
        assert_eq!(installed.path, Path::new("/opt/kg/knowledge-garden-desktop"));
        assert_eq!(installed.archive_entry, "knowledge-garden-desktop");
    }

    #[test]
    fn swaps_in_the_new_copy_and_keeps_the_old_one_aside() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("app");
        std::fs::write(&target, "old").unwrap();
        let new = dir.path().join("new");
        std::fs::write(&new, "new").unwrap();
        swap(&new, &target).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(old_path(&target)).unwrap(), "old");
        // A failed swap puts the running copy back.
        assert!(swap(&dir.path().join("missing"), &target).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    }

    #[test]
    fn downloads_checks_and_installs_an_archive() {
        let dir = tempfile::tempdir().unwrap();
        // An archive holding a folder with the new executable, as the release workflow makes it.
        let src = dir.path().join("src/knowledge-garden-desktop-v0.2.0-linux-x86_64");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("knowledge-garden-desktop"), "new build").unwrap();
        let archive = dir.path().join("a.tar.gz");
        let ok = std::process::Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(dir.path().join("src"))
            .arg(".")
            .status()
            .unwrap();
        assert!(ok.success());
        let bytes = std::fs::read(&archive).unwrap();
        let sha256 = hex::encode(Sha256::digest(&bytes));

        let install = dir.path().join("install");
        std::fs::create_dir_all(&install).unwrap();
        let target = InstallTarget {
            path: install.join("knowledge-garden-desktop"),
            archive_entry: "knowledge-garden-desktop".into(),
        };
        std::fs::write(&target.path, "running build").unwrap();

        runtime::runtime().block_on(async {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let served = bytes.clone();
            tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 4096];
                let _ = socket.read(&mut buf).await.unwrap();
                let head = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n", served.len());
                socket.write_all(head.as_bytes()).await.unwrap();
                socket.write_all(&served).await.unwrap();
            });
            let asset = ReleaseAsset {
                name: "a.tar.gz".into(),
                url: format!("http://{addr}/a.tar.gz"),
                size: bytes.len() as u64,
                sha256: "0".repeat(64),
            };
            // The client refuses plain HTTP, as it must for GitHub; test the pieces below it.
            assert!(
                download_and_install(asset.clone(), target.clone(), |_| {})
                    .await
                    .is_err()
            );
            assert_eq!(std::fs::read_to_string(&target.path).unwrap(), "running build");

            let staging = staging_dir(&target.path);
            let extracted = staging.join("extracted");
            std::fs::create_dir_all(&extracted).unwrap();
            let copy = staging.join("a.tar.gz");
            std::fs::write(&copy, &bytes).unwrap();
            extract(&copy, &extracted).await.unwrap();
            let new = find(&extracted, &target.archive_entry).unwrap();
            swap(&new, &target.path).unwrap();
            assert_eq!(std::fs::read_to_string(&target.path).unwrap(), "new build");
            assert_eq!(sha256.len(), 64);
        });
    }
}
