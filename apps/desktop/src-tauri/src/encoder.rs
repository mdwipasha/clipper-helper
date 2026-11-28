//! Lazy video encoder chain, S4 stderr failure classifier, and persisted fallback state
//! (ARCHITECTURE §8; docs/benchmarks/S4-render.md; PROGRESS_AND_ETA §8; M1.1d-lite Part 3).
//!
//! No up-front startup benchmark is run. On first export, `select_or_probe_encoder` lazily tests
//! `h264_nvenc -> h264_qsv -> h264_amf -> h264_mf` via the `EncoderProber` trait, persists the
//! first working encoder alongside the hardware/driver fingerprint so GPU/driver updates re-probe,
//! falls back automatically on runtime failure, and maps total failure to `E_ENCODER_UNAVAILABLE`.

use serde::{Deserialize, Serialize};
use specta::Type;
use thiserror::Error;

use crate::hardware::HardwareProfile;
use crate::security;
use crate::store::{LocalStore, NewErrorEvent, StoreError};

pub const SETTING_PREFERRED_ENCODER: &str = "preferred_encoder";
pub const SETTING_ENCODER_FINGERPRINT: &str = "encoder_fingerprint";
pub const SETTING_ENCODER_FALLBACK_HISTORY: &str = "encoder_fallback_history";

/// Failures during the first N seconds of encoded media time (or matching S4 init
/// stderr patterns) trigger automatic fallback to the next encoder in the chain.
pub const EARLY_ENCODER_FAILURE_WINDOW_SECS: f64 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum VideoEncoder {
    H264Nvenc,
    H264Qsv,
    H264Amf,
    H264Mf,
}

