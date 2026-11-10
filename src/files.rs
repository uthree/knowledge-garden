//! Attachment files: content-addressed blobs on disk, and the rules for their
//! names and media types.
//!
//! A blob is stored once under `<root>/<first two hex digits>/<sha256>`, however
//! many entries attach it. Uploads are written to `<root>/tmp` while they are
//! hashed and then renamed into place, so a half-written file is never served.

use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::error::{Error, Result};

pub const MAX_NAME_CHARS: usize = 200;
/// At most this many files per entry.
pub const MAX_ATTACHMENTS_PER_ENTRY: i64 = 100;

pub struct FileStore {
    root: PathBuf,
}

/// A blob that has been written to the store.
#[derive(Debug, Clone)]
pub struct StoredBlob {
    pub sha256: String,
    pub size: u64,
}

pub fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn too_large(max: u64) -> Error {
    Error::PayloadTooLarge(format!(
        "the file is larger than this Garden's limit of {} MB",
        max.div_ceil(1024 * 1024)
    ))
}

impl FileStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn blob_path(&self, sha256: &str) -> PathBuf {
        debug_assert!(is_sha256(sha256));
        self.root.join(&sha256[..2]).join(sha256)
    }

    /// Starts writing a new blob of at most `max` bytes.
    pub async fn writer(&self, max: u64) -> Result<BlobWriter> {
        let tmp_dir = self.root.join("tmp");
        tokio::fs::create_dir_all(&tmp_dir).await.map_err(Error::internal)?;
        let tmp = tmp_dir.join(uuid::Uuid::now_v7().to_string());
        let file = tokio::fs::File::create(&tmp).await.map_err(Error::internal)?;
        Ok(BlobWriter {
            root: self.root.clone(),
            tmp,
            file: Some(file),
            hasher: Sha256::new(),
            size: 0,
            max,
        })
    }

    /// Stores bytes that are already in memory.
    pub async fn put(&self, bytes: &[u8], max: u64) -> Result<StoredBlob> {
        let mut w = self.writer(max).await?;
        w.write(bytes).await?;
        w.finish().await
    }

    /// Opens a blob for reading, with its size.
    pub async fn open(&self, sha256: &str) -> Result<Option<(tokio::fs::File, u64)>> {
        if !is_sha256(sha256) {
            return Ok(None);
        }
        match tokio::fs::File::open(self.blob_path(sha256)).await {
            Ok(f) => {
                let len = f.metadata().await.map_err(Error::internal)?.len();
                Ok(Some((f, len)))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::internal(e)),
        }
    }

    pub async fn exists(&self, sha256: &str) -> bool {
        is_sha256(sha256) && tokio::fs::try_exists(self.blob_path(sha256)).await.unwrap_or(false)
    }

    pub async fn read(&self, sha256: &str) -> Result<Option<Vec<u8>>> {
        if !is_sha256(sha256) {
            return Ok(None);
        }
        match tokio::fs::read(self.blob_path(sha256)).await {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::internal(e)),
        }
    }

    /// Deletes a blob that no attachment uses any more.
    pub async fn remove(&self, sha256: &str) {
        if !is_sha256(sha256) {
            return;
        }
        if let Err(e) = tokio::fs::remove_file(self.blob_path(sha256)).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("could not delete blob {sha256}: {e}");
        }
    }
}

/// Writes a blob while hashing it. Dropping it without `finish` discards the file.
pub struct BlobWriter {
    root: PathBuf,
    tmp: PathBuf,
    file: Option<tokio::fs::File>,
    hasher: Sha256,
    size: u64,
    max: u64,
}

