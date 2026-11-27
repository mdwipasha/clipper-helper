//! Progress aggregation, conservative-prior ETA estimator, and live EMA/median ETA tracker
//! (ARCHITECTURE §4, PROGRESS_AND_ETA §4–§5, M1.1d-lite Part 3, M1.4 Part 2d).

use std::collections::VecDeque;

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
        ("download", "yt-dlp") => PriorRangeSpec {
            // S1 benchmark prior: 1.66x RT (~0.602 s per range-second) + 10 s startup
            low_sec_per_unit: 0.45,
            high_sec_per_unit: 1.0 / 1.66,
            init_seconds: 10.0,
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

/// Stall severity derived from elapsed wall time without forward progress (`PROGRESS_AND_ETA §5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum StallStatus {
    Normal,
    /// >= 15 s without advancement -> warn "Koneksi lambat / proses belum bergerak — masih mencoba…"
    SlowWarning,
    /// >= 60 s without advancement -> offer Retry action
    StalledRetry,
}

#[derive(Debug, Clone)]
pub struct LiveEtaConfig {
    pub stage: String,
    pub total_units: f64,
    pub prior_rate_units_per_sec: f64,
    pub has_calibrated_prior: bool,
    pub is_download_stage: bool,
}

impl LiveEtaConfig {
    pub fn for_download(
        store: Option<&LocalStore>,
        device_profile: &str,
        total_units: f64,
    ) -> Self {
        let calibrated = store.and_then(|st| {
            st.get_median_rate(device_profile, "download", "yt-dlp", 10)
                .ok()
                .flatten()
                .filter(|r| *r > 0.0)
                .map(|units_per_ms| units_per_ms * 1000.0)
        });
        Self {
            stage: "download".to_string(),
            total_units: total_units.max(0.1),
            prior_rate_units_per_sec: calibrated.unwrap_or(1.66),
            has_calibrated_prior: calibrated.is_some(),
            is_download_stage: true,
        }
    }