impl VideoEncoder {
    pub fn as_ffmpeg_name(&self) -> &'static str {
        match self {
            VideoEncoder::H264Nvenc => "h264_nvenc",
            VideoEncoder::H264Qsv => "h264_qsv",
            VideoEncoder::H264Amf => "h264_amf",
            VideoEncoder::H264Mf => "h264_mf",
        }
    }

    pub fn from_ffmpeg_name(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "h264_nvenc" => Some(VideoEncoder::H264Nvenc),
            "h264_qsv" => Some(VideoEncoder::H264Qsv),
            "h264_amf" => Some(VideoEncoder::H264Amf),
            "h264_mf" => Some(VideoEncoder::H264Mf),
            _ => None,
        }
    }

    pub fn ordered_chain() -> &'static [VideoEncoder] {
        &[
            VideoEncoder::H264Nvenc,
            VideoEncoder::H264Qsv,
            VideoEncoder::H264Amf,
            VideoEncoder::H264Mf,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum EncoderFailureKind {
    EncoderNotFound,
    DriverUnsupportedOrInitError,
    MediaFoundationOrProfileError,
    HardwareUnavailable,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct EncoderProbeFailure {
    pub encoder: VideoEncoder,
    pub exit_code: Option<i32>,
    pub kind: EncoderFailureKind,
    pub summary: String,
    pub stderr_tail_redacted: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct EncoderFallbackEntry {
    pub from: String,
    pub to: Option<String>,
    pub reason_kind: EncoderFailureKind,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum EncoderUnavailableAction {
    UpdateGpuDriver,
    InstallMediaFeaturePack,
    ExportDiagnostics,
}

#[derive(Error, Debug, Clone, Serialize, Deserialize)]
#[error("{message}")]
pub struct EncoderUnavailableError {
    pub code: String,
    pub message_key: String,
    pub message: String,
    pub is_windows_n_kn: bool,
    pub actions: Vec<EncoderUnavailableAction>,
    pub attempts: Vec<EncoderProbeFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EncoderSelectionResult {
    pub selected: VideoEncoder,
    pub used_cached: bool,
    pub fingerprint_changed: bool,
    pub fallback_occurred: bool,
    pub fallback_message: Option<String>,
    pub attempts: Vec<EncoderProbeFailure>,
}

/// Passive snapshot shown in Pengaturan & Diagnostik before any export has probed FFmpeg.
#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct EncoderStatusSnapshot {
    pub chain_order: Vec<String>,
    pub cached_encoder: Option<String>,
    pub cached_fingerprint: Option<String>,
    pub current_fingerprint: String,
    pub fingerprint_matches: bool,
    pub is_windows_n_kn: bool,
    pub fallback_history: Vec<EncoderFallbackEntry>,
}

/// Trait implemented by export runners (real FFmpeg sidecar runner in M1.7, mock in unit tests).
pub trait EncoderProber {
    fn probe_encoder(&self, encoder: VideoEncoder) -> Result<(), EncoderProbeFailure>;
}

/// Classifies FFmpeg encoder stderr failures using the S4 Render benchmark signatures
/// (`docs/benchmarks/S4-render.md`).
pub fn classify_encoder_stderr(
    encoder: VideoEncoder,
    exit_code: Option<i32>,
    stderr: &str,
) -> EncoderProbeFailure {
    let lower = stderr.to_ascii_lowercase();
    let redacted_tail = security::redact_sensitive_text(stderr);

    let (kind, summary) = if lower.contains("unknown encoder")
        || lower.contains("encoder not found")
        || lower.contains("error opening output files: encoder not found")
    {
        (
            EncoderFailureKind::EncoderNotFound,
            format!(
                "{} is not available in this FFmpeg build",
                encoder.as_ffmpeg_name()
            ),
        )
    } else if lower.contains("unable to parse \"profile\"")
        || lower.contains("mfplat.dll")
        || lower.contains("media foundation")
        || lower.contains("mfstartup")
        || (lower.contains("[h264_mf") && lower.contains("error"))
    {
        (
            EncoderFailureKind::MediaFoundationOrProfileError,
            format!(
                "{} Media Foundation initialization or profile error",
                encoder.as_ffmpeg_name()
            ),
        )
    } else if lower.contains("cannot load nvcuda")
        || lower.contains("nvenc api version")
        || lower.contains("does not support nvenc")
        || lower.contains("error initializing mfx session")
        || lower.contains("unsupported device")
        || lower.contains("no capable devices found")
        || lower.contains("task finished with error code: -22")
        || lower.contains("error opening output files: generic error in an external library")
    {
        let hw_unavailable = lower.contains("cannot load nvcuda")
            || lower.contains("no capable devices found")
            || lower.contains("unsupported device");
        if hw_unavailable {
            (
                EncoderFailureKind::HardwareUnavailable,
                format!(
                    "{} hardware adapter unavailable or driver missing",
                    encoder.as_ffmpeg_name()
                ),
            )
        } else {
            (
                EncoderFailureKind::DriverUnsupportedOrInitError,
                format!(
                    "{} initialization failed (driver or option mismatch)",
                    encoder.as_ffmpeg_name()
                ),
            )
        }
    } else {
        (
            EncoderFailureKind::Unknown,
            format!(
                "{} probe failed (exit {:?})",
                encoder.as_ffmpeg_name(),
                exit_code
            ),
        )
    };

    EncoderProbeFailure {
        encoder,
        exit_code,
        kind,
        summary,
        stderr_tail_redacted: redacted_tail,
    }
}

/// Reads passive encoder status & fallback history from SQLite without invoking FFmpeg.
pub fn get_encoder_status(
    store: &LocalStore,
    hw: &HardwareProfile,
) -> Result<EncoderStatusSnapshot, StoreError> {
    let cached_encoder = store.get_setting(SETTING_PREFERRED_ENCODER)?;
    let cached_fingerprint = store.get_setting(SETTING_ENCODER_FINGERPRINT)?;
    let fingerprint_matches = cached_fingerprint
        .as_deref()
        .is_some_and(|fp| fp == hw.fingerprint);
    let fallback_history: Vec<EncoderFallbackEntry> = store
        .get_setting(SETTING_ENCODER_FALLBACK_HISTORY)?
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();

    Ok(EncoderStatusSnapshot {
        chain_order: VideoEncoder::ordered_chain()
            .iter()
            .map(|e| e.as_ffmpeg_name().to_string())
            .collect(),
        cached_encoder,
        cached_fingerprint,
        current_fingerprint: hw.fingerprint.clone(),
        fingerprint_matches,
        is_windows_n_kn: hw.is_windows_n_kn,
        fallback_history,
    })
}

fn append_fallback_history(store: &LocalStore, new_entries: &[EncoderFallbackEntry]) {
    if new_entries.is_empty() {
        return;
    }
    let mut history: Vec<EncoderFallbackEntry> = store
        .get_setting(SETTING_ENCODER_FALLBACK_HISTORY)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    history.extend(new_entries.iter().cloned());
    if history.len() > 20 {
        let drop_n = history.len() - 20;
        history.drain(0..drop_n);
    }
    if let Ok(json) = serde_json::to_string(&history) {
        let _ = store.set_setting(SETTING_ENCODER_FALLBACK_HISTORY, &json);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderCandidatePlan {
    pub candidates: Vec<VideoEncoder>,
    pub cached_encoder: Option<VideoEncoder>,
    pub fingerprint_changed: bool,
}

/// Computes the ordered list of candidate encoders for an export run:
/// remembered encoder first when the hardware/driver fingerprint matches,
/// else the full lazy chain `h264_nvenc -> h264_qsv -> h264_amf -> h264_mf`.
pub fn ordered_encoder_candidates(
    store: &LocalStore,
    hw: &HardwareProfile,
) -> EncoderCandidatePlan {
    let cached_encoder = store
        .get_setting(SETTING_PREFERRED_ENCODER)
        .ok()
        .flatten()
        .and_then(|s| VideoEncoder::from_ffmpeg_name(&s));
    let cached_fp = store
        .get_setting(SETTING_ENCODER_FINGERPRINT)
        .ok()
        .flatten();
    let fingerprint_changed = match (cached_encoder, cached_fp.as_deref()) {
        (Some(_), Some(fp)) => fp != hw.fingerprint,
        _ => false,
    };

    let candidates = if let Some(cached) = cached_encoder {
        if !fingerprint_changed {
            let mut list = vec![cached];
            for enc in VideoEncoder::ordered_chain() {
                if *enc != cached {
                    list.push(*enc);
                }
            }
            list
        } else {
            VideoEncoder::ordered_chain().to_vec()
        }
    } else {
        VideoEncoder::ordered_chain().to_vec()
    };

    EncoderCandidatePlan {
        candidates,
        cached_encoder,
        fingerprint_changed,
    }
}

/// Persists the first working encoder + hardware/driver fingerprint so subsequent exports
/// start directly with the verified encoder.
pub fn persist_working_encoder(store: &LocalStore, hw: &HardwareProfile, encoder: VideoEncoder) {
    let _ = store.set_setting(SETTING_PREFERRED_ENCODER, encoder.as_ffmpeg_name());
    let _ = store.set_setting(SETTING_ENCODER_FINGERPRINT, &hw.fingerprint);
}

/// Returns `true` when an FFmpeg render failure happened during initialization / matches an
/// S4 encoder stderr pattern OR occurred within the first `EARLY_ENCODER_FAILURE_WINDOW_SECS`.
pub fn is_early_or_init_encoder_failure(
    failure: &EncoderProbeFailure,
    encoded_media_secs: f64,
) -> bool {
    if failure.kind != EncoderFailureKind::Unknown {
        return true;
    }
    !encoded_media_secs.is_finite() || encoded_media_secs <= EARLY_ENCODER_FAILURE_WINDOW_SECS
}

/// Records an encoder fallback event in both `encoder_fallback_history` (shown in Pengaturan &
/// Diagnostik) and SQLite `error_events`, returning the visible Indonesian status message
/// `"Encoder X gagal, mencoba Y"` when a next candidate exists.
pub fn record_runtime_encoder_fallback(
    store: &LocalStore,
    from: VideoEncoder,
    to: Option<VideoEncoder>,
    failure: &EncoderProbeFailure,
    job_id: Option<&str>,
    app_version: &str,
) -> Option<String> {
    append_fallback_history(
        store,
        &[EncoderFallbackEntry {
            from: from.as_ffmpeg_name().to_string(),
            to: to.map(|e| e.as_ffmpeg_name().to_string()),
            reason_kind: failure.kind,
            summary: failure.summary.clone(),
        }],
    );

    if let Some(next) = to {
        let notice = format!(
            "Encoder {} gagal, mencoba {}",
            from.as_ffmpeg_name(),
            next.as_ffmpeg_name()
        );
        let _ = store.record_error_event(NewErrorEvent {
            code: "E_RENDER_FAILED".to_string(),
            stage: "render_fallback".to_string(),
            job_id: job_id.map(str::to_string),
            message_key: "err.E_RENDER_FAILED.body".to_string(),
            detail: format!("{}: {}", notice, failure.summary),
            stderr_tail: failure.stderr_tail_redacted.clone(),
            app_version: app_version.to_string(),
            component_versions_json: "{}".to_string(),
        });
        Some(notice)
    } else {
        None
    }
}

/// Builds `E_ENCODER_UNAVAILABLE` after all candidate encoders have been exhausted,
/// recording `error_events` and fallback history.
pub fn build_encoder_unavailable_error(
    store: &LocalStore,
    hw: &HardwareProfile,
    attempts: Vec<EncoderProbeFailure>,
    job_id: Option<&str>,
    app_version: &str,
    record_history: bool,
) -> Box<EncoderUnavailableError> {
    if record_history {
        let fallback_records: Vec<EncoderFallbackEntry> = attempts
            .iter()
            .map(|a| EncoderFallbackEntry {
                from: a.encoder.as_ffmpeg_name().to_string(),
                to: None,
                reason_kind: a.kind,
                summary: a.summary.clone(),
            })
            .collect();
        append_fallback_history(store, &fallback_records);
    }

    let mut actions = if hw.is_windows_n_kn {
        vec![
            EncoderUnavailableAction::InstallMediaFeaturePack,
            EncoderUnavailableAction::UpdateGpuDriver,
            EncoderUnavailableAction::ExportDiagnostics,
        ]
    } else {
        vec![
            EncoderUnavailableAction::UpdateGpuDriver,
            EncoderUnavailableAction::InstallMediaFeaturePack,
            EncoderUnavailableAction::ExportDiagnostics,
        ]
    };
    actions.dedup();

    let joined_summary: Vec<String> = attempts
        .iter()
        .map(|a| format!("{}: {}", a.encoder.as_ffmpeg_name(), a.summary))
        .collect();
    let msg = format!(
        "Semua encoder video tidak tersedia ({}). Perbarui driver GPU atau pasang Media Feature Pack untuk Windows N/KN. Detail: {}",
        VideoEncoder::ordered_chain()
            .iter()
            .map(|e| e.as_ffmpeg_name())
            .collect::<Vec<_>>()
            .join(" -> "),
        joined_summary.join("; ")
    );

    let stderr_tail = attempts
        .iter()
        .filter(|a| !a.stderr_tail_redacted.is_empty())
        .map(|a| {
            format!(
                "[{}] {}",
                a.encoder.as_ffmpeg_name(),
                a.stderr_tail_redacted
            )
        })
        .collect::<Vec<_>>()
        .join("\n---\n");

    let _ = store.record_error_event(NewErrorEvent {
        code: "E_ENCODER_UNAVAILABLE".to_string(),
        stage: if job_id.is_some() {
            "render"
        } else {
            "render_probe"
        }
        .to_string(),
        job_id: job_id.map(str::to_string),
        message_key: "err.E_ENCODER_UNAVAILABLE.body".to_string(),
        detail: msg.clone(),
        stderr_tail,
        app_version: app_version.to_string(),
        component_versions_json: "{}".to_string(),
    });

    Box::new(EncoderUnavailableError {
        code: "E_ENCODER_UNAVAILABLE".to_string(),
        message_key: "err.E_ENCODER_UNAVAILABLE.body".to_string(),
        message: msg,
        is_windows_n_kn: hw.is_windows_n_kn,
        actions,
        attempts,
    })
}

/// Lazily selects or probes the encoder chain (`h264_nvenc -> h264_qsv -> h264_amf -> h264_mf`).
/// Re-probes cleanly whenever hardware/driver fingerprint changed or cached encoder fails.
pub fn select_or_probe_encoder(
    store: &LocalStore,
    hw: &HardwareProfile,
    prober: &dyn EncoderProber,
    app_version: &str,
) -> Result<EncoderSelectionResult, Box<EncoderUnavailableError>> {
    let plan = ordered_encoder_candidates(store, hw);
    let mut attempts: Vec<EncoderProbeFailure> = Vec::new();
    let mut fallback_records: Vec<EncoderFallbackEntry> = Vec::new();

    for (idx, enc) in plan.candidates.iter().enumerate() {
        match prober.probe_encoder(*enc) {
            Ok(()) => {
                persist_working_encoder(store, hw, *enc);

                let used_cached =
                    plan.cached_encoder == Some(*enc) && !plan.fingerprint_changed && idx == 0;
                let fallback_occurred = !attempts.is_empty();
                let fallback_message = if fallback_occurred {
                    let prev_names: Vec<&str> = attempts
                        .iter()
                        .map(|a| a.encoder.as_ffmpeg_name())
                        .collect();
                    for a in &attempts {
                        fallback_records.push(EncoderFallbackEntry {
                            from: a.encoder.as_ffmpeg_name().to_string(),
                            to: Some(enc.as_ffmpeg_name().to_string()),
                            reason_kind: a.kind,
                            summary: a.summary.clone(),
                        });
                    }
                    append_fallback_history(store, &fallback_records);
                    Some(format!(
                        "Encoder {} gagal ({}), mencoba {}",
                        prev_names.join(", "),
                        attempts
                            .last()
                            .map(|a| a.summary.as_str())
                            .unwrap_or("probe failed"),
                        enc.as_ffmpeg_name()
                    ))
                } else {
                    None
                };

                return Ok(EncoderSelectionResult {
                    selected: *enc,
                    used_cached,
                    fingerprint_changed: plan.fingerprint_changed,
                    fallback_occurred,
                    fallback_message,
                    attempts,
                });
            }
            Err(fail) => {
                attempts.push(fail);
            }
        }
    }

    Err(build_encoder_unavailable_error(
        store,
        hw,
        attempts,
        None,
        app_version,
        true,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::GpuAdapterInfo;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use tempfile::TempDir;

    type MockOutcome = Result<(), (Option<i32>, &'static str)>;

    struct MockProber {
        outcomes: HashMap<VideoEncoder, MockOutcome>,
        called: RefCell<Vec<VideoEncoder>>,
    }

    impl MockProber {
        fn new(outcomes: Vec<(VideoEncoder, MockOutcome)>) -> Self {
            Self {
                outcomes: outcomes.into_iter().collect(),
                called: RefCell::new(Vec::new()),
            }
        }
    }

    impl EncoderProber for MockProber {
        fn probe_encoder(&self, encoder: VideoEncoder) -> Result<(), EncoderProbeFailure> {
            self.called.borrow_mut().push(encoder);
            match self.outcomes.get(&encoder) {
                Some(Ok(())) => Ok(()),
                Some(Err((code, stderr))) => Err(classify_encoder_stderr(encoder, *code, stderr)),
                None => Err(classify_encoder_stderr(encoder, Some(1), "Unknown encoder")),
            }
        }
    }

    fn sample_hw(driver_ver: &str, is_n_kn: bool) -> HardwareProfile {
        let gpus = vec![GpuAdapterInfo {
            name: "NVIDIA GeForce GTX 1650".into(),
            driver_version: Some(driver_ver.into()),
            vendor: Some("NVIDIA".into()),
        }];
        HardwareProfile {
            cpu_model: "AMD Ryzen 5 5500U".into(),
            physical_cores: 6,
            logical_cores: 12,
            ram_bytes: Some((16u64 * 1024 * 1024 * 1024) as f64),
            windows_product: if is_n_kn {
                "Windows 11 Pro N".into()
            } else {
                "Windows 11 Pro".into()
            },
            windows_display_version: Some("24H2".into()),
            windows_build: "26100.1".into(),
            is_windows_n_kn: is_n_kn,
            webview2_version: Some("130.0.0.0".into()),
            system_locale: "id-ID".into(),
            timezone: "UTC+07:00 (SE Asia Standard Time)".into(),
            ac_power: Some(true),
            power_plan: "Balanced".into(),
            fingerprint: crate::hardware::compute_hardware_fingerprint(&gpus, "26100.1"),
            gpus,
        }
    }

    #[test]
    fn test_s4_golden_stderr_classifications() {
        let nvenc = classify_encoder_stderr(
            VideoEncoder::H264Nvenc,
            Some(1),
            "[vost#0:0/h264_nvenc @ 000001] Task finished with error code: -22 (Invalid argument)\nError opening output files: Generic error in an external library",
        );
        assert_eq!(nvenc.kind, EncoderFailureKind::DriverUnsupportedOrInitError);

        let qsv = classify_encoder_stderr(
            VideoEncoder::H264Qsv,
            Some(1),
            "[h264_qsv @ 000001] Error initializing MFX session: unsupported (-3)\n[vost#0:0/h264_qsv] Task finished with error code: -1",
        );
        assert_eq!(qsv.kind, EncoderFailureKind::DriverUnsupportedOrInitError);

        let amf = classify_encoder_stderr(
            VideoEncoder::H264Amf,
            Some(1),
            "Unknown encoder 'h264_amf'\nError opening output files: Encoder not found",
        );
        assert_eq!(amf.kind, EncoderFailureKind::EncoderNotFound);

        let mf = classify_encoder_stderr(
            VideoEncoder::H264Mf,
            Some(1),
            "[h264_mf @ 000001] Unable to parse \"profile\" option value \"high\"",
        );
        assert_eq!(mf.kind, EncoderFailureKind::MediaFoundationOrProfileError);
    }

    #[test]
    fn test_lazy_chain_remembers_working_reprobes_on_driver_change_and_falls_back() {
        let dir = TempDir::new().unwrap();
        let store = LocalStore::open(&dir.path().join("enc.db")).unwrap();
        let hw1 = sample_hw("31.0.15.4633", false);

        // 1. NVENC & QSV fail, AMF succeeds -> persists h264_amf + hw1 fingerprint + fallback notice
        let p1 = MockProber::new(vec![
            (
                VideoEncoder::H264Nvenc,
                Err((Some(1), "Cannot load nvcuda.dll")),
            ),
            (
                VideoEncoder::H264Qsv,
                Err((Some(1), "Unknown encoder 'h264_qsv'")),
            ),
            (VideoEncoder::H264Amf, Ok(())),
            (VideoEncoder::H264Mf, Ok(())),
        ]);
        let r1 = select_or_probe_encoder(&store, &hw1, &p1, "0.1.0").unwrap();
        assert_eq!(r1.selected, VideoEncoder::H264Amf);
        assert!(!r1.used_cached);
        assert!(r1.fallback_occurred);
        assert!(r1.fallback_message.as_deref().unwrap().contains("h264_amf"));
        assert_eq!(
            *p1.called.borrow(),
            vec![
                VideoEncoder::H264Nvenc,
                VideoEncoder::H264Qsv,
                VideoEncoder::H264Amf
            ]
        );

        // 2. Second run with same fingerprint tries cached h264_amf first (single probe!)
        let p2 = MockProber::new(vec![(VideoEncoder::H264Amf, Ok(()))]);
        let r2 = select_or_probe_encoder(&store, &hw1, &p2, "0.1.0").unwrap();
        assert_eq!(r2.selected, VideoEncoder::H264Amf);
        assert!(r2.used_cached);
        assert!(!r2.fingerprint_changed);
        assert_eq!(*p2.called.borrow(), vec![VideoEncoder::H264Amf]);

        // 3. GPU driver updated -> fingerprint changed -> re-probes from top of chain (`h264_nvenc` now works!)
        let hw2 = sample_hw("32.0.15.6636", false);
        let p3 = MockProber::new(vec![(VideoEncoder::H264Nvenc, Ok(()))]);
        let r3 = select_or_probe_encoder(&store, &hw2, &p3, "0.1.0").unwrap();
        assert_eq!(r3.selected, VideoEncoder::H264Nvenc);
        assert!(r3.fingerprint_changed);
        assert!(!r3.used_cached);
        assert_eq!(*p3.called.borrow(), vec![VideoEncoder::H264Nvenc]);

        // 4. Cached encoder suddenly fails mid-flight -> automatically falls back to h264_mf
        let p4 = MockProber::new(vec![
            (
                VideoEncoder::H264Nvenc,
                Err((
                    Some(-22),
                    "[vost#0:0/h264_nvenc] Task finished with error code: -22",
                )),
            ),
            (
                VideoEncoder::H264Qsv,
                Err((Some(1), "Error initializing MFX session")),
            ),
            (VideoEncoder::H264Amf, Err((Some(1), "Unknown encoder"))),
            (VideoEncoder::H264Mf, Ok(())),
        ]);
        let r4 = select_or_probe_encoder(&store, &hw2, &p4, "0.1.0").unwrap();
        assert_eq!(r4.selected, VideoEncoder::H264Mf);
        assert!(r4.fallback_occurred);
    }

    #[test]
    fn test_all_encoders_fail_returns_e_encoder_unavailable_and_records_event() {
        let dir = TempDir::new().unwrap();
        let store = LocalStore::open(&dir.path().join("enc_fail.db")).unwrap();
        let hw_n = sample_hw("31.0.15.4633", true);

        let p_fail = MockProber::new(vec![
            (
                VideoEncoder::H264Nvenc,
                Err((Some(1), "Cannot load nvcuda.dll")),
            ),
            (VideoEncoder::H264Qsv, Err((Some(1), "Unknown encoder"))),
            (VideoEncoder::H264Amf, Err((Some(1), "Unknown encoder"))),
            (
                VideoEncoder::H264Mf,
                Err((Some(1), "Failed loading mfplat.dll")),
            ),
        ]);

        let err = select_or_probe_encoder(&store, &hw_n, &p_fail, "0.1.0").unwrap_err();
        assert_eq!(err.code, "E_ENCODER_UNAVAILABLE");
        assert!(err.is_windows_n_kn);
        assert_eq!(
            err.actions[0],
            EncoderUnavailableAction::InstallMediaFeaturePack
        );
        assert!(err
            .actions
            .contains(&EncoderUnavailableAction::UpdateGpuDriver));
        assert!(err
            .actions
            .contains(&EncoderUnavailableAction::ExportDiagnostics));

        let recorded = store
            .list_error_events(Some("E_ENCODER_UNAVAILABLE"), 10)
            .unwrap();
        assert_eq!(recorded.len(), 1);
    }

    #[test]
    fn test_runtime_encoder_fallback_records_notice_history_and_error_event() {
        let dir = TempDir::new().unwrap();
        let store = LocalStore::open(&dir.path().join("rt_fb.db")).unwrap();
        let hw = sample_hw("31.0.15.4633", false);

        // Remembered encoder starts first when fingerprint matches
        persist_working_encoder(&store, &hw, VideoEncoder::H264Amf);
        let plan = ordered_encoder_candidates(&store, &hw);
        assert_eq!(plan.cached_encoder, Some(VideoEncoder::H264Amf));
        assert!(!plan.fingerprint_changed);
        assert_eq!(plan.candidates[0], VideoEncoder::H264Amf);

        // Fingerprint change resets candidate order to top of lazy chain
        let hw_updated = sample_hw("32.0.15.9999", false);
        let plan_changed = ordered_encoder_candidates(&store, &hw_updated);
        assert!(plan_changed.fingerprint_changed);
        assert_eq!(plan_changed.candidates, VideoEncoder::ordered_chain());

        // Early/init classification checks
        let s4_fail = classify_encoder_stderr(
            VideoEncoder::H264Nvenc,
            Some(-22),
            "[vost#0:0/h264_nvenc] Task finished with error code: -22",
        );
        assert!(is_early_or_init_encoder_failure(&s4_fail, 12.0));

        let unknown_early = classify_encoder_stderr(VideoEncoder::H264Amf, Some(1), "boom");
        assert!(is_early_or_init_encoder_failure(&unknown_early, 2.0));
        assert!(!is_early_or_init_encoder_failure(&unknown_early, 18.0));

        // Runtime fallback writes "Encoder X gagal, mencoba Y", error_events, and fallback history
        let notice = record_runtime_encoder_fallback(
            &store,
            VideoEncoder::H264Amf,
            Some(VideoEncoder::H264Mf),
            &s4_fail,
            Some("job-exp-1"),
            "0.1.0",
        )
        .unwrap();
        assert_eq!(notice, "Encoder h264_amf gagal, mencoba h264_mf");

        let status = get_encoder_status(&store, &hw).unwrap();
        assert_eq!(status.fallback_history.len(), 1);
        assert_eq!(status.fallback_history[0].from, "h264_amf");
        assert_eq!(status.fallback_history[0].to.as_deref(), Some("h264_mf"));

        let events = store
            .list_error_events(Some("E_RENDER_FAILED"), 10)
            .unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0]
            .detail_redacted
            .contains("Encoder h264_amf gagal, mencoba h264_mf"));
    }
}