impl BlobWriter {
    pub async fn write(&mut self, chunk: &[u8]) -> Result<()> {
        self.size += chunk.len() as u64;
        if self.size > self.max {
            return Err(too_large(self.max));
        }
        self.hasher.update(chunk);
        let file = self.file.as_mut().expect("writer is open");
        file.write_all(chunk).await.map_err(Error::internal)
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub async fn finish(mut self) -> Result<StoredBlob> {
        let mut file = self.file.take().expect("writer is open");
        file.flush().await.map_err(Error::internal)?;
        file.sync_all().await.map_err(Error::internal)?;
        drop(file);
        let sha256 = hex::encode(std::mem::take(&mut self.hasher).finalize());
        let dir = self.root.join(&sha256[..2]);
        tokio::fs::create_dir_all(&dir).await.map_err(Error::internal)?;
        let dest = dir.join(&sha256);
        if tokio::fs::try_exists(&dest).await.unwrap_or(false) {
            // Same content is already stored.
            let _ = tokio::fs::remove_file(&self.tmp).await;
        } else {
            tokio::fs::rename(&self.tmp, &dest).await.map_err(Error::internal)?;
        }
        self.tmp = PathBuf::new();
        Ok(StoredBlob {
            sha256,
            size: self.size,
        })
    }
}

impl Drop for BlobWriter {
    fn drop(&mut self) {
        if !self.tmp.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.tmp);
        }
    }
}

// ---------------------------------------------------------------- names

fn forbidden_char(c: char) -> bool {
    c.is_control() || c.is_whitespace() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '#' | '%')
}

/// Checks a file name given through the API. Names are used in URLs and in
/// Markdown (`![plot](attachment:plot.png)`), so they cannot contain spaces,
/// slashes or characters that need escaping there.
pub fn validate_name(name: &str) -> std::result::Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a file name is required".into());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!("file names are at most {MAX_NAME_CHARS} characters"));
    }
    if name.starts_with('.') {
        return Err("file names must not start with '.'".into());
    }
    if let Some(c) = name.chars().find(|c| forbidden_char(*c)) {
        return Err(format!(
            "file names must not contain {c:?}: use letters, digits, '-', '_' and '.' (e.g. loss-curve.png)"
        ));
    }
    Ok(name.to_string())
}

/// Turns a name from a browser upload (which may contain spaces or a path)
/// into a valid name, e.g. `Screen Shot 1.png` → `Screen-Shot-1.png`.
pub fn sanitize_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw).trim();
    let mut out = String::new();
    for c in base.chars() {
        let c = if forbidden_char(c) { '-' } else { c };
        if c == '-' && out.ends_with('-') {
            continue;
        }
        out.push(c);
    }
    let out = out.trim_start_matches(['.', '-']).trim_end_matches('-');
    let mut out: String = out.chars().take(MAX_NAME_CHARS).collect();
    if out.is_empty() {
        out = "file".into();
    }
    out
}

/// Percent-encodes a file name for a URL path segment.
pub fn encode_path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ---------------------------------------------------------------- media types

/// How a file can be shown inline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Preview {
    Image,
    Audio,
    Video,
    Pdf,
    /// Plain text (logs, CSV, JSON, source code): shown as text in the browser.
    Text,
    /// Downloaded, not shown.
    None,
}

impl Preview {
    pub fn of(content_type: &str) -> Self {
        let ct = content_type.split(';').next().unwrap_or("").trim();
        if ct == "application/pdf" {
            Preview::Pdf
        } else if ct.starts_with("image/") {
            Preview::Image
        } else if ct.starts_with("audio/") {
            Preview::Audio
        } else if ct.starts_with("video/") {
            Preview::Video
        } else if ct.starts_with("text/") || matches!(ct, "application/json" | "application/x-ndjson") {
            Preview::Text
        } else {
            Preview::None
        }
    }

    /// Whether Markdown should embed it (`![..](attachment:..)`) rather than link to it.
    pub fn embeds(self) -> bool {
        matches!(self, Preview::Image | Preview::Audio | Preview::Video | Preview::Pdf)
    }
}

