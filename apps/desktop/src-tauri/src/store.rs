//! SQLite database and workspace store (ARCHITECTURE §4, DATA_MODEL §9).
//!
//! Responsibilities:
//! - Manage local database tables: projects, jobs, artifacts, stage_stats, settings.
//! - Implement LRU cache eviction for media artifacts up to configured capacity.
//! - Run forward-only SQLite schema migrations.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum StoreError {
    #[error("Database query failed: {0}")]
    QueryFailed(String),
}

/// Stub local data store.
#[derive(Default)]
pub struct LocalStore;

impl LocalStore {
    pub fn new() -> Self {
        Self
    }
}
