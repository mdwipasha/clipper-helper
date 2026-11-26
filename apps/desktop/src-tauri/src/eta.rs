//! Progress aggregation and conservative-prior ETA estimator
//! (ARCHITECTURE §4, PROGRESS_AND_ETA §4, M1.1d-lite Part 3).
//!
//! When `stage_stats` contains no local history yet, returns conservative **RANGE** estimates with
//! `EtaConfidence::Low` based on docs/PROGRESS_AND_ETA §4. Once real runs populate `stage_stats`,
//! transitions to calibrated median-rate estimates.

use serde::{Deserialize, Serialize};
use specta::Type;
use thiserror::Error;

use crate::store::LocalStore;

#[derive(Error, Debug)]
pub enum EtaError {
    #[error("Invalid progress value: {0}")]
    InvalidProgress(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum EtaConfidence {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum EtaSource {
    PriorRange,
    CalibratedStats,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
pub struct EtaEstimate {
    pub seconds: f64,
    pub low_seconds: f64,
    pub high_seconds: f64,
    pub confidence: EtaConfidence,
    pub source: EtaSource,
}

#[derive(Debug, Clone, Copy)]
struct PriorRangeSpec {
    low_sec_per_unit: f64,
    high_sec_per_unit: f64,
    init_seconds: f64,
}

fn lookup_stage_prior(stage: &str, variant: &str) -> PriorRangeSpec {
    match (stage, variant) {
        ("transcribe", "small-q5_1") => PriorRangeSpec {
            low_sec_per_unit: 1.1,
            high_sec_per_unit: 2.2,
            init_seconds: 8.0,
        },
        ("transcribe", "base") => PriorRangeSpec {
            low_sec_per_unit: 2.5,
            high_sec_per_unit: 4.0,
            init_seconds: 5.0,
        },
        ("transcribe", "tiny") => PriorRangeSpec {
            low_sec_per_unit: 0.8,
            high_sec_per_unit: 1.8,
            init_seconds: 4.0,
        },
        ("transcribe", "small") => PriorRangeSpec {
            low_sec_per_unit: 2.0,
            high_sec_per_unit: 3.8,
            init_seconds: 10.0,
        },
        ("transcribe", "medium") => PriorRangeSpec {
            low_sec_per_unit: 4.0,
            high_sec_per_unit: 7.5,
            init_seconds: 15.0,
        },
        ("render", "h264_nvenc") | ("render", "h264_qsv") => PriorRangeSpec {
            // Untested conservative default prior centered at 1.00x RT per PROGRESS_AND_ETA §4
            low_sec_per_unit: 0.6,
            high_sec_per_unit: 1.4,
            init_seconds: 2.0,
        },
        ("render", "h264_amf") => PriorRangeSpec {
            // ~2.70x RT -> 1 / 2.70 = ~0.37 s/s conservative range
            low_sec_per_unit: 0.30,
            high_sec_per_unit: 0.55,
            init_seconds: 2.0,
        },
        ("render", "h264_mf") => PriorRangeSpec {
            // ~3.50x RT -> ~0.29 s/s; wide initial uncertainty before real telemetry
            low_sec_per_unit: 0.22,
            high_sec_per_unit: 0.45,
            init_seconds: 2.0,
        },
        _ => PriorRangeSpec {
            low_sec_per_unit: 0.5,
            high_sec_per_unit: 1.8,
            init_seconds: 3.0,
        },
    }
}

/// Estimator for stage & overall ETA using priors until `stage_stats` exists.
#[derive(Default)]
pub struct EtaEstimator;

impl EtaEstimator {
    pub fn new() -> Self {
        Self
    }

    /// Estimates remaining time for a stage with `work_units_remaining` (e.g. audio seconds or
    /// output duration seconds). Returns a conservative low-confidence range when `stage_stats`
    /// has no prior runs, or a tighter calibrated estimate once recorded history exists.
    pub fn estimate_stage(
        &self,
        store: Option<&LocalStore>,
        device_profile: &str,
        stage: &str,
        variant: &str,
        work_units_remaining: f64,
    ) -> Result<EtaEstimate, EtaError> {
        if !work_units_remaining.is_finite() || work_units_remaining < 0.0 {
            return Err(EtaError::InvalidProgress(format!(
                "work_units_remaining must be finite and >= 0, got {}",
                work_units_remaining
            )));
        }

        if let Some(st) = store {
            if let Ok(Some(median_units_per_ms)) =
                st.get_median_rate(device_profile, stage, variant, 20)
            {
                if median_units_per_ms > 0.0 {
                    let units_per_sec = median_units_per_ms * 1000.0;
                    let point = (work_units_remaining / units_per_sec).max(0.0);
                    return Ok(EtaEstimate {
                        seconds: point,
                        low_seconds: (point * 0.8).max(0.0),
                        high_seconds: point * 1.25,
                        confidence: EtaConfidence::Medium,
                        source: EtaSource::CalibratedStats,
                    });
                }
            }
        }

        let prior = lookup_stage_prior(stage, variant);
        let low_seconds = prior.init_seconds + (work_units_remaining * prior.low_sec_per_unit);
        let high_seconds = prior.init_seconds + (work_units_remaining * prior.high_sec_per_unit);
        let seconds = (low_seconds + high_seconds) * 0.5;

        Ok(EtaEstimate {
            seconds,
            low_seconds,
            high_seconds,
            confidence: EtaConfidence::Low,
            source: EtaSource::PriorRange,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_priors_return_conservative_low_confidence_range_without_stats() {
        let dir = TempDir::new().unwrap();
        let store = LocalStore::open(&dir.path().join("eta.db")).unwrap();
        let est = EtaEstimator::new();

        // No stage_stats yet -> prior range with low confidence
        let transcribe = est
            .estimate_stage(Some(&store), "prof1", "transcribe", "small-q5_1", 60.0)
            .unwrap();
        assert_eq!(transcribe.confidence, EtaConfidence::Low);
        assert_eq!(transcribe.source, EtaSource::PriorRange);
        // small-q5_1: [1.1*60 + 8, 2.2*60 + 8] = [74, 140]
        assert!((transcribe.low_seconds - 74.0).abs() < 1e-6);
        assert!((transcribe.high_seconds - 140.0).abs() < 1e-6);
        assert!(transcribe.high_seconds > transcribe.low_seconds);

        let nvenc = est
            .estimate_stage(Some(&store), "prof1", "render", "h264_nvenc", 60.0)
            .unwrap();
        assert_eq!(nvenc.confidence, EtaConfidence::Low);
        assert_eq!(nvenc.source, EtaSource::PriorRange);

        // Once stage_stats is recorded, estimator switches to calibrated median!
        store
            .insert_stage_stat("prof1", "transcribe", "small-q5_1", 60.0, 48_000)
            .unwrap();
        let calibrated = est
            .estimate_stage(Some(&store), "prof1", "transcribe", "small-q5_1", 60.0)
            .unwrap();
        assert_eq!(calibrated.source, EtaSource::CalibratedStats);
        assert_eq!(calibrated.confidence, EtaConfidence::Medium);
        assert!((calibrated.seconds - 48.0).abs() < 1e-4);
    }
}
