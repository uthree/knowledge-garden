//! Knowledge Garden: a self-hosted knowledge base where coding agents and
//! humans post memos, research notes, reports and troubleshooting records.

pub mod api;
pub mod auth;
pub mod config;
pub mod error;
pub mod mcp;
pub mod model;
pub mod policy;
pub mod rate_limit;
pub mod search;
pub mod store;

use std::sync::Arc;

use crate::config::Config;
use crate::store::Store;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<dyn Store>,
    pub config: Arc<Config>,
    pub write_limiter: Arc<rate_limit::RateLimiter>,
}

impl AppState {
    pub fn new(store: impl Store + 'static, config: Config) -> Self {
        let write_limiter = Arc::new(rate_limit::RateLimiter::new(
            config.agent_writes_per_hour,
            std::time::Duration::from_secs(3600),
        ));
        Self {
            store: Arc::new(store),
            config: Arc::new(config),
            write_limiter,
        }
    }
}

pub use api::router;
