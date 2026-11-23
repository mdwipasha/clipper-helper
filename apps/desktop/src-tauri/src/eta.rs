//! Progress aggregation and ETA estimator (ARCHITECTURE §4, PROGRESS_AND_ETA §4).
//!
//! Responsibilities:
//! - Aggregate stage progress events from sidecars (FFmpeg time= lines, Whisper progress).
//! - Blend live rates (EMA) with device-profile priors stored in `stage_stats`.
//! - Enforce monotonic overall progress percentage and damped ETA updates.
//! - Emit periodic heartbeats (every 2s) to prevent UI stall alarms.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum EtaError {
    #[error("Invalid progress value: {0}")]
    InvalidProgress(String),
}

/// Stub estimator for job ETA calculation.
#[derive(Default)]
pub struct EtaEstimator;

impl EtaEstimator {
    pub fn new() -> Self {
        Self
    }
}
