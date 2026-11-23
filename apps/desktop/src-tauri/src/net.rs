//! Network client for Supabase Edge Functions (ARCHITECTURE §4, §10).
//!
//! Responsibilities:
//! - Communicate with Edge Functions (`begin-export`, `finalize-export`, `get-app-config`).
//! - Manage exponential backoff and persistent outbox retries for offline resilience.
//! - Enforce request timeouts and rate limits.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum NetError {
    #[error("Network offline or unreachable")]
    Offline,
    #[error("Server error: {0}")]
    ServerError(String),
}

/// Stub network client for cloud backend communications.
#[derive(Default)]
pub struct ApiClient;

impl ApiClient {
    pub fn new() -> Self {
        Self
    }
}
