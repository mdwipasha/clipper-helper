//! Hardware probe and benchmarking (ARCHITECTURE §4, §8).
//!
//! Responsibilities:
//! - Probe system CPU, RAM, available GPUs, and storage headroom.
//! - Test hardware encoders (`h264_nvenc`, `h264_qsv`, `h264_amf`, `h264_mf`) to determine device profile.
//! - Run first-launch micro-benchmark to seed ETA priors in `stage_stats`.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum HardwareError {
    #[error("Hardware probe failure: {0}")]
    ProbeFailed(String),
}

/// Stub hardware probe and benchmark engine.
#[derive(Default)]
pub struct HardwareProber;

impl HardwareProber {
    pub fn new() -> Self {
        Self
    }
}
