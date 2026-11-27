//! Safe default speech-model selection & real-time-factor (RTF) slowdown advisor
//! (ARCHITECTURE §8; PROGRESS_AND_ETA §4; docs/benchmarks/S3-speech.md; M1.1d-lite Part 3).
//!
//! - Defaults cleanly without any up-front benchmark:
//!   `RAM >= 6.5 GiB` reported physical RAM (an "8 GB" Windows laptop reports ~7.4 GiB) -> `small-q5_1`,
//!   otherwise (including unknown RAM) -> `base`.
//! - Users can override cleanly in Pengaturan (`tiny` / `base` / `small-q5_1` / `small` / `medium`).
//! - Records measured transcription RTF in `stage_stats`; if `small-q5_1` exceeds `1.5` RTF
//!   twice consecutively, raises a non-blocking suggestion to switch to a lighter model
//!   (never switches automatically).

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::store::{LocalStore, StoreError};

/// Minimum reported physical RAM (`6.5 GiB` = 6,979,321,856 bytes) to default to `small-q5_1`.
/// Nominal "8 GB" Windows laptops commonly report ~7.4 GiB usable physical memory.
pub const MIN_RAM_BYTES_FOR_SMALL_Q5_1: u64 = (13 * 1024 * 1024 * 1024) / 2;
pub const SMALL_Q5_1_SLOW_RTF_THRESHOLD: f64 = 1.5;
pub const CONSECUTIVE_SLOW_RUNS_TO_SUGGEST: u32 = 2;

pub const SETTING_SPEECH_MODEL: &str = "speech_model";
pub const SETTING_SPEECH_DOWNGRADE_SUGGESTED: &str = "speech_model_downgrade_suggested";
pub const SETTING_SPEECH_DOWNGRADE_DISMISSED_FOR: &str = "speech_model_downgrade_dismissed_for";
pub const SETTING_SPEECH_SLOW_RUN_STREAK: &str = "speech_model_consecutive_slow_runs";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "kebab-case")]
pub enum SpeechModel {
    Tiny,
    Base,
    #[serde(rename = "small-q5_1")]
    SmallQ5_1,
    Small,
    Medium,
}

