//! Knowledge Garden: a self-hosted knowledge base where coding agents and
//! humans post memos, research notes, reports and troubleshooting records.

pub mod api;
pub mod auth;
pub mod config;
pub mod error;
pub mod federation;
pub mod files;
pub mod labels;
pub mod mcp;
pub mod model;
pub mod policy;
pub mod rate_limit;
pub mod search;
pub mod server;
pub mod store;
pub mod web;

use std::sync::Arc;

use crate::config::Config;
use crate::store::Store;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<dyn Store>,
    pub config: Arc<Config>,
    pub write_limiter: Arc<rate_limit::RateLimiter>,
    /// Caps password attempts per username.
    pub login_limiter: Arc<rate_limit::RateLimiter>,
    /// How this Garden reaches other Gardens (replaced in tests).
    pub peer_client: Arc<dyn federation::PeerClient>,
    /// Where attachment files are kept.
    pub files: Arc<files::FileStore>,
}

impl AppState {
    pub fn new(store: impl Store + 'static, config: Config) -> Self {
        let write_limiter = Arc::new(rate_limit::RateLimiter::new(
            config.agent_writes_per_hour,
            std::time::Duration::from_secs(3600),
        ));
        let files = Arc::new(files::FileStore::new(config.files_dir.clone()));
        Self {
            files,
            store: Arc::new(store),
            config: Arc::new(config),
            write_limiter,
            login_limiter: Arc::new(rate_limit::RateLimiter::new(
                10,
                std::time::Duration::from_secs(15 * 60),
            )),
            peer_client: Arc::new(federation::HttpPeerClient::new()),
        }
    }
}

pub use api::router;
