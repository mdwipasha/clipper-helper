//! Sidecar lifecycle supervisor (ARCHITECTURE §4, SECURITY §9).
//!
//! Responsibilities:
//! - Spawn external binaries (`yt-dlp`, `qjs`, `ffmpeg`, `ffprobe`, `whisper`) with argument arrays.
//! - Enclose every process inside a Windows Job Object with `kill-on-close` flag.
//! - Enforce timeouts, output size limits, and parse real-time progress events.
//! - Verify binary checksums against component manifests before execution.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum SidecarError {
    #[error("Binary missing or failed integrity check: {0}")]
    BinaryMissing(String),
    #[error("Process execution timed out after {0} seconds")]
    Timeout(u64),
    #[error("Process failed with exit code: {0:?}")]
    ExecutionFailed(Option<i32>),
}

/// Stub supervisor for external sidecar processes.
#[derive(Default)]
pub struct SidecarSupervisor;

impl SidecarSupervisor {
    pub fn new() -> Self {
        Self
    }
}