impl SpeechModel {
    pub fn as_str(&self) -> &'static str {
        match self {
            SpeechModel::Tiny => "tiny",
            SpeechModel::Base => "base",
            SpeechModel::SmallQ5_1 => "small-q5_1",
            SpeechModel::Small => "small",
            SpeechModel::Medium => "medium",
        }
    }

    pub fn from_str_id(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "tiny" => Some(SpeechModel::Tiny),
            "base" => Some(SpeechModel::Base),
            "small-q5_1" | "small_q5_1" => Some(SpeechModel::SmallQ5_1),
            "small" => Some(SpeechModel::Small),
            "medium" => Some(SpeechModel::Medium),
            _ => None,
        }
    }

    pub fn manifest_component(&self) -> &'static str {
        match self {
            SpeechModel::Tiny => "ggml-tiny.bin",
            SpeechModel::Base => "ggml-base.bin",
            SpeechModel::SmallQ5_1 => "ggml-small-q5_1.bin",
            SpeechModel::Small => "ggml-small.bin",
            SpeechModel::Medium => "ggml-medium.bin",
        }
    }

    pub fn min_ram_gb(&self) -> u32 {
        match self {
            SpeechModel::Tiny => 2,
            SpeechModel::Base => 4,
            SpeechModel::SmallQ5_1 => 8,
            SpeechModel::Small => 8,
            SpeechModel::Medium => 16,
        }
    }

    pub fn speed_tier(&self) -> &'static str {
        match self {
            SpeechModel::Tiny => "very_fast",
            SpeechModel::Base => "fast",
            SpeechModel::SmallQ5_1 => "balanced",
            SpeechModel::Small => "slower",
            SpeechModel::Medium => "heavy",
        }
    }

    pub fn all_options() -> &'static [SpeechModel] {
        &[
            SpeechModel::Tiny,
            SpeechModel::Base,
            SpeechModel::SmallQ5_1,
            SpeechModel::Small,
            SpeechModel::Medium,
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct SpeechModelOptionInfo {
    pub id: String,
    pub manifest_component: String,
    pub min_ram_gb: u32,
    pub speed_tier: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct SpeechModelState {
    pub active_model: SpeechModel,
    pub recommended_model: SpeechModel,
    pub is_user_override: bool,
    pub downgrade_suggested: bool,
    pub suggested_lighter_model: Option<SpeechModel>,
    pub consecutive_slow_runs: u32,
    pub options: Vec<SpeechModelOptionInfo>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RtfRecordOutcome {
    pub measured_rtf: f64,
    pub consecutive_slow_runs: u32,
    pub downgrade_suggested: bool,
}

/// Safe default speech model rule without benchmark:
/// `RAM >= 6.5 GiB` reported total -> `small-q5_1`, otherwise (including `None`) -> `base`.
pub fn recommended_speech_model(ram_bytes: Option<u64>) -> SpeechModel {
    match ram_bytes {
        Some(bytes) if bytes >= MIN_RAM_BYTES_FOR_SMALL_Q5_1 => SpeechModel::SmallQ5_1,
        _ => SpeechModel::Base,
    }
}

/// Reads the user's current speech-model state from settings + detected RAM.
pub fn resolve_speech_model_state(
    store: &LocalStore,
    ram_bytes: Option<u64>,
) -> Result<SpeechModelState, StoreError> {
    let recommended = recommended_speech_model(ram_bytes);
    let saved = store
        .get_setting(SETTING_SPEECH_MODEL)?
        .and_then(|v| SpeechModel::from_str_id(&v));

    let (active_model, is_user_override) = match saved {
        Some(m) => (m, true),
        None => (recommended, false),
    };

    let streak = store
        .get_setting(SETTING_SPEECH_SLOW_RUN_STREAK)?
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);

    let suggested_flag = store
        .get_setting(SETTING_SPEECH_DOWNGRADE_SUGGESTED)?
        .as_deref()
        == Some("1");
    let dismissed_for = store.get_setting(SETTING_SPEECH_DOWNGRADE_DISMISSED_FOR)?;

    // Only suggest when active model is `small-q5_1` (never auto-switch!).
    let downgrade_suggested = active_model == SpeechModel::SmallQ5_1
        && suggested_flag
        && dismissed_for.as_deref() != Some(active_model.as_str());

    let options = SpeechModel::all_options()
        .iter()
        .map(|m| SpeechModelOptionInfo {
            id: m.as_str().to_string(),
            manifest_component: m.manifest_component().to_string(),
            min_ram_gb: m.min_ram_gb(),
            speed_tier: m.speed_tier().to_string(),
        })
        .collect();

    Ok(SpeechModelState {
        active_model,
        recommended_model: recommended,
        is_user_override,
        downgrade_suggested,
        suggested_lighter_model: if downgrade_suggested {
            Some(SpeechModel::Base)
        } else {
            None
        },
        consecutive_slow_runs: streak,
        options,
    })
}

/// Persists explicit speech model selection (`Some(model)`) or resets to auto-default (`None`).
/// Clears stale downgrade banner when switching away from `small-q5_1`.
pub fn set_speech_model_preference(
    store: &LocalStore,
    model: Option<SpeechModel>,
) -> Result<(), StoreError> {
    match model {
        Some(m) => {
            store.set_setting(SETTING_SPEECH_MODEL, m.as_str())?;
            if m != SpeechModel::SmallQ5_1 {
                let _ = store.delete_setting(SETTING_SPEECH_DOWNGRADE_SUGGESTED);
                let _ = store.delete_setting(SETTING_SPEECH_SLOW_RUN_STREAK);
            }
        }
        None => {
            let _ = store.delete_setting(SETTING_SPEECH_MODEL);
        }
    }
    let _ = store.delete_setting(SETTING_SPEECH_DOWNGRADE_DISMISSED_FOR);
    Ok(())
}

/// Records the measured transcription Real-Time Factor (`wall_secs / audio_secs`) into
/// `stage_stats` and updates consecutive slow-run tracking for `small-q5_1`.
/// Never changes the active model automatically.
pub fn record_transcribe_run_rtf(
    store: &LocalStore,
    device_profile: &str,
    model: SpeechModel,
    audio_seconds: f64,
    wall_ms: i64,
) -> Result<RtfRecordOutcome, StoreError> {
    if audio_seconds <= 0.0 || wall_ms <= 0 {
        return Ok(RtfRecordOutcome {
            measured_rtf: 0.0,
            consecutive_slow_runs: 0,
            downgrade_suggested: false,
        });
    }

    store.insert_stage_stat(
        device_profile,
        "transcribe",
        model.as_str(),
        audio_seconds,
        wall_ms,
    )?;

    let rtf = (wall_ms as f64 / 1000.0) / audio_seconds;

    if model != SpeechModel::SmallQ5_1 {
        return Ok(RtfRecordOutcome {
            measured_rtf: rtf,
            consecutive_slow_runs: 0,
            downgrade_suggested: false,
        });
    }

    let prev_streak = store
        .get_setting(SETTING_SPEECH_SLOW_RUN_STREAK)?
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);

    let new_streak = if rtf > SMALL_Q5_1_SLOW_RTF_THRESHOLD {
        prev_streak.saturating_add(1)
    } else {
        0
    };

    store.set_setting(SETTING_SPEECH_SLOW_RUN_STREAK, &new_streak.to_string())?;

    let mut downgrade_suggested = store
        .get_setting(SETTING_SPEECH_DOWNGRADE_SUGGESTED)?
        .as_deref()
        == Some("1");
    if new_streak >= CONSECUTIVE_SLOW_RUNS_TO_SUGGEST {
        store.set_setting(SETTING_SPEECH_DOWNGRADE_SUGGESTED, "1")?;
        downgrade_suggested = true;
    }

    Ok(RtfRecordOutcome {
        measured_rtf: rtf,
        consecutive_slow_runs: new_streak,
        downgrade_suggested,
    })
}

/// Dismisses the non-blocking downgrade banner for `small-q5_1` without changing model.
pub fn dismiss_speech_downgrade_suggestion(store: &LocalStore) -> Result<(), StoreError> {
    store.set_setting(
        SETTING_SPEECH_DOWNGRADE_DISMISSED_FOR,
        SpeechModel::SmallQ5_1.as_str(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn gib_to_bytes(gib: f64) -> u64 {
        (gib * (1024.0 * 1024.0 * 1024.0)) as u64
    }

    #[test]
    fn test_recommended_speech_model_table() {
        let cases: &[(Option<u64>, SpeechModel)] = &[
            (None, SpeechModel::Base),
            (Some(0), SpeechModel::Base),
            (Some(gib_to_bytes(3.7)), SpeechModel::Base),
            (Some(gib_to_bytes(5.8)), SpeechModel::Base),
            (Some(MIN_RAM_BYTES_FOR_SMALL_Q5_1 - 1), SpeechModel::Base),
            (Some(MIN_RAM_BYTES_FOR_SMALL_Q5_1), SpeechModel::SmallQ5_1),
            (Some(gib_to_bytes(6.5)), SpeechModel::SmallQ5_1),
            (Some(gib_to_bytes(7.4)), SpeechModel::SmallQ5_1),
            (Some(gib_to_bytes(7.9)), SpeechModel::SmallQ5_1),
            (Some(gib_to_bytes(15.6)), SpeechModel::SmallQ5_1),
            (Some(gib_to_bytes(31.6)), SpeechModel::SmallQ5_1),
        ];

        for (ram, expected) in cases {
            assert_eq!(
                recommended_speech_model(*ram),
                *expected,
                "Failed for ram={:?}",
                ram
            );
        }
    }

    #[test]
    fn test_rtf_twice_exceeding_threshold_suggests_but_never_auto_switches() {
        let dir = TempDir::new().unwrap();
        let store = LocalStore::open(&dir.path().join("speech.db")).unwrap();

        // Device with 16 GiB RAM defaults to small-q5_1
        let ram = Some(16 * 1024 * 1024 * 1024);
        let init_state = resolve_speech_model_state(&store, ram).unwrap();
        assert_eq!(init_state.active_model, SpeechModel::SmallQ5_1);
        assert!(!init_state.downgrade_suggested);

        // Run 1: slow (RTF = 18s / 10s = 1.8 > 1.5) -> streak 1, NOT suggested yet
        let r1 = record_transcribe_run_rtf(&store, "prof", SpeechModel::SmallQ5_1, 10.0, 18_000)
            .unwrap();
        assert!(r1.measured_rtf > 1.5);
        assert_eq!(r1.consecutive_slow_runs, 1);
        assert!(!r1.downgrade_suggested);

        // Fast run resets streak back to 0 (must be *twice in a row*)
        let fast =
            record_transcribe_run_rtf(&store, "prof", SpeechModel::SmallQ5_1, 10.0, 9_000).unwrap();
        assert_eq!(fast.consecutive_slow_runs, 0);
        assert!(!fast.downgrade_suggested);

        // Run 2a: slow (16s / 10s = 1.6)
        let r2a = record_transcribe_run_rtf(&store, "prof", SpeechModel::SmallQ5_1, 10.0, 16_000)
            .unwrap();
        assert_eq!(r2a.consecutive_slow_runs, 1);
        assert!(!r2a.downgrade_suggested);

        // Run 2b: slow again (17s / 10s = 1.7) -> streak 2 -> suggestion raised!
        let r2b = record_transcribe_run_rtf(&store, "prof", SpeechModel::SmallQ5_1, 10.0, 17_000)
            .unwrap();
        assert_eq!(r2b.consecutive_slow_runs, 2);
        assert!(r2b.downgrade_suggested);

        // Critical invariant: active model MUST still be small-q5_1 (never auto-switched!)
        let after = resolve_speech_model_state(&store, ram).unwrap();
        assert_eq!(after.active_model, SpeechModel::SmallQ5_1);
        assert!(after.downgrade_suggested);
        assert_eq!(after.suggested_lighter_model, Some(SpeechModel::Base));

        // Explicit user override to tiny clears suggestion cleanly
        set_speech_model_preference(&store, Some(SpeechModel::Tiny)).unwrap();
        let overridden = resolve_speech_model_state(&store, ram).unwrap();
        assert_eq!(overridden.active_model, SpeechModel::Tiny);
        assert!(overridden.is_user_override);
        assert!(!overridden.downgrade_suggested);
    }
}