    pub fn for_prepare_preview(
        store: Option<&LocalStore>,
        device_profile: &str,
        total_units: f64,
    ) -> Self {
        let calibrated = store.and_then(|st| {
            st.get_median_rate(device_profile, "prepare_preview", "h264_mf", 10)
                .ok()
                .flatten()
                .filter(|r| *r > 0.0)
                .map(|units_per_ms| units_per_ms * 1000.0)
        });
        Self {
            stage: "prepare_preview".to_string(),
            total_units: total_units.max(0.1),
            prior_rate_units_per_sec: calibrated.unwrap_or(3.50),
            has_calibrated_prior: calibrated.is_some(),
            is_download_stage: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LiveEtaSnapshot {
    pub is_startup_indeterminate: bool,
    pub done_units: f64,
    pub total_units: f64,
    pub fraction: f64,
    pub smoothed_rate_units_per_sec: Option<f64>,
    pub eta: Option<EtaEstimate>,
    pub stall_status: StallStatus,
    pub stall_elapsed_ms: u64,
}

#[derive(Debug, Clone)]
struct RateSample {
    ts_ms: u64,
    rate: f64,
}

/// Stateful per-stage live progress & ETA estimator implementing `PROGRESS_AND_ETA §4.2 & §5`:
/// - Indeterminate startup until first positive progress sample (`6-11 s` yt-dlp startup)
/// - EMA (α = 0.2) over 2 s windows (+ 10 s rolling median for `download` bursts)
/// - Blending `r = w * r_live + (1 - w) * r_prior` after >= 3 s live progress data
/// - Confidence levels (`low` / `medium` / `high` via CoV < 0.25 over >= 20 s)
/// - ETA damping (<10% unless >5 s, max +20% jump within 5 s unless stalled)
/// - Stall detection at 15 s (`SlowWarning`) and 60 s (`StalledRetry`)
pub struct LiveStageEtaTracker {
    cfg: LiveEtaConfig,
    started_at_ms: u64,
    first_progress_at_ms: Option<u64>,
    last_advance_at_ms: u64,
    last_done_units: f64,
    last_window_ts_ms: Option<u64>,
    last_window_done: f64,
    ema_rate: Option<f64>,
    recent_rates: VecDeque<RateSample>,
    last_damped_eta_s: Option<f64>,
    last_damped_at_ms: Option<u64>,
}

impl LiveStageEtaTracker {
    pub fn new(cfg: LiveEtaConfig, started_at_ms: u64) -> Self {
        Self {
            cfg,
            started_at_ms,
            first_progress_at_ms: None,
            last_advance_at_ms: started_at_ms,
            last_done_units: 0.0,
            last_window_ts_ms: None,
            last_window_done: 0.0,
            ema_rate: None,
            recent_rates: VecDeque::new(),
            last_damped_eta_s: None,
            last_damped_at_ms: None,
        }
    }

    pub fn update(&mut self, now_ms: u64, reported_done_units: Option<f64>) -> LiveEtaSnapshot {
        let now_ms = now_ms.max(self.started_at_ms);

        if let Some(raw_done) = reported_done_units {
            if raw_done.is_finite() && raw_done > 0.0 {
                let clamped = raw_done.clamp(0.0, self.cfg.total_units);
                if clamped > self.last_done_units + 0.01 {
                    if self.first_progress_at_ms.is_none() {
                        self.first_progress_at_ms = Some(now_ms);
                        self.last_window_ts_ms = Some(now_ms);
                        self.last_window_done = clamped;
                    }
                    self.last_done_units = clamped;
                    self.last_advance_at_ms = now_ms;
                }
            }
        }

        // Compute 2 s windowed instantaneous rate samples
        if let Some(win_ts) = self.last_window_ts_ms {
            let dt_ms = now_ms.saturating_sub(win_ts);
            if dt_ms >= 2_000 {
                let delta_units = (self.last_done_units - self.last_window_done).max(0.0);
                let inst_rate = delta_units / (dt_ms as f64 / 1000.0);
                if inst_rate > 0.0 && inst_rate.is_finite() {
                    let next_ema = match self.ema_rate {
                        Some(prev) => 0.2 * inst_rate + 0.8 * prev,
                        None => inst_rate,
                    };
                    self.ema_rate = Some(next_ema);
                    self.recent_rates.push_back(RateSample {
                        ts_ms: now_ms,
                        rate: inst_rate,
                    });
                    // Keep last 25 s (for 10 s median + 20 s CoV check)
                    while let Some(front) = self.recent_rates.front() {
                        if now_ms.saturating_sub(front.ts_ms) > 25_000 {
                            self.recent_rates.pop_front();
                        } else {
                            break;
                        }
                    }
                }
                self.last_window_ts_ms = Some(now_ms);
                self.last_window_done = self.last_done_units;
            }
        }

        let stall_elapsed_ms = now_ms.saturating_sub(self.last_advance_at_ms);
        let stall_status = if stall_elapsed_ms >= 60_000 {
            StallStatus::StalledRetry
        } else if stall_elapsed_ms >= 15_000 {
            StallStatus::SlowWarning
        } else {
            StallStatus::Normal
        };

        let fraction = (self.last_done_units / self.cfg.total_units).clamp(0.0, 1.0);
        let Some(first_ms) = self.first_progress_at_ms else {
            return LiveEtaSnapshot {
                is_startup_indeterminate: true,
                done_units: 0.0,
                total_units: self.cfg.total_units,
                fraction: 0.0,
                smoothed_rate_units_per_sec: None,
                eta: None,
                stall_status,
                stall_elapsed_ms,
            };
        };

        let live_elapsed_s = now_ms.saturating_sub(first_ms) as f64 / 1000.0;
        let median_10s = self.median_rate_over_window(now_ms, 10_000);
        let live_rate = if self.cfg.is_download_stage {
            median_10s.or(self.ema_rate)
        } else {
            self.ema_rate.or(median_10s)
        };

        let blended_rate = if live_elapsed_s >= 3.0 {
            if let Some(r_live) = live_rate {
                let w = (fraction / 0.15).clamp(0.0, 1.0);
                (w * r_live + (1.0 - w) * self.cfg.prior_rate_units_per_sec).max(0.05)
            } else {
                self.cfg.prior_rate_units_per_sec
            }
        } else {
            self.cfg.prior_rate_units_per_sec
        };

        let remaining_units = (self.cfg.total_units - self.last_done_units).max(0.0);
        let raw_eta_s = if remaining_units <= 0.0 {
            0.0
        } else {
            (remaining_units / blended_rate).max(0.0)
        };

        let damped_eta_s =
            self.apply_damping(now_ms, raw_eta_s, stall_status != StallStatus::Normal);
        let confidence = self.compute_confidence(now_ms, live_elapsed_s, fraction);

        let (low_seconds, high_seconds) = match confidence {
            EtaConfidence::High => ((damped_eta_s * 0.9).max(0.0), damped_eta_s * 1.15),
            EtaConfidence::Medium => ((damped_eta_s * 0.85).max(0.0), damped_eta_s * 1.25),
            EtaConfidence::Low => ((damped_eta_s * 0.85).max(0.0), damped_eta_s * 1.3),
        };

        LiveEtaSnapshot {
            is_startup_indeterminate: false,
            done_units: self.last_done_units,
            total_units: self.cfg.total_units,
            fraction,
            smoothed_rate_units_per_sec: live_rate,
            eta: Some(EtaEstimate {
                seconds: damped_eta_s,
                low_seconds,
                high_seconds,
                confidence,
                source: if self.cfg.has_calibrated_prior || live_rate.is_some() {
                    EtaSource::CalibratedStats
                } else {
                    EtaSource::PriorRange
                },
            }),
            stall_status,
            stall_elapsed_ms,
        }
    }

    fn median_rate_over_window(&self, now_ms: u64, window_ms: u64) -> Option<f64> {
        let mut vals: Vec<f64> = self
            .recent_rates
            .iter()
            .filter(|s| now_ms.saturating_sub(s.ts_ms) <= window_ms)
            .map(|s| s.rate)
            .collect();
        if vals.is_empty() {
            return None;
        }
        vals.sort_by(|a, b| a.total_cmp(b));
        let mid = vals.len() / 2;
        if vals.len() % 2 == 1 {
            Some(vals[mid])
        } else {
            Some((vals[mid - 1] + vals[mid]) * 0.5)
        }
    }

    fn compute_confidence(&self, now_ms: u64, live_elapsed_s: f64, fraction: f64) -> EtaConfidence {
        if live_elapsed_s >= 20.0 && fraction >= 0.05 {
            let vals: Vec<f64> = self
                .recent_rates
                .iter()
                .filter(|s| now_ms.saturating_sub(s.ts_ms) <= 20_000)
                .map(|s| s.rate)
                .collect();
            if vals.len() >= 3 {
                let mean = vals.iter().sum::<f64>() / vals.len() as f64;
                if mean > 0.0 {
                    let var = vals
                        .iter()
                        .map(|v| {
                            let d = v - mean;
                            d * d
                        })
                        .sum::<f64>()
                        / vals.len() as f64;
                    let cov = var.sqrt() / mean;
                    if cov < 0.25 {
                        return EtaConfidence::High;
                    }
                }
            }
        }

        if (live_elapsed_s >= 10.0 && fraction >= 0.05) || self.cfg.has_calibrated_prior {
            EtaConfidence::Medium
        } else {
            EtaConfidence::Low
        }
    }

    fn apply_damping(&mut self, now_ms: u64, candidate_s: f64, is_stalled: bool) -> f64 {
        let Some(prev_s) = self.last_damped_eta_s else {
            self.last_damped_eta_s = Some(candidate_s);
            self.last_damped_at_ms = Some(now_ms);
            return candidate_s;
        };

        let delta = (candidate_s - prev_s).abs();
        let rel_change = if prev_s > 0.01 { delta / prev_s } else { 1.0 };

        // Ignore ETA changes < 10% unless > 5 s
        if rel_change < 0.10 && delta <= 5.0 {
            return prev_s;
        }

        // Do not increase ETA by > 20% within 5 s unless a stall is active
        let elapsed_since_eta_change = self
            .last_damped_at_ms
            .map(|t| now_ms.saturating_sub(t))
            .unwrap_or(u64::MAX);
        let damped = if !is_stalled
            && elapsed_since_eta_change < 5_000
            && prev_s > 0.0
            && candidate_s > prev_s * 1.20
        {
            prev_s * 1.20
        } else {
            candidate_s
        };

        self.last_damped_eta_s = Some(damped);
        self.last_damped_at_ms = Some(now_ms);
        damped
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

    #[test]
    fn test_live_eta_table_driven_traces_steady_bursty_slow_start_and_stall() {
        // Trace 1: Steady 2.0x real-time over 300 s clip -> error at 30% progress (done=90s) <= 25%
        let mut steady = LiveStageEtaTracker::new(LiveEtaConfig::for_download(None, "p", 300.0), 0);
        // Startup indeterminate for first 4 s
        let init_snap = steady.update(4_000, None);
        assert!(init_snap.is_startup_indeterminate);
        assert!(init_snap.eta.is_none());

        let mut last_fraction = 0.0;
        let mut snap_at_30pct = None;
        for step_s in (6..=50).step_by(2) {
            let done = ((step_s - 6) as f64) * 2.0 + 2.0;
            let snap = steady.update((step_s as u64) * 1000, Some(done));
            assert!(!snap.is_startup_indeterminate);
            assert!(snap.fraction >= last_fraction);
            last_fraction = snap.fraction;
            if snap.fraction >= 0.30 && snap_at_30pct.is_none() {
                snap_at_30pct = Some(snap);
            }
        }
        let s30 = snap_at_30pct.expect("reached 30% progress");
        let true_remaining = (300.0 - s30.done_units) / 2.0;
        let est_eta = s30.eta.unwrap().seconds;
        let rel_err = (est_eta - true_remaining).abs() / true_remaining;
        assert!(
            rel_err <= 0.25,
            "Steady trace ETA error at 30% must be <= 25%, got {:.2}% (est={:.1}s true={:.1}s)",
            rel_err * 100.0,
            est_eta,
            true_remaining
        );

        // Trace 2: Bursty DASH trace (alternating fast burst & brief pause) -> median resists wild spikes
        let mut bursty = LiveStageEtaTracker::new(LiveEtaConfig::for_download(None, "p", 300.0), 0);
        let burst_trace: &[(u64, f64)] = &[
            (6_000, 3.0),
            (8_000, 12.0),  // 4.5x spike
            (10_000, 12.5), // slow dip
            (12_000, 22.0), // 4.75x spike
            (14_000, 23.0),
            (16_000, 27.0),
        ];
        let mut burst_snaps = Vec::new();
        for &(ts, done) in burst_trace {
            burst_snaps.push(bursty.update(ts, Some(done)));
        }
        let burst_last = burst_snaps.last().unwrap();
        let eta_s = burst_last.eta.as_ref().unwrap().seconds;
        assert!(eta_s > 40.0 && eta_s < 250.0, "bounded ETA got {}", eta_s);

        // Trace 3: Slow start (8 s indeterminate startup before first frame)
        let mut slow_start =
            LiveStageEtaTracker::new(LiveEtaConfig::for_download(None, "p", 120.0), 0);
        assert!(slow_start.update(3_000, None).is_startup_indeterminate);
        assert!(slow_start.update(8_000, None).is_startup_indeterminate);
        let after_start = slow_start.update(9_000, Some(2.0));
        assert!(!after_start.is_startup_indeterminate);
        assert!(after_start.eta.is_some());

        // Trace 4: Stall detection at 15 s (SlowWarning) and 60 s (StalledRetry)
        let mut stall = LiveStageEtaTracker::new(LiveEtaConfig::for_download(None, "p", 120.0), 0);
        let s1 = stall.update(6_000, Some(10.0));
        assert_eq!(s1.stall_status, StallStatus::Normal);
        let s2 = stall.update(21_500, Some(10.0));
        assert_eq!(s2.stall_status, StallStatus::SlowWarning);
        let s3 = stall.update(66_500, Some(10.0));
        assert_eq!(s3.stall_status, StallStatus::StalledRetry);
    }
}