/// Media types this Garden serves, by file extension. Anything else is served as
/// `application/octet-stream` and downloaded, never rendered by the browser
/// (HTML and other active content cannot be smuggled in as an attachment).
const TYPES: &[(&[&str], &str)] = &[
    (&["png"], "image/png"),
    (&["jpg", "jpeg"], "image/jpeg"),
    (&["gif"], "image/gif"),
    (&["webp"], "image/webp"),
    (&["avif"], "image/avif"),
    (&["svg"], "image/svg+xml"),
    (&["bmp"], "image/bmp"),
    (&["mp3"], "audio/mpeg"),
    (&["m4a"], "audio/mp4"),
    (&["aac"], "audio/aac"),
    (&["wav"], "audio/wav"),
    (&["ogg", "oga", "opus"], "audio/ogg"),
    (&["flac"], "audio/flac"),
    (&["weba"], "audio/webm"),
    (&["mp4", "m4v"], "video/mp4"),
    (&["webm"], "video/webm"),
    (&["ogv"], "video/ogg"),
    (&["mov"], "video/quicktime"),
    (&["pdf"], "application/pdf"),
    (
        &[
            "txt", "log", "out", "err", "diff", "patch", "rs", "py", "js", "ts", "go", "c", "h", "cpp", "hpp", "java",
            "kt", "rb", "sh", "sql", "toml", "ini", "cfg", "conf", "lock",
        ],
        "text/plain",
    ),
    (&["md", "markdown"], "text/markdown"),
    (&["csv"], "text/csv"),
    (&["tsv"], "text/tab-separated-values"),
    (&["yaml", "yml"], "text/yaml"),
    (&["json"], "application/json"),
    (&["jsonl", "ndjson"], "application/x-ndjson"),
    (&["zip"], "application/zip"),
    (&["gz", "tgz"], "application/gzip"),
    (&["tar"], "application/x-tar"),
    (&["zst"], "application/zstd"),
    (&["parquet"], "application/vnd.apache.parquet"),
    (&["ipynb"], "application/x-ipynb+json"),
];

pub const OCTET_STREAM: &str = "application/octet-stream";

/// The media type a file is served with: from its extension, else the type the
/// uploader declared if it is one we know, else `application/octet-stream`.
pub fn content_type_for(name: &str, declared: Option<&str>) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    if let Some(ext) = ext
        && let Some((_, ty)) = TYPES.iter().find(|(exts, _)| exts.contains(&ext.as_str()))
    {
        return ty;
    }
    let declared = declared
        .and_then(|d| d.split(';').next())
        .map(|d| d.trim().to_ascii_lowercase());
    declared
        .and_then(|d| TYPES.iter().map(|(_, ty)| *ty).find(|ty| *ty == d))
        .unwrap_or(OCTET_STREAM)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(validate_name(" plot.png ").unwrap(), "plot.png");
        assert_eq!(validate_name("損失曲線.png").unwrap(), "損失曲線.png");
        for bad in ["", ".env", "a/b.png", "a b.png", "x?.png", "a%2F.png", "..", "a\\b"] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
        assert_eq!(sanitize_name("C:\\Users\\me\\Screen Shot 1.png"), "Screen-Shot-1.png");
        assert_eq!(sanitize_name("../../.env"), "env");
        assert_eq!(sanitize_name("???"), "file");
        assert!(validate_name(&sanitize_name("a  b#c.txt")).is_ok());
    }

    #[test]
    fn types_and_previews() {
        assert_eq!(content_type_for("a.PNG", None), "image/png");
        assert_eq!(content_type_for("run.log", Some("text/html")), "text/plain");
        assert_eq!(content_type_for("page.html", Some("text/html")), OCTET_STREAM);
        assert_eq!(content_type_for("blob", Some("audio/mpeg")), "audio/mpeg");
        assert_eq!(content_type_for("blob", None), OCTET_STREAM);
        assert_eq!(Preview::of("image/svg+xml"), Preview::Image);
        assert_eq!(Preview::of("application/pdf"), Preview::Pdf);
        assert_eq!(Preview::of("video/mp4"), Preview::Video);
        assert_eq!(Preview::of("text/csv"), Preview::Text);
        assert_eq!(Preview::of(OCTET_STREAM), Preview::None);
    }

    #[tokio::test]
    async fn blobs_are_content_addressed() {
        let dir = std::env::temp_dir().join(format!("kg-files-{}", uuid::Uuid::now_v7()));
        let store = FileStore::new(&dir);
        let a = store.put(b"hello", 100).await.unwrap();
        let b = store.put(b"hello", 100).await.unwrap();
        assert_eq!(a.sha256, b.sha256);
        assert_eq!(
            a.sha256,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(store.read(&a.sha256).await.unwrap().unwrap(), b"hello");
        assert!(matches!(
            store.put(b"too long", 3).await,
            Err(Error::PayloadTooLarge(_))
        ));
        // Nothing is left behind in tmp.
        assert_eq!(std::fs::read_dir(dir.join("tmp")).unwrap().count(), 0);
        store.remove(&a.sha256).await;
        assert!(!store.exists(&a.sha256).await);
        assert!(store.open("../../etc/passwd").await.unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
