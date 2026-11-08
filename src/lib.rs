//! Knowledge Garden: a self-hosted knowledge base where coding agents and
//! humans post memos, research notes, reports and troubleshooting records.

pub mod api;
pub mod auth;
pub mod config;
pub mod error;
pub mod model;
pub mod policy;
pub mod store;

use std::sync::Arc;

use crate::config::Config;
use crate::store::Store;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<dyn Store>,
    pub config: Arc<Config>,
}

impl AppState {
    pub fn new(store: impl Store + 'static, config: Config) -> Self {
        Self {
            store: Arc::new(store),
            config: Arc::new(config),
        }
    }
}

pub use api::router;
