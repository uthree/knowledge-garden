//! Federation between Gardens by subscription-style mirroring.
//!
//! - Every Garden has an ed25519 key, published at `/.well-known/knowledge-garden`.
//! - An admin approves a peer by its base URL; its public key is pinned at that moment.
//! - A Garden serves its change feed (`/federation/v1/feed`) only to approved peers,
//!   which prove who they are by signing the request with their key.
//! - The feed carries `public` and `federated` entries, each signed by the origin,
//!   and withdrawal notices for entries that stopped being shareable.
//! - The subscriber verifies every item and stores it as a read-only mirror, which then
//!   shows up in its own lists and searches.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::http::HeaderMap;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AppState;
use crate::error::{Error, Result};
use crate::model::{AuthorRef, EntryKind, PostedVia, Relation, Status, Visibility, normalize_tags};
use crate::store::{MirrorEntry, MirrorOutcome, Peer, PeerStatus, RawLink, VersionContent};

pub const WELL_KNOWN_PATH: &str = "/.well-known/knowledge-garden";
pub const FEED_PATH: &str = "/federation/v1/feed";
pub const HEADER_SIGNER: &str = "kg-signer";
pub const HEADER_DATE: &str = "kg-date";
pub const HEADER_SIGNATURE: &str = "kg-signature";
/// How far a signed request's date may be from our clock.
pub const MAX_CLOCK_SKEW_SECS: i64 = 300;
pub const FEED_PAGE_SIZE: i64 = 100;
/// Safety limit on pages fetched from one peer in one sync.
const MAX_PAGES_PER_SYNC: usize = 100;

// ---------------------------------------------------------------- keys and signatures

pub struct GardenKey(SigningKey);

impl GardenKey {
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self(SigningKey::from_bytes(seed))
    }

    pub async fn load(state: &AppState) -> Result<Self> {
        Ok(Self::from_seed(&state.store.garden_key_seed().await?))
    }

    pub fn public_key(&self) -> String {
        STANDARD.encode(self.0.verifying_key().to_bytes())
    }

    pub fn sign(&self, message: &[u8]) -> String {
        STANDARD.encode(self.0.sign(message).to_bytes())
    }
}

pub fn parse_public_key(b64: &str) -> Option<VerifyingKey> {
    let bytes: [u8; 32] = STANDARD.decode(b64).ok()?.try_into().ok()?;
    VerifyingKey::from_bytes(&bytes).ok()
}

pub fn verify(public_key: &str, message: &[u8], signature: &str) -> bool {
    let Some(key) = parse_public_key(public_key) else {
        return false;
    };
    let Some(sig) = STANDARD
        .decode(signature)
        .ok()
        .and_then(|b| <[u8; 64]>::try_from(b).ok())
    else {
        return false;
    };
    key.verify_strict(message, &Signature::from_bytes(&sig)).is_ok()
}

/// The bytes signed for a peer request.
fn request_message(method: &str, path_and_query: &str, date: &str, signer: &str) -> String {
    format!("{method}\n{path_and_query}\n{date}\n{signer}")
}

