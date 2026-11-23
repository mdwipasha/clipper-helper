//! Application and component updater (ARCHITECTURE §4, §13).
//!
//! Responsibilities:
//! - Fetch and verify signed component manifests for sidecars and speech models.
//! - Perform atomic in-place updates with fallback rollback support.
//! - Check application version against `min_app_version` policy.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum UpdaterError {
    #[error("Manifest verification failed: {0}")]
    ManifestInvalid(String),
}

/// Stub updater for application and binary components.
#[derive(Default)]
pub struct ComponentUpdater;

impl ComponentUpdater {
    pub fn new() -> Self {
        Self
    }
}
