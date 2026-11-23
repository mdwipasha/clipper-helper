//! Job runner and pipeline coordinator (ARCHITECTURE §4).
//!
//! Responsibilities:
//! - Manage asynchronous lifecycle for long-running pipelines (`generate`, `captions`, `export`).
//! - Support cancellation via process tree termination (Windows Job Object).
//! - Persist stage execution state to local SQLite for crash recovery and resume.
//! - Enforce single-heavy-stage concurrency limit.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum JobError {
    #[error("Job not found: {0}")]
    NotFound(String),
    #[error("Job execution cancelled: {0}")]
    Cancelled(String),
}

/// Stub coordinator for background jobs.
#[derive(Default)]
pub struct JobCoordinator;

impl JobCoordinator {
    pub fn new() -> Self {
        Self
    }
}