/// Headers that identify this Garden on a request to a peer.
pub fn sign_request(key: &GardenKey, signer: &str, method: &str, path_and_query: &str) -> Vec<(String, String)> {
    let date = Utc::now().to_rfc3339();
    let signature = key.sign(request_message(method, path_and_query, &date, signer).as_bytes());
    vec![
        (HEADER_SIGNER.into(), signer.into()),
        (HEADER_DATE.into(), date),
        (HEADER_SIGNATURE.into(), signature),
    ]
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Checks a signed peer request and returns the approved peer that sent it.
pub async fn verify_request(state: &AppState, headers: &HeaderMap, method: &str, path_and_query: &str) -> Result<Peer> {
    let unauthorized = |m: &str| Error::Unauthorized(m.to_string());
    let (Some(signer), Some(date), Some(signature)) = (
        header(headers, HEADER_SIGNER),
        header(headers, HEADER_DATE),
        header(headers, HEADER_SIGNATURE),
    ) else {
        return Err(unauthorized(
            "the feed is only served to approved Gardens: sign the request",
        ));
    };
    let sent = DateTime::parse_from_rfc3339(date).map_err(|_| unauthorized("invalid kg-date"))?;
    if (Utc::now() - sent.with_timezone(&Utc)).num_seconds().abs() > MAX_CLOCK_SKEW_SECS {
        return Err(unauthorized("kg-date is too far from the current time"));
    }
    let peer = state
        .store
        .find_peer(signer.trim_end_matches('/'))
        .await?
        .filter(|p| p.status == PeerStatus::Approved)
        .ok_or_else(|| Error::forbidden("this Garden has not approved the signer"))?;
    if !verify(
        &peer.public_key,
        request_message(method, path_and_query, date, signer).as_bytes(),
        signature,
    ) {
        return Err(unauthorized("invalid signature"));
    }
    Ok(peer)
}

// ---------------------------------------------------------------- feed format

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedLink {
    pub relation: Relation,
    pub target_uri: String,
    pub target_version: Option<i64>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedEntry {
    pub kind: EntryKind,
    pub visibility: Visibility,
    pub status: Status,
    pub title: String,
    pub summary: Option<String>,
    pub body: String,
    pub lang: Option<String>,
    pub tags: Vec<String>,
    pub provenance: Value,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub version_created_at: DateTime<Utc>,
    pub author: AuthorRef,
    pub posted_via: PostedVia,
    pub links: Vec<FeedLink>,
}

/// One change in a Garden's feed. `signature` covers the canonical JSON of
/// every other field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedItem {
    pub uri: String,
    pub origin: String,
    pub updated_at: String,
    /// The entry is no longer shared; subscribers should hide their copy.
    pub withdrawn: bool,
    pub entry: Option<FeedEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FeedPage {
    pub items: Vec<FeedItem>,
    /// Pass as `after` to continue; absent when there were no items.
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

/// Canonical bytes of an item for signing: JSON with object keys sorted.
pub fn canonical_bytes(item: &FeedItem) -> Vec<u8> {
    let mut unsigned = item.clone();
    unsigned.signature = None;
    // serde_json's Value keeps object keys sorted, which makes this canonical.
    let value = serde_json::to_value(&unsigned).expect("feed items serialize");
    serde_json::to_vec(&value).expect("values serialize")
}

pub fn encode_cursor(updated_at: &str, id: &str) -> String {
    URL_SAFE_NO_PAD.encode(format!("{updated_at}|{id}"))
}

pub fn decode_cursor(cursor: &str) -> Option<(String, String)> {
    let raw = String::from_utf8(URL_SAFE_NO_PAD.decode(cursor).ok()?).ok()?;
    let (updated_at, id) = raw.split_once('|')?;
    Some((updated_at.to_string(), id.to_string()))
}

/// Builds one page of this Garden's feed.
pub async fn build_feed(state: &AppState, key: &GardenKey, after: Option<&str>, limit: i64) -> Result<FeedPage> {
    let after = match after {
        Some(c) => Some(decode_cursor(c).ok_or_else(|| Error::bad_request("invalid cursor"))?),
        None => None,
    };
    let rows = state
        .store
        .feed_rows(after.as_ref().map(|(u, i)| (u.as_str(), i.as_str())), limit + 1)
        .await?;
    let has_more = rows.len() as i64 > limit;
    let rows = &rows[..rows.len().min(limit as usize)];
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let entry = if row.visibility.is_shareable() {
            let e = state.store.get_entry(&row.id, None).await?.ok_or(Error::NotFound)?;
            let links = state.store.raw_links(&row.id).await?;
            Some(FeedEntry {
                kind: e.kind,
                visibility: e.visibility,
                status: e.status,
                title: e.title,
                summary: e.summary,
                body: e.body,
                lang: e.lang,
                tags: e.tags,
                provenance: e.provenance,
                version: e.current_version,
                created_at: e.created_at,
                version_created_at: e.version_created_at,
                author: e.author,
                posted_via: e.posted_via,
                links: links
                    .into_iter()
                    .map(|l| FeedLink {
                        relation: l.relation,
                        target_uri: l.target_uri,
                        target_version: l.target_version,
                        note: l.note,
                    })
                    .collect(),
            })
        } else {
            None
        };
        let mut item = FeedItem {
            uri: row.uri.clone(),
            origin: state.config.base_url.clone(),
            updated_at: row.updated_at.clone(),
            withdrawn: entry.is_none(),
            entry,
            signature: None,
        };
        item.signature = Some(key.sign(&canonical_bytes(&item)));
        items.push(item);
    }
    let next_cursor = rows.last().map(|r| encode_cursor(&r.updated_at, &r.id));
    Ok(FeedPage {
        items,
        next_cursor,
        has_more,
    })
}

// ---------------------------------------------------------------- talking to peers

#[derive(Debug)]
pub struct PeerResponse {
    pub status: u16,
    pub body: Value,
}

/// HTTP access to other Gardens. Abstracted so tests can connect Gardens in memory.
#[async_trait]
pub trait PeerClient: Send + Sync {
    async fn get(&self, url: &str, headers: Vec<(String, String)>) -> Result<PeerResponse>;
}

pub struct HttpPeerClient(reqwest::Client);

impl HttpPeerClient {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("knowledge-garden/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("HTTP client builds");
        Self(client)
    }
}

impl Default for HttpPeerClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PeerClient for HttpPeerClient {
    async fn get(&self, url: &str, headers: Vec<(String, String)>) -> Result<PeerResponse> {
        let mut req = self.0.get(url);
        for (k, v) in headers {
            req = req.header(k, v);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| Error::BadRequest(format!("could not reach {url}: {e}")))?;
        let status = resp.status().as_u16();
        let body = resp.json().await.unwrap_or(Value::Null);
        Ok(PeerResponse { status, body })
    }
}

pub fn normalize_base_url(url: &str) -> Result<String> {
    let url = url.trim().trim_end_matches('/');
    let parsed = url::Url::parse(url).map_err(|_| Error::bad_request(format!("invalid URL: {url}")))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(Error::bad_request("a Garden URL must be http(s)://host[:port][/path]"));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(Error::bad_request("a Garden URL must not have a query or fragment"));
    }
    Ok(url.to_string())
}

/// Fetches a Garden's public description and checks that it is consistent.
pub async fn discover(state: &AppState, base_url: &str) -> Result<(String, String)> {
    let resp = state
        .peer_client
        .get(&format!("{base_url}{WELL_KNOWN_PATH}"), vec![])
        .await?;
    if resp.status != 200 {
        return Err(Error::bad_request(format!(
            "{base_url} is not a Knowledge Garden (status {})",
            resp.status
        )));
    }
    let claimed = resp.body["base_url"].as_str().map(|u| u.trim_end_matches('/'));
    if claimed != Some(base_url) {
        return Err(Error::bad_request(format!(
            "{base_url} describes itself as {}; register it by that URL",
            claimed.unwrap_or("nothing")
        )));
    }
    let key = resp.body["public_key"]
        .as_str()
        .filter(|k| parse_public_key(k).is_some());
    let key = key.ok_or_else(|| Error::bad_request(format!("{base_url} has no valid public key")))?;
    let name = resp.body["name"].as_str().unwrap_or(base_url).to_string();
    Ok((name, key.to_string()))
}

#[derive(Debug, Default, Serialize)]
pub struct SyncReport {
    pub received: usize,
    pub created: usize,
    pub updated: usize,
    pub withdrawn: usize,
    /// Items that failed verification or validation; they are skipped.
    pub rejected: Vec<String>,
}

fn host_of(base_url: &str) -> String {
    url::Url::parse(base_url)
        .ok()
        .and_then(|u| {
            let host = u.host_str()?.to_string();
            Some(match u.port() {
                Some(p) => format!("{host}:{p}"),
                None => host,
            })
        })
        .unwrap_or_else(|| base_url.to_string())
}

/// Checks one item from `peer` and turns it into something the store can apply.
fn validate_item(peer: &Peer, item: &FeedItem) -> std::result::Result<Option<MirrorEntry>, String> {
    let signature = item.signature.as_deref().ok_or("unsigned item")?;
    if !verify(&peer.public_key, &canonical_bytes(item), signature) {
        return Err("bad signature".into());
    }
    if item.origin != peer.base_url {
        return Err(format!("origin {} is not the peer", item.origin));
    }
    if !item.uri.starts_with(&format!("{}/entries/", peer.base_url)) {
        return Err("URI is outside the peer's namespace".into());
    }
    let updated_at = DateTime::parse_from_rfc3339(&item.updated_at)
        .map_err(|_| "invalid updated_at")?
        .with_timezone(&Utc);
    let Some(e) = &item.entry else {
        return if item.withdrawn {
            Ok(None)
        } else {
            Err("item has neither entry nor withdrawal".into())
        };
    };
    let visibility = match e.visibility {
        Visibility::Public => Visibility::Public,
        // Shared with approved Gardens only: our members may read it, and it is never passed on.
        Visibility::Federated => Visibility::Members,
        other => return Err(format!("{other} entries must not be federated")),
    };
    let title = crate::model::validate_title(&e.title)?;
    let summary = crate::model::validate_summary(e.summary.clone())?;
    crate::model::validate_body(&e.body)?;
    if !e.provenance.is_object() || e.version < 1 {
        return Err("invalid provenance or version".into());
    }
    crate::model::validate_username(&e.author.username)?;
    Ok(Some(MirrorEntry {
        origin: peer.base_url.clone(),
        uri: item.uri.clone(),
        signature: signature.to_string(),
        updated_at,
        kind: e.kind,
        visibility,
        status: e.status,
        version: e.version,
        created_at: e.created_at,
        version_created_at: e.version_created_at,
        author_username: format!("{}@{}", e.author.username, host_of(&peer.base_url)),
        author_display_name: e.author.display_name.chars().take(100).collect(),
        posted_via: PostedVia {
            name: e.posted_via.name.chars().take(64).collect(),
            kind: e.posted_via.kind,
        },
        tags: normalize_tags(&e.tags)?,
        content: VersionContent {
            title,
            summary,
            body: e.body.clone(),
            lang: e.lang.clone(),
            provenance: e.provenance.clone(),
        },
        links: e
            .links
            .iter()
            .filter(|l| l.target_uri.starts_with("http://") || l.target_uri.starts_with("https://"))
            .map(|l| RawLink {
                relation: l.relation,
                target_uri: l.target_uri.clone(),
                target_version: l.target_version,
                note: l.note.clone(),
            })
            .collect(),
    }))
}

/// Pulls everything new from one approved peer.
pub async fn sync_peer(state: &AppState, peer: &Peer) -> Result<SyncReport> {
    if peer.status != PeerStatus::Approved {
        return Err(Error::bad_request("only approved peers are synced"));
    }
    let key = GardenKey::load(state).await?;
    let mut report = SyncReport::default();
    let mut cursor = peer.cursor.clone();
    let result: Result<()> = async {
        for _ in 0..MAX_PAGES_PER_SYNC {
            let query = {
                // The serializer is not Send, so it must not live across an await.
                let mut q = url::form_urlencoded::Serializer::new(String::new());
                q.append_pair("limit", &FEED_PAGE_SIZE.to_string());
                if let Some(c) = &cursor {
                    q.append_pair("after", c);
                }
                q.finish()
            };
            let path = format!("{FEED_PATH}?{query}");
            let headers = sign_request(&key, &state.config.base_url, "GET", &path);
            let resp = state
                .peer_client
                .get(&format!("{}{path}", peer.base_url), headers)
                .await?;
            if resp.status != 200 {
                let msg = resp.body["error"]["message"].as_str().unwrap_or("").to_string();
                return Err(Error::BadRequest(format!("feed returned {}: {msg}", resp.status)));
            }
            let page: FeedPage =
                serde_json::from_value(resp.body).map_err(|e| Error::BadRequest(format!("malformed feed: {e}")))?;
            for item in &page.items {
                report.received += 1;
                match validate_item(peer, item) {
                    Ok(Some(mirror)) => match state.store.apply_mirror(mirror).await {
                        Ok(MirrorOutcome::Created) => report.created += 1,
                        Ok(MirrorOutcome::Updated) => report.updated += 1,
                        Err(e) => report.rejected.push(format!("{}: {e}", item.uri)),
                    },
                    Ok(None) => {
                        if state.store.withdraw_mirror(&peer.base_url, &item.uri).await? {
                            report.withdrawn += 1;
                        }
                    }
                    Err(reason) => report.rejected.push(format!("{}: {reason}", item.uri)),
                }
            }
            if page.next_cursor.is_some() {
                cursor = page.next_cursor;
                state.store.record_sync(&peer.id, cursor.as_deref(), None).await?;
            }
            if !page.has_more {
                break;
            }
        }
        Ok(())
    }
    .await;
    match result {
        Ok(()) => {
            state.store.record_sync(&peer.id, cursor.as_deref(), None).await?;
            if !report.rejected.is_empty() {
                tracing::warn!(
                    "rejected {} items from {}: {:?}",
                    report.rejected.len(),
                    peer.base_url,
                    report.rejected
                );
            }
            Ok(report)
        }
        Err(e) => {
            state
                .store
                .record_sync(&peer.id, cursor.as_deref(), Some(&e.to_string()))
                .await?;
            Err(e)
        }
    }
}

/// Syncs all approved peers every `interval`, forever.
pub async fn run_sync_loop(state: AppState, interval: Duration) {
    let state = Arc::new(state);
    loop {
        match state.store.list_peers().await {
            Ok(peers) => {
                for peer in peers.iter().filter(|p| p.status == PeerStatus::Approved) {
                    match sync_peer(&state, peer).await {
                        Ok(r) if r.received > 0 => tracing::info!("synced {}: {r:?}", peer.base_url),
                        Ok(_) => {}
                        Err(e) => tracing::warn!("sync with {} failed: {e}", peer.base_url),
                    }
                }
            }
            Err(e) => tracing::error!("could not list peers: {e}"),
        }
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_round_trip_and_detect_tampering() {
        let key = GardenKey::from_seed(&[7; 32]);
        let sig = key.sign(b"hello");
        assert!(verify(&key.public_key(), b"hello", &sig));
        assert!(!verify(&key.public_key(), b"hellO", &sig));
        let other = GardenKey::from_seed(&[8; 32]);
        assert!(!verify(&other.public_key(), b"hello", &sig));
    }

    #[test]
    fn canonical_bytes_ignore_the_signature_and_sort_keys() {
        let mut item = FeedItem {
            uri: "u".into(),
            origin: "o".into(),
            updated_at: "t".into(),
            withdrawn: true,
            entry: None,
            signature: None,
        };
        let unsigned = canonical_bytes(&item);
        item.signature = Some("sig".into());
        assert_eq!(unsigned, canonical_bytes(&item));
        assert_eq!(
            String::from_utf8(unsigned).unwrap(),
            r#"{"entry":null,"origin":"o","updated_at":"t","uri":"u","withdrawn":true}"#
        );
    }

    #[test]
    fn cursors_round_trip() {
        let c = encode_cursor("2026-11-09T12:00:00.000000Z", "abc");
        assert_eq!(
            decode_cursor(&c),
            Some(("2026-11-09T12:00:00.000000Z".into(), "abc".into()))
        );
        assert_eq!(decode_cursor("!!"), None);
    }

    #[test]
    fn base_urls_are_normalized() {
        assert_eq!(normalize_base_url("https://g.example/ ").unwrap(), "https://g.example");
        assert_eq!(
            normalize_base_url("http://nas.local:8080/garden/").unwrap(),
            "http://nas.local:8080/garden"
        );
        assert!(normalize_base_url("ftp://g.example").is_err());
        assert!(normalize_base_url("https://g.example/?x=1").is_err());
        assert_eq!(host_of("http://nas.local:8080"), "nas.local:8080");
    }
}
