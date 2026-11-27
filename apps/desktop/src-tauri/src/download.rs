//! Range download stage, editing handles, cache verification, ffprobe media check,
//! and silent 403 re-probe recovery (ARCHITECTURE §5.1, §6, §9; PROGRESS_AND_ETA §4, §7, §8; S1/S1b/S1c).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::eta::{LiveEtaConfig, LiveEtaSnapshot, LiveStageEtaTracker};
use crate::jobs::{cleanup_job_temp_dir, job_temp_dir};
use crate::probe::{
    self, cleanup_probe_handle, ensure_fresh_probe, ProbeError, ProbeHandle, ProbeInfo,
    YT_FORMAT_SELECTOR,
};
use crate::progress_parser;
use crate::resolver::{ComponentResolver, ResolverError};
use crate::security;
use crate::sidecars::{SidecarEnv, SidecarError, SidecarSpec, SidecarSupervisor};
use crate::store::{LocalStore, NewErrorEvent, ProjectRecord};
use crate::url_intake;

/// Default editing handle added before/after the chosen clip range (`app_config` default = 5 s).
pub const DEFAULT_RANGE_HANDLE_SECONDS: u32 = 5;
/// Free space safety buffer required before starting a range download (500 MB per Part 2g).
pub const DOWNLOAD_DISK_BUFFER_BYTES: u64 = 500 * 1024 * 1024;
/// Fallback conservative stream bitrate estimate when probe format bitrate is absent (10 Mbps).
pub const FALLBACK_BITRATE_BYTES_PER_SEC: f64 = 1_250_000.0;
/// Maximum acceptable deviation between requested window duration and ffprobe output duration.
pub const VERIFY_DURATION_TOLERANCE_SECS: f64 = 2.0;
/// Download stage safety timeout (45 min, covering up to 10 min @ slow connection).
pub const DOWNLOAD_STAGE_TIMEOUT_SECS: u64 = 45 * 60;
/// `ffprobe` media inspection timeout.
pub const FFPROBE_TIMEOUT_SECS: u64 = 20;

#[derive(Error, Debug)]
pub enum DownloadError {
    #[error("{message}")]
    RangeInvalid { code: &'static str, message: String },

    #[error("Component missing or damaged: {message}")]
    ComponentMissing { component: String, message: String },

    #[error("Insufficient disk space: available {available_bytes} bytes, required {required_bytes} bytes")]
    DiskLow {
        available_bytes: u64,
        required_bytes: u64,
    },

    #[error("Probe error: {0}")]
    Probe(#[from] ProbeError),

    #[error("Verification failed: {reason}")]
    VerifyFailed { reason: String, detail: String },

    #[error("Network offline during download: {detail}")]
    NetOffline { detail: String, stderr_tail: String },

    #[error("Network timeout during download: {detail}")]
    NetTimeout { detail: String, stderr_tail: String },

    #[error("FFmpeg/downloader failed: {detail}")]
    SidecarFailed { detail: String, stderr_tail: String },

    #[error("Download cancelled")]
    Cancelled,

    #[error("Disk IO failed: {0}")]
    Io(String),
}

impl DownloadError {
    pub fn error_code(&self) -> &'static str {
        match self {
            DownloadError::RangeInvalid { code, .. } => code,
            DownloadError::ComponentMissing { .. } => "E_COMPONENT_MISSING",
            DownloadError::DiskLow { .. } => "E_DISK_LOW",
            DownloadError::Probe(pe) => pe.code(),
            DownloadError::VerifyFailed { .. } => "E_VERIFY_FAILED",
            DownloadError::NetOffline { .. } => "E_NET_OFFLINE",
            DownloadError::NetTimeout { .. } => "E_NET_TIMEOUT",
            DownloadError::SidecarFailed { .. } => "E_YT_EXTRACT_FAILED",
            DownloadError::Cancelled => "E_JOB_CANCELLED",
            DownloadError::Io(_) => "E_DISK_FULL",
        }
    }

    pub fn message_key(&self) -> String {
        match self {
            DownloadError::Probe(pe) => pe.message_key(),
            _ => format!("err.{}.body", self.error_code()),
        }
    }

    pub fn retryable(&self) -> bool {
        !matches!(
            self,
            DownloadError::RangeInvalid { .. }
                | DownloadError::Probe(ProbeError::Unavailable { .. })
                | DownloadError::Probe(ProbeError::UrlInvalid { .. })
                | DownloadError::Probe(ProbeError::BotCheck { .. })
                | DownloadError::Cancelled
        )
    }

    pub fn stderr_tail(&self) -> &str {
        match self {
            DownloadError::NetOffline { stderr_tail, .. }
            | DownloadError::NetTimeout { stderr_tail, .. }
            | DownloadError::SidecarFailed { stderr_tail, .. } => stderr_tail,
            DownloadError::VerifyFailed { detail, .. } => detail,
            _ => "",
        }
    }
}

/// Requested range expanded with editing handles `[start - H, end + H]` clipped to `[0, duration]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadWindow {
    pub selected_start_ms: u32,
    pub selected_end_ms: u32,
    pub requested_start_ms: u32,
    pub requested_end_ms: u32,
    pub handle_before_ms: u32,
    pub handle_after_ms: u32,
}

impl DownloadWindow {
    pub fn requested_duration_secs(&self) -> f64 {
        self.requested_end_ms
            .saturating_sub(self.requested_start_ms) as f64
            / 1000.0
    }

    pub fn selected_duration_ms(&self) -> u32 {
        self.selected_end_ms.saturating_sub(self.selected_start_ms)
    }
}

/// Computes editing-handle download window clipped safely to `[0, video_duration_ms]`.
pub fn compute_download_window(
    selected_start_ms: u32,
    selected_end_ms: u32,
    video_duration_seconds: u32,
    handle_seconds: u32,
) -> Result<DownloadWindow, DownloadError> {
    let video_duration_ms = video_duration_seconds.saturating_mul(1000);
    if selected_end_ms <= selected_start_ms
        || selected_end_ms.saturating_sub(selected_start_ms) < 1000
    {
        return Err(DownloadError::RangeInvalid {
            code: "E_RANGE_INVALID",
            message: "Rentang minimal 1 detik dan waktu akhir harus lebih besar dari waktu mulai."
                .to_string(),
        });
    }
    if video_duration_ms > 0 && selected_end_ms > video_duration_ms {
        return Err(DownloadError::RangeInvalid {
            code: "E_RANGE_INVALID",
            message: "Waktu akhir melebihi durasi video.".to_string(),
        });
    }

    let h_ms = handle_seconds.saturating_mul(1000);
    let requested_start_ms = selected_start_ms.saturating_sub(h_ms);
    let requested_end_ms = if video_duration_ms > 0 {
        selected_end_ms.saturating_add(h_ms).min(video_duration_ms)
    } else {
        selected_end_ms.saturating_add(h_ms)
    };

    Ok(DownloadWindow {
        selected_start_ms,
        selected_end_ms,
        requested_start_ms,
        requested_end_ms,
        handle_before_ms: selected_start_ms.saturating_sub(requested_start_ms),
        handle_after_ms: requested_end_ms.saturating_sub(selected_end_ms),
    })
}

/// Computes required preflight disk space: `range_s * bitrate_bytes_per_sec * 1.3 + 500 MB`.
pub fn compute_download_required_bytes(
    window: &DownloadWindow,
    est_bytes_per_sec: Option<f64>,
) -> u64 {
    let bps = est_bytes_per_sec
        .filter(|b| b.is_finite() && *b > 0.0)
        .unwrap_or(FALLBACK_BITRATE_BYTES_PER_SEC);
    let media_bytes = (window.requested_duration_secs() * bps * 1.3)
        .ceil()
        .max(1.0) as u64;
    media_bytes.saturating_add(DOWNLOAD_DISK_BUFFER_BYTES)
}

/// Computes content-addressed cache key `sha256(provider|video_id|start_ms|end_ms|format_selector|handles)`.
pub fn compute_download_cache_key(
    provider: &str,
    video_id: &str,
    window: &DownloadWindow,
    format_selector: &str,
) -> String {
    let mut hasher = Sha256::new();
    let payload = format!(
        "{}|{}|{}|{}|{}|{}:{}",
        provider,
        video_id,
        window.requested_start_ms,
        window.requested_end_ms,
        format_selector,
        window.handle_before_ms,
        window.handle_after_ms
    );
    hasher.update(payload.as_bytes());
    hex::encode(hasher.finalize())
}

/// Formats milliseconds as `HH:MM:SS` (or `HH:MM:SS.mmm` when fractional ms present) for yt-dlp `--download-sections`.
pub fn format_ytdlp_section_timestamp(ms: u32) -> String {
    let total_secs = ms / 1000;
    let rem_ms = ms % 1000;
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    if rem_ms == 0 {
        format!("{:02}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}:{:02}.{:03}", h, m, s, rem_ms)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerifiedMediaInfo {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration_ms: u32,
    pub video_codec: String,
    pub audio_codec: String,
    pub format_name: String,
    pub size_bytes: u64,
    pub is_webview_native: bool,
}

#[derive(Debug, Deserialize)]
struct FfprobeRawOutput {
    #[serde(default)]
    streams: Vec<FfprobeRawStream>,
    format: Option<FfprobeRawFormat>,
}

#[derive(Debug, Deserialize)]
struct FfprobeRawStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FfprobeRawFormat {
    format_name: Option<String>,
    duration: Option<String>,
    size: Option<String>,
}

fn parse_fps_ratio(raw: Option<&str>) -> Option<f64> {
    let s = raw?.trim();
    if let Some((num_s, den_s)) = s.split_once('/') {
        let num: f64 = num_s.parse().ok()?;
        let den: f64 = den_s.parse().ok()?;
        if den > 0.0 && num > 0.0 && num.is_finite() && den.is_finite() {
            return Some((num / den * 1000.0).round() / 1000.0);
        }
        return None;
    }
    let val: f64 = s.parse().ok()?;
    if val > 0.0 && val.is_finite() {
        Some(val)
    } else {
        None
    }
}

/// Parses & validates `ffprobe -print_format json -show_format -show_streams` output.
pub fn validate_ffprobe_output(
    raw_json: &[u8],
    file_size_on_disk: u64,
    expected_duration_secs: Option<f64>,
) -> Result<VerifiedMediaInfo, DownloadError> {
    if file_size_on_disk == 0 {
        return Err(DownloadError::VerifyFailed {
            reason: "File hasil unduhan kosong.".to_string(),
            detail: "Downloaded output file is 0 bytes".to_string(),
        });
    }

    let parsed: FfprobeRawOutput =
        serde_json::from_slice(raw_json).map_err(|e| DownloadError::VerifyFailed {
            reason: "Metadata media tidak dapat dibaca ffprobe.".to_string(),
            detail: format!("Invalid ffprobe JSON: {}", e),
        })?;

    let video_stream = parsed
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"))
        .ok_or_else(|| DownloadError::VerifyFailed {
            reason: "File hasil unduhan tidak memiliki stream video.".to_string(),
            detail: "ffprobe reported no video stream".to_string(),
        })?;

    let audio_stream = parsed
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("audio"))
        .ok_or_else(|| DownloadError::VerifyFailed {
            reason: "File hasil unduhan tidak memiliki stream audio.".to_string(),
            detail: "ffprobe reported no audio stream".to_string(),
        })?;

    let width = video_stream.width.unwrap_or(0);
    let height = video_stream.height.unwrap_or(0);
    if width == 0 || height == 0 {
        return Err(DownloadError::VerifyFailed {
            reason: "Dimensi video tidak valid.".to_string(),
            detail: format!("Invalid ffprobe video dimensions {}x{}", width, height),
        });
    }

    let fps = parse_fps_ratio(video_stream.avg_frame_rate.as_deref())
        .or_else(|| parse_fps_ratio(video_stream.r_frame_rate.as_deref()))
        .unwrap_or(30.0);

    let format = parsed.format.ok_or_else(|| DownloadError::VerifyFailed {
        reason: "Container media tidak dikenali ffprobe.".to_string(),
        detail: "Missing ffprobe format block".to_string(),
    })?;

    let format_name = format.format_name.unwrap_or_default().to_ascii_lowercase();
    let container_playable = format_name.contains("mp4")
        || format_name.contains("mov")
        || format_name.contains("matroska")
        || format_name.contains("webm");
    if !container_playable {
        return Err(DownloadError::VerifyFailed {
            reason: "Format container video tidak valid atau korup.".to_string(),
            detail: format!("Unplayable container '{}'", format_name),
        });
    }

    let duration_secs: f64 = format
        .duration
        .as_deref()
        .and_then(|d| d.parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d > 0.0)
        .ok_or_else(|| DownloadError::VerifyFailed {
            reason: "Durasi file hasil unduhan tidak valid.".to_string(),
            detail: "Missing or zero duration in ffprobe format".to_string(),
        })?;

    if let Some(expected) = expected_duration_secs {
        let diff = (duration_secs - expected).abs();
        if diff > VERIFY_DURATION_TOLERANCE_SECS {
            return Err(DownloadError::VerifyFailed {
                reason: format!(
                    "Durasi unduhan ({:.1} dtk) tidak sesuai rentang yang diminta ({:.1} dtk).",
                    duration_secs, expected
                ),
                detail: format!(
                    "Duration mismatch: actual={:.3}s expected={:.3}s tolerance={:.1}s",
                    duration_secs, expected, VERIFY_DURATION_TOLERANCE_SECS
                ),
            });
        }
    }

    let video_codec = video_stream
        .codec_name
        .as_deref()
        .unwrap_or("unknown")
        .to_ascii_lowercase();
    let audio_codec = audio_stream
        .codec_name
        .as_deref()
        .unwrap_or("unknown")
        .to_ascii_lowercase();

    let is_mp4 = format_name.contains("mp4") || format_name.contains("mov");
    let is_h264 = video_codec == "h264" || video_codec.starts_with("avc");
    let is_aac = audio_codec == "aac" || audio_codec.starts_with("mp4a");

    let probed_size = format
        .size
        .as_deref()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(file_size_on_disk);

    Ok(VerifiedMediaInfo {
        width,
        height,
        fps,
        duration_ms: (duration_secs * 1000.0).round() as u32,
        video_codec,
        audio_codec,
        format_name,
        size_bytes: probed_size.max(file_size_on_disk),
        is_webview_native: is_mp4 && is_h264 && is_aac,
    })
}

/// Runs `ffprobe` via `SidecarSupervisor` against `media_path` and validates streams/container/duration.
pub async fn verify_media_with_ffprobe(
    ffprobe_bin: &Path,
    ws_root: &Path,
    media_path: &Path,
    expected_duration_secs: Option<f64>,
    cancel_token: Option<&CancellationToken>,
) -> Result<VerifiedMediaInfo, DownloadError> {
    let meta = std::fs::metadata(media_path).map_err(|e| DownloadError::VerifyFailed {
        reason: "File hasil unduhan tidak ditemukan untuk verifikasi.".to_string(),
        detail: e.to_string(),
    })?;
    let size_on_disk = meta.len();

    let env = SidecarEnv::for_component("ffmpeg", ws_root);
    env.ensure_writable_dirs()
        .map_err(|e| DownloadError::Io(e.to_string()))?;

    let args = vec![
        "-v".to_string(),
        "quiet".to_string(),
        "-print_format".to_string(),
        "json".to_string(),
        "-show_format".to_string(),
        "-show_streams".to_string(),
        media_path.to_string_lossy().to_string(),
    ];

    let spec = SidecarSpec::new(ffprobe_bin.to_path_buf(), args)
        .with_component_env(&env)
        .map_err(|e| DownloadError::Io(e.to_string()))?
        .timeout(Duration::from_secs(FFPROBE_TIMEOUT_SECS))
        .max_total_bytes(4 * 1024 * 1024);

    let result = SidecarSupervisor::new()
        .run_to_completion(&spec, cancel_token)
        .await
        .map_err(|e| match e {
            SidecarError::Cancelled => DownloadError::Cancelled,
            other => DownloadError::VerifyFailed {
                reason: "Gagal memeriksa file media dengan ffprobe.".to_string(),
                detail: security::redact_sensitive_text(&other.to_string()),
            },
        })?;

    validate_ffprobe_output(&result.stdout, size_on_disk, expected_duration_secs)
}

/// Classifies yt-dlp / FFmpeg download stderr into typed `DownloadError` taxonomy (Part 2h).
pub fn classify_download_sidecar_failure(err: SidecarError) -> DownloadError {
    match err {
        SidecarError::Cancelled => DownloadError::Cancelled,
        SidecarError::Timeout(d) => DownloadError::NetTimeout {
            detail: format!("Download timed out after {} s", d.as_secs()),
            stderr_tail: String::new(),
        },
        SidecarError::ExecutionFailed {
            exit_code,
            stderr_tail,
        } => {
            let lower = stderr_tail.to_ascii_lowercase();
            // Network timeout / disconnect / DNS / TLS patterns map strictly to E_NET_TIMEOUT / E_NET_OFFLINE
            if lower.contains("timed out") || lower.contains("read operation timed out") {
                return DownloadError::NetTimeout {
                    detail: format!("Network timeout during download (exit {:?})", exit_code),
                    stderr_tail,
                };
            }
            if lower.contains("connection reset")
                || lower.contains("connection refused")
                || lower.contains("connection aborted")
                || lower.contains("failed to resolve")
                || lower.contains("getaddrinfo")
                || lower.contains("name or service not known")
                || lower.contains("network is unreachable")
                || lower.contains("temporary failure in name resolution")
                || lower.contains("ssl")
                || lower.contains("tls")
                || lower.contains("eof occurred")
            {
                return DownloadError::NetOffline {
                    detail: format!(
                        "Network offline or reset during download (exit {:?})",
                        exit_code
                    ),
                    stderr_tail,
                };
            }
            if lower.contains("no space left on device") || lower.contains("disk full") {
                return DownloadError::Io("Disk penuh saat menulis potongan video.".to_string());
            }

            // Otherwise reuse canonical YouTube extractor / HTTP >= 400 classifier
            let probe_err = probe::classify_yt_failure(None, &stderr_tail);
            match probe_err {
                ProbeError::BotCheck { .. }
                | ProbeError::RateLimited { .. }
                | ProbeError::Unavailable { .. }
                | ProbeError::LiveNotEnded { .. } => DownloadError::Probe(probe_err),
                _ => DownloadError::SidecarFailed {
                    detail: format!("yt-dlp exited with {:?}", exit_code),
                    stderr_tail,
                },
            }
        }
        other => DownloadError::SidecarFailed {
            detail: security::redact_sensitive_text(&other.to_string()),
            stderr_tail: String::new(),
        },
    }
}

pub fn is_http_403_stderr(stderr_tail: &str) -> bool {
    let lower = stderr_tail.to_ascii_lowercase();
    lower.contains("http error 403") || lower.contains("403 forbidden")
}

/// Builds the supervised `SidecarSpec` for copy-mode range download (`--download-sections` WITHOUT `--force-keyframes-at-cuts`).
#[allow(clippy::too_many_arguments)]
pub fn build_ytdlp_download_spec(
    yt_dlp_bin: PathBuf,
    qjs_bin: PathBuf,
    ffmpeg_dir: PathBuf,
    ws_root: &Path,
    working_dir: &Path,
    probe_json_path: &Path,
    output_mp4_path: &Path,
    canonical_url: &str,
    window: &DownloadWindow,
    extra_env: &[(String, String)],
) -> Result<SidecarSpec, DownloadError> {
    if !yt_dlp_bin.is_absolute() || !qjs_bin.is_absolute() || !ffmpeg_dir.is_absolute() {
        return Err(DownloadError::ComponentMissing {
            component: "yt-dlp".to_string(),
            message: "Executable component paths must be absolute".to_string(),
        });
    }

    let sidecar_env = SidecarEnv::for_component("yt-dlp", ws_root);
    sidecar_env
        .ensure_writable_dirs()
        .map_err(|e| DownloadError::Io(e.to_string()))?;
    let cache_dir = sidecar_env
        .cache_dir
        .clone()
        .unwrap_or_else(|| ws_root.join("cache").join("yt-dlp"));

    let section_spec = format!(
        "*{}-{}",
        format_ytdlp_section_timestamp(window.requested_start_ms),
        format_ytdlp_section_timestamp(window.requested_end_ms)
    );

    let args = vec![
        "--no-playlist".to_string(),
        "--no-warnings".to_string(),
        "--newline".to_string(),
        "--ignore-config".to_string(),
        "--load-info-json".to_string(),
        probe_json_path.to_string_lossy().to_string(),
        "--ffmpeg-location".to_string(),
        ffmpeg_dir.to_string_lossy().to_string(),
        "--js-runtimes".to_string(),
        format!("quickjs:{}", qjs_bin.to_string_lossy()),
        "--cache-dir".to_string(),
        cache_dir.to_string_lossy().to_string(),
        "-f".to_string(),
        YT_FORMAT_SELECTOR.to_string(),
        "--download-sections".to_string(),
        section_spec,
        "--merge-output-format".to_string(),
        "mp4".to_string(),
        "-o".to_string(),
        output_mp4_path.to_string_lossy().to_string(),
        "--".to_string(),
        canonical_url.to_string(),
    ];

    let mut spec = SidecarSpec::new(yt_dlp_bin, args)
        .working_dir(working_dir)
        .with_component_env(&sidecar_env)
        .map_err(|e| DownloadError::Io(e.to_string()))?
        .timeout(Duration::from_secs(DOWNLOAD_STAGE_TIMEOUT_SECS))
        .max_total_bytes(16 * 1024 * 1024);

    for (k, v) in extra_env {
        spec = spec.env(k, v);
    }

    Ok(spec)
}

#[derive(Debug, Clone)]
pub struct DownloadStageProgress {
    pub snapshot: LiveEtaSnapshot,
    pub speed_ratio: Option<f64>,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct DownloadStageOutcome {
    pub cache_key: String,
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub window: DownloadWindow,
    pub media: VerifiedMediaInfo,
    pub cache_hit: bool,
    pub wall_ms: u64,
    pub reprobed_after_403: bool,
}

/// Ensures `ProbeHandle` is deleted on exit (success, error, or cancellation); never logs probe contents.
struct ProbeHandleDropGuard {
    handle: Option<ProbeHandle>,
}

impl ProbeHandleDropGuard {
    fn new(handle: Option<ProbeHandle>) -> Self {
        Self { handle }
    }

    fn take(&mut self) -> Option<ProbeHandle> {
        self.handle.take()
    }

    fn replace(&mut self, handle: ProbeHandle) {
        if let Some(old) = self.handle.replace(handle) {
            let _ = cleanup_probe_handle(&old);
        }
    }
}

impl Drop for ProbeHandleDropGuard {
    fn drop(&mut self) {
        if let Some(ref h) = self.handle {
            let _ = cleanup_probe_handle(h);
        }
    }
}

fn epoch_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn map_resolver_err(e: ResolverError) -> DownloadError {
    match e {
        ResolverError::Missing(adv) => DownloadError::ComponentMissing {
            component: adv.component,
            message: adv.message,
        },
        other => DownloadError::ComponentMissing {
            component: "sidecar".to_string(),
            message: other.to_string(),
        },
    }
}

/// Executes the `download` stage:
/// - Validates range, components, and free disk space
/// - Checks cache hit first (`cache/<cache_key>.mp4` + `ffprobe` valid)
/// - Ensures fresh `ProbeHandle` (`--load-info-json`), runs yt-dlp copy-mode section download
///   into `cache/tmp/<job_id>/` with live progress/ETA + 2 s stall ticks + silent single retry on HTTP 403
/// - Verifies output with `ffprobe`, atomically moves to `cache/<cache_key>.mp4`, deletes probe JSON,
///   and records `stage_stats`.
#[allow(clippy::too_many_arguments)]
pub async fn run_download_stage<F>(
    ws_root: &Path,
    store: &LocalStore,
    resolver: &ComponentResolver,
    device_profile: &str,
    job_id: &str,
    project: &ProjectRecord,
    selected_start_ms: u32,
    selected_end_ms: u32,
    max_range_seconds: u32,
    handle_seconds: u32,
    initial_probe: Option<ProbeHandle>,
    cancel_token: &CancellationToken,
    on_progress: F,
) -> Result<DownloadStageOutcome, DownloadError>
where
    F: Fn(DownloadStageProgress) + Send + Sync + 'static,
{
    run_download_stage_with_env(
        ws_root,
        store,
        resolver,
        device_profile,
        job_id,
        project,
        selected_start_ms,
        selected_end_ms,
        max_range_seconds,
        handle_seconds,
        initial_probe,
        None,
        &[],
        cancel_token,
        on_progress,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_download_stage_with_env<F>(
    ws_root: &Path,
    store: &LocalStore,
    resolver: &ComponentResolver,
    device_profile: &str,
    job_id: &str,
    project: &ProjectRecord,
    selected_start_ms: u32,
    selected_end_ms: u32,
    max_range_seconds: u32,
    handle_seconds: u32,
    initial_probe: Option<ProbeHandle>,
    free_space_override: Option<u64>,
    extra_sidecar_env: &[(String, String)],
    cancel_token: &CancellationToken,
    on_progress: F,
) -> Result<DownloadStageOutcome, DownloadError>
where
    F: Fn(DownloadStageProgress) + Send + Sync + 'static,
{
    if cancel_token.is_cancelled() {
        return Err(DownloadError::Cancelled);
    }

    let range_ms = selected_end_ms.saturating_sub(selected_start_ms);
    if range_ms > max_range_seconds.saturating_mul(1000) {
        return Err(DownloadError::RangeInvalid {
            code: "E_RANGE_TOO_LONG",
            message: format!(
                "Durasi rentang melebihi batas maksimal {} menit ({} detik).",
                max_range_seconds / 60,
                max_range_seconds
            ),
        });
    }

    let window = compute_download_window(
        selected_start_ms,
        selected_end_ms,
        project.duration_seconds,
        handle_seconds,
    )?;

    // Resolve & verify all required sidecar components up front
    let yt_dlp_res = resolver
        .resolve_executable("yt-dlp", "yt-dlp.exe")
        .map_err(map_resolver_err)?;
    let qjs_res = resolver
        .resolve_executable("qjs", "qjs.exe")
        .map_err(map_resolver_err)?;
    let ffmpeg_res = resolver
        .resolve_executable("ffmpeg", "ffmpeg.exe")
        .map_err(map_resolver_err)?;
    let ffprobe_res = resolver
        .resolve_executable("ffmpeg", "ffprobe.exe")
        .map_err(map_resolver_err)?;

    let cache_key = compute_download_cache_key(
        &project.source_provider,
        &project.source_video_id,
        &window,
        YT_FORMAT_SELECTOR,
    );
    let final_rel_path = format!("cache/{}.mp4", cache_key);
    let final_abs_path = security::resolve_workspace_path_with_root(ws_root, &final_rel_path)
        .map_err(|e| DownloadError::Io(e.to_string()))?;

    let mut probe_guard = ProbeHandleDropGuard::new(initial_probe);

    // Cache hit fast path: verify existing cached MP4 cleanly with ffprobe
    if final_abs_path.is_file() {
        if let Ok(verified) = verify_media_with_ffprobe(
            &ffprobe_res.path,
            ws_root,
            &final_abs_path,
            Some(window.requested_duration_secs()),
            Some(cancel_token),
        )
        .await
        {
            drop(probe_guard);
            return Ok(DownloadStageOutcome {
                cache_key,
                rel_path: final_rel_path,
                abs_path: final_abs_path,
                window,
                media: verified,
                cache_hit: true,
                wall_ms: 0,
                reprobed_after_403: false,
            });
        }
    }

    // Disk preflight before network download
    let parsed_yt = url_intake::parse_youtube_url(&project.source_url).map_err(|e| {
        DownloadError::RangeInvalid {
            code: "E_YT_URL_INVALID",
            message: e.to_string(),
        }
    })?;

    let (probe_info, active_probe_handle) = match probe_guard.take() {
        Some(h) => {
            let yt_p = yt_dlp_res.path.clone();
            let qjs_p = qjs_res.path.clone();
            let ws_owned = ws_root.to_path_buf();
            let env_owned = extra_sidecar_env.to_vec();
            let tok = cancel_token.clone();
            ensure_fresh_probe(ws_root, h, false, |p| async move {
                run_raw_probe_json(&yt_p, &qjs_p, &ws_owned, &p, &env_owned, &tok).await
            })
            .await?
        }
        None => {
            let (info, bytes) = run_raw_probe_json(
                &yt_dlp_res.path,
                &qjs_res.path,
                ws_root,
                &parsed_yt,
                extra_sidecar_env,
                cancel_token,
            )
            .await?;
            let h = probe::persist_probe_json(
                ws_root,
                &parsed_yt.video_id,
                &parsed_yt.canonical_url,
                &bytes,
            )?;
            (info, h)
        }
    };
    probe_guard.replace(active_probe_handle.clone());

    let required_bytes =
        compute_download_required_bytes(&window, probe_info.estimated_bytes_per_second);
    let available_bytes = match free_space_override {
        Some(a) => a,
        None => {
            security::get_free_disk_space(ws_root).map_err(|e| DownloadError::Io(e.to_string()))?
        }
    };
    if available_bytes < required_bytes {
        return Err(DownloadError::DiskLow {
            available_bytes,
            required_bytes,
        });
    }

    let tmp_dir = job_temp_dir(ws_root, job_id).map_err(|e| DownloadError::Io(e.to_string()))?;
    std::fs::create_dir_all(&tmp_dir).map_err(|e| DownloadError::Io(e.to_string()))?;
    let tmp_output = tmp_dir.join(format!("{}.mp4", cache_key));

    let ffmpeg_dir = ffmpeg_res
        .path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| ws_root.to_path_buf());

    let started = Instant::now();
    let tracker = Arc::new(Mutex::new(LiveStageEtaTracker::new(
        LiveEtaConfig::for_download(
            Some(store),
            device_profile,
            window.requested_duration_secs(),
        ),
        epoch_ms_now(),
    )));
    let on_progress_arc = Arc::new(on_progress);

    let mut current_probe_handle = active_probe_handle;
    let mut reprobed_after_403 = false;

    let download_res = loop {
        let spec = build_ytdlp_download_spec(
            yt_dlp_res.path.clone(),
            qjs_res.path.clone(),
            ffmpeg_dir.clone(),
            ws_root,
            &tmp_dir,
            current_probe_handle.abs_path(),
            &tmp_output,
            &parsed_yt.canonical_url,
            &window,
            extra_sidecar_env,
        )?;

        let tracker_out = tracker.clone();
        let cb_out = on_progress_arc.clone();
        let target_secs = window.requested_duration_secs();
        let tracker_err = tracker.clone();
        let cb_err = on_progress_arc.clone();
        let tracker_tick = tracker.clone();
        let cb_tick = on_progress_arc.clone();

        let handle_line = move |line: String, tr: &Arc<Mutex<LiveStageEtaTracker>>, cb: &Arc<F>| {
            if let Some(parsed_line) =
                progress_parser::parse_progress_line(&line, Some(target_secs))
            {
                let reported_done = parsed_line
                    .time_s
                    .or_else(|| parsed_line.percent.map(|p| (p / 100.0) * target_secs));
                if let Ok(mut guard) = tr.lock() {
                    let snap = guard.update(epoch_ms_now(), reported_done);
                    cb(DownloadStageProgress {
                        snapshot: snap,
                        speed_ratio: parsed_line.speed_ratio,
                        size_bytes: parsed_line.size_bytes,
                    });
                }
            }
        };

        let sidecar_res = SidecarSupervisor::new()
            .spawn_streaming_with_hooks(
                &spec,
                move |line| handle_line(line, &tracker_out, &cb_out),
                move |line| handle_line(line, &tracker_err, &cb_err),
                move || {
                    if let Ok(mut guard) = tracker_tick.lock() {
                        let snap = guard.update(epoch_ms_now(), None);
                        cb_tick(DownloadStageProgress {
                            snapshot: snap,
                            speed_ratio: None,
                            size_bytes: None,
                        });
                    }
                },
                Some(cancel_token),
            )
            .await;

        match sidecar_res {
            Ok(res) if res.truncated => {
                break Err(DownloadError::SidecarFailed {
                    detail: "Sidecar output exceeded safety cap".to_string(),
                    stderr_tail: res.stderr_tail,
                });
            }
            Ok(_) => break Ok(()),
            Err(SidecarError::Cancelled) => {
                cleanup_job_temp_dir(ws_root, job_id);
                break Err(DownloadError::Cancelled);
            }
            Err(SidecarError::ExecutionFailed {
                exit_code,
                stderr_tail,
            }) if !reprobed_after_403 && is_http_403_stderr(&stderr_tail) => {
                reprobed_after_403 = true;
                tracing::warn!(
                    "yt-dlp hit HTTP 403 (exit {:?}); silently re-probing once before retry",
                    exit_code
                );
                let _ = std::fs::remove_dir_all(&tmp_dir);
                let _ = std::fs::create_dir_all(&tmp_dir);

                let yt_p = yt_dlp_res.path.clone();
                let qjs_p = qjs_res.path.clone();
                let ws_owned = ws_root.to_path_buf();
                let env_owned = extra_sidecar_env.to_vec();
                let tok = cancel_token.clone();
                let (_fresh_info, refreshed_handle) = ensure_fresh_probe(
                    ws_root,
                    current_probe_handle.clone(),
                    true,
                    |p| async move {
                        run_raw_probe_json(&yt_p, &qjs_p, &ws_owned, &p, &env_owned, &tok).await
                    },
                )
                .await?;
                current_probe_handle = refreshed_handle;
                probe_guard.replace(current_probe_handle.clone());
                continue;
            }
            Err(SidecarError::ExecutionFailed {
                exit_code,
                stderr_tail,
            }) if reprobed_after_403 && is_http_403_stderr(&stderr_tail) => {
                let _ = exit_code;
                break Err(DownloadError::Probe(ProbeError::RateLimited {
                    detail: "HTTP 403 retry exhausted".to_string(),
                }));
            }
            Err(other) => {
                break Err(classify_download_sidecar_failure(other));
            }
        }
    };

    // Always delete probe JSON immediately after download attempt (Part 2i)
    drop(probe_guard);

    if let Err(err) = download_res {
        cleanup_job_temp_dir(ws_root, job_id);
        return Err(err);
    }

    // Verify downloaded file via ffprobe before moving to final cache location
    let verified = match verify_media_with_ffprobe(
        &ffprobe_res.path,
        ws_root,
        &tmp_output,
        Some(window.requested_duration_secs()),
        Some(cancel_token),
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            cleanup_job_temp_dir(ws_root, job_id);
            return Err(e);
        }
    };

    if let Some(parent) = final_abs_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| DownloadError::Io(e.to_string()))?;
    }
    if final_abs_path.exists() {
        let _ = std::fs::remove_file(&final_abs_path);
    }
    std::fs::rename(&tmp_output, &final_abs_path).map_err(|e| {
        cleanup_job_temp_dir(ws_root, job_id);
        DownloadError::Io(e.to_string())
    })?;
    cleanup_job_temp_dir(ws_root, job_id);

    let wall_ms = started.elapsed().as_millis() as u64;
    let _ = store.insert_stage_stat(
        device_profile,
        "download",
        "yt-dlp",
        window.requested_duration_secs(),
        wall_ms.max(1) as i64,
    );

    Ok(DownloadStageOutcome {
        cache_key,
        rel_path: final_rel_path,
        abs_path: final_abs_path,
        window,
        media: verified,
        cache_hit: false,
        wall_ms,
        reprobed_after_403,
    })
}

async fn run_raw_probe_json(
    yt_dlp_bin: &Path,
    qjs_bin: &Path,
    ws_root: &Path,
    parsed: &url_intake::ParsedYouTubeUrl,
    extra_env: &[(String, String)],
    cancel_token: &CancellationToken,
) -> Result<(ProbeInfo, Vec<u8>), ProbeError> {
    let mut spec = probe::build_probe_spec(
        yt_dlp_bin.to_path_buf(),
        qjs_bin.to_path_buf(),
        ws_root,
        parsed,
    )?;
    for (k, v) in extra_env {
        spec = spec.env(k, v);
    }
    let res = SidecarSupervisor::new()
        .run_to_completion(&spec, Some(cancel_token))
        .await
        .map_err(|e| match e {
            SidecarError::Cancelled => ProbeError::Cancelled,
            SidecarError::Timeout(d) => ProbeError::NetTimeout {
                detail: format!("Probe timed out after {} s", d.as_secs()),
            },
            SidecarError::ExecutionFailed { stderr_tail, .. } => {
                probe::classify_yt_failure(None, &stderr_tail)
            }
            other => ProbeError::Internal {
                detail: security::redact_sensitive_text(&other.to_string()),
            },
        })?;
    let info =
        probe::parse_and_validate_probe_json(&res.stdout, &res.stderr_tail, &parsed.video_id)?;
    Ok((info, res.stdout))
}

/// Records download/job failure cleanly in `error_events` with redacted stderr tail.
pub fn record_stage_error(
    store: &LocalStore,
    job_id: &str,
    stage: &str,
    err: &DownloadError,
    app_version: &str,
) {
    let _ = store.record_error_event(NewErrorEvent {
        code: err.error_code().to_string(),
        stage: stage.to_string(),
        job_id: Some(job_id.to_string()),
        message_key: err.message_key(),
        detail: security::redact_sensitive_text(&err.to_string()),
        stderr_tail: security::redact_sensitive_text(err.stderr_tail()),
        app_version: app_version.to_string(),
        component_versions_json: "{}".to_string(),
    });
}

/// Ephemeral in-memory map storing latest `ProbeHandle` per `(provider, video_id)` from Beranda probe
/// so M1.4 generate reuses it (`--load-info-json`) and cleans it up cleanly afterwards.
#[derive(Default, Clone)]
pub struct ProbeHandleCache {
    entries: Arc<Mutex<HashMap<String, ProbeHandle>>>,
}

impl ProbeHandleCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, provider: &str, video_id: &str, handle: ProbeHandle) {
        if let Ok(mut guard) = self.entries.lock() {
            if let Some(prev) = guard.insert(format!("{}:{}", provider, video_id), handle) {
                let _ = cleanup_probe_handle(&prev);
            }
        }
    }

    pub fn take(&self, provider: &str, video_id: &str) -> Option<ProbeHandle> {
        self.entries
            .lock()
            .ok()
            .and_then(|mut g| g.remove(&format!("{}:{}", provider, video_id)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installer::InUseRegistry;
    use tempfile::TempDir;

    pub(crate) fn setup_fake_components(ws_root: &Path, store: &LocalStore) -> PathBuf {
        use crate::archive::ExtractedFileRecord;
        use crate::installer::InstallReceipt;
        use sha2::Digest;

        let fake_bin = crate::sidecars::test_fake_sidecar_bin();
        assert!(
            fake_bin.exists(),
            "fake-sidecar binary must exist: {:?}",
            fake_bin
        );

        let fake_bytes = std::fs::read(&fake_bin).unwrap();
        let fake_sha = hex::encode(sha2::Sha256::digest(&fake_bytes));
        let fake_size = fake_bytes.len() as u64;

        for (comp, ver, exe) in [
            ("yt-dlp", "2026.10.26", "yt-dlp.exe"),
            ("qjs", "v0.18.0", "qjs.exe"),
            ("ffmpeg", "n9.0.2", "ffmpeg.exe"),
        ] {
            let dir = ws_root.join("bin").join(comp).join(ver);
            std::fs::create_dir_all(&dir).unwrap();
            let dst = dir.join(exe);
            std::fs::copy(&fake_bin, &dst).unwrap();
            let mut files = vec![ExtractedFileRecord {
                path: exe.to_string(),
                sha256: fake_sha.clone(),
                size_bytes: fake_size,
            }];
            if comp == "ffmpeg" {
                let ffprobe_dst = dir.join("ffprobe.exe");
                std::fs::copy(&fake_bin, &ffprobe_dst).unwrap();
                files.push(ExtractedFileRecord {
                    path: "ffprobe.exe".to_string(),
                    sha256: fake_sha.clone(),
                    size_bytes: fake_size,
                });
            }
            let files_json = serde_json::to_string(&files).unwrap();
            let receipt = InstallReceipt {
                schema: 1,
                component: comp.to_string(),
                version: ver.to_string(),
                kind: "bin".to_string(),
                manifest_sequence: 1,
                files,
                installed_at: 1_700_000_000,
            };
            std::fs::write(
                dir.join("install.json"),
                serde_json::to_vec(&receipt).unwrap(),
            )
            .unwrap();
            store
                .set_component_installed_current(
                    comp,
                    ver,
                    "bin",
                    &format!("bin/{}/{}", comp, ver),
                    1,
                    &files_json,
                    None,
                    1_700_000_000,
                )
                .unwrap();
        }
        fake_bin
    }

    fn sample_project() -> ProjectRecord {
        ProjectRecord {
            id: uuid::Uuid::new_v4().to_string(),
            title: "Sample VOD".into(),
            channel: "Creator".into(),
            source_provider: "youtube".into(),
            source_video_id: "WufkeDaJisQ".into(),
            source_url: "https://www.youtube.com/watch?v=WufkeDaJisQ".into(),
            duration_seconds: 600,
            live_status: Some("not_live".into()),
            preview_mode: "embed".into(),
            suggested_start_seconds: None,
            range_start_ms: Some(10_000.0),
            range_end_ms: Some(70_000.0),
            status: "ranged".into(),
            composition_hash: None,
            created_at: 1_700_000_000.0,
            updated_at: 1_700_000_000.0,
        }
    }

    #[test]
    fn test_handle_clipping_at_zero_and_end_plus_cache_key_stability() {
        let w_start = compute_download_window(2_000, 32_000, 600, 5).unwrap();
        assert_eq!(w_start.requested_start_ms, 0);
        assert_eq!(w_start.requested_end_ms, 37_000);
        assert_eq!(w_start.handle_before_ms, 2_000);
        assert_eq!(w_start.handle_after_ms, 5_000);

        let w_end = compute_download_window(550_000, 598_000, 600, 5).unwrap();
        assert_eq!(w_end.requested_start_ms, 545_000);
        assert_eq!(w_end.requested_end_ms, 600_000);
        assert_eq!(w_end.handle_before_ms, 5_000);
        assert_eq!(w_end.handle_after_ms, 2_000);

        let k1 = compute_download_cache_key("youtube", "WufkeDaJisQ", &w_start, YT_FORMAT_SELECTOR);
        let k2 = compute_download_cache_key("youtube", "WufkeDaJisQ", &w_start, YT_FORMAT_SELECTOR);
        let k3 = compute_download_cache_key("youtube", "WufkeDaJisQ", &w_end, YT_FORMAT_SELECTOR);
        assert_eq!(k1, k2);
        assert_ne!(k1, k3);
        assert_eq!(k1.len(), 64);

        assert_eq!(format_ytdlp_section_timestamp(5_000), "00:00:05");
        assert_eq!(format_ytdlp_section_timestamp(3_661_500), "01:01:01.500");
    }

    #[test]
    fn test_ffprobe_verification_rules() {
        let valid_avc1 = br#"{"streams":[{"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"avg_frame_rate":"60/1"},{"codec_type":"audio","codec_name":"aac"}],"format":{"format_name":"mov,mp4,m4a","duration":"70.2","size":"5242880"}}"#;
        let ok = validate_ffprobe_output(valid_avc1, 5_242_880, Some(70.0)).unwrap();
        assert!(ok.is_webview_native);
        assert_eq!(ok.width, 1920);
        assert_eq!(ok.fps, 60.0);

        let vp9 = br#"{"streams":[{"codec_type":"video","codec_name":"vp9","width":1920,"height":1080,"avg_frame_rate":"30/1"},{"codec_type":"audio","codec_name":"opus"}],"format":{"format_name":"matroska,webm","duration":"70.0","size":"4000000"}}"#;
        let non_native = validate_ffprobe_output(vp9, 4_000_000, Some(70.0)).unwrap();
        assert!(!non_native.is_webview_native);

        // Duration off by > 2 s -> E_VERIFY_FAILED
        let err_dur = validate_ffprobe_output(valid_avc1, 5_242_880, Some(60.0)).unwrap_err();
        assert_eq!(err_dur.error_code(), "E_VERIFY_FAILED");

        // Missing audio -> E_VERIFY_FAILED
        let no_audio = br#"{"streams":[{"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"avg_frame_rate":"30/1"}],"format":{"format_name":"mp4","duration":"70.0","size":"1000"}}"#;
        let err_audio = validate_ffprobe_output(no_audio, 1000, Some(70.0)).unwrap_err();
        assert_eq!(err_audio.error_code(), "E_VERIFY_FAILED");

        // 0 bytes -> E_VERIFY_FAILED
        assert_eq!(
            validate_ffprobe_output(valid_avc1, 0, Some(70.0))
                .unwrap_err()
                .error_code(),
            "E_VERIFY_FAILED"
        );
    }

    #[tokio::test]
    async fn test_download_stage_fake_sidecar_success_cache_hit_403_errors_and_disk_low() {
        let tmp = TempDir::new().unwrap();
        let ws = security::ensure_workspace_with_root(tmp.path()).unwrap();
        let store = LocalStore::open(&ws.join("clipper.db")).unwrap();
        let _ = setup_fake_components(&ws, &store);
        let resolver = ComponentResolver::new(ws.clone(), store.clone(), InUseRegistry::new());
        let proj = sample_project();

        let probe_handle =
            probe::persist_probe_json(&ws, "WufkeDaJisQ", &proj.source_url, br#"{"id":"WufkeDaJisQ","title":"VOD","duration":600,"requested_formats":[{"vcodec":"avc1.64002A","url":"https://example.invalid/v?expire=9999999999"}]}"#)
                .unwrap();
        let probe_path = probe_handle.abs_path().to_path_buf();

        // 1. Disk-low preflight check rejects with byte numbers
        let disk_err = run_download_stage_with_env(
            &ws,
            &store,
            &resolver,
            "prof1",
            "job-disk-low",
            &proj,
            10_000,
            70_000,
            300,
            5,
            Some(probe_handle.clone()),
            Some(100 * 1024 * 1024), // 100 MB < ~500 MB + media
            &[],
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(disk_err.error_code(), "E_DISK_LOW");

        // Re-create probe handle for real success run (previous was dropped/deleted)
        let probe_handle = probe::persist_probe_json(
            &ws,
            "WufkeDaJisQ",
            &proj.source_url,
            br#"{"id":"WufkeDaJisQ","title":"VOD","duration":600,"requested_formats":[{"vcodec":"avc1.64002A","url":"https://example.invalid/v?expire=9999999999"}]}"#,
        )
        .unwrap();
        let probe_path_2 = probe_handle.abs_path().to_path_buf();

        // 2. Success + progress + probe JSON deleted + cache hit on repeat + stage_stats recorded
        let progress_count = Arc::new(Mutex::new(0usize));
        let pc_clone = progress_count.clone();
        let env_ok = vec![
            ("CLIPPER_FAKE_YTDLP_SCENARIO".into(), "success".into()),
            ("CLIPPER_FAKE_FFPROBE_SCENARIO".into(), "avc1".into()),
            ("CLIPPER_FAKE_FFPROBE_DURATION".into(), "70.0".into()),
        ];
        let out = run_download_stage_with_env(
            &ws,
            &store,
            &resolver,
            "prof1",
            "job-ok-1",
            &proj,
            10_000,
            70_000,
            300,
            5,
            Some(probe_handle),
            None,
            &env_ok,
            &CancellationToken::new(),
            move |_| {
                *pc_clone.lock().unwrap() += 1;
            },
        )
        .await
        .unwrap();
        assert!(!out.cache_hit);
        assert!(out.abs_path.exists());
        assert!(!probe_path.exists());
        assert!(
            !probe_path_2.exists(),
            "probe JSON must be deleted after download stage"
        );
        assert!(*progress_count.lock().unwrap() > 0);
        assert!(store
            .get_median_rate("prof1", "download", "yt-dlp", 10)
            .unwrap()
            .is_some());

        // Repeat -> instant cache hit
        let out_hit = run_download_stage_with_env(
            &ws,
            &store,
            &resolver,
            "prof1",
            "job-ok-2",
            &proj,
            10_000,
            70_000,
            300,
            5,
            None,
            None,
            &env_ok,
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        assert!(out_hit.cache_hit);
        assert_eq!(out_hit.cache_key, out.cache_key);

        // Remove cached file so error modes actually execute yt-dlp
        std::fs::remove_file(&out.abs_path).unwrap();

        // 3. 403 once -> silent re-probe -> success
        let marker = tmp.path().join("403.marker");
        let env_403_retry = vec![
            (
                "CLIPPER_FAKE_YTDLP_SCENARIO".into(),
                "403_then_success".into(),
            ),
            (
                "CLIPPER_FAKE_403_MARKER".into(),
                marker.to_string_lossy().to_string(),
            ),
            ("CLIPPER_FAKE_FFPROBE_SCENARIO".into(), "avc1".into()),
            ("CLIPPER_FAKE_FFPROBE_DURATION".into(), "70.0".into()),
        ];
        let p403 = run_download_stage_with_env(
            &ws,
            &store,
            &resolver,
            "prof1",
            "job-403-retry",
            &proj,
            10_000,
            70_000,
            300,
            5,
            None,
            None,
            &env_403_retry,
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        assert!(p403.reprobed_after_403);
        std::fs::remove_file(&p403.abs_path).unwrap();

        // 4. Error modes: 429, bot_check, disconnect, flood
        for (scenario, expected_code) in [
            ("429", "E_YT_RATE_LIMITED"),
            ("bot_check", "E_YT_BOT_CHECK"),
            ("disconnect", "E_NET_OFFLINE"),
            ("flood", "E_YT_EXTRACT_FAILED"),
        ] {
            let env_err = vec![("CLIPPER_FAKE_YTDLP_SCENARIO".into(), scenario.into())];
            let err = run_download_stage_with_env(
                &ws,
                &store,
                &resolver,
                "prof1",
                &format!("job-{}", scenario),
                &proj,
                10_000,
                70_000,
                300,
                5,
                None,
                None,
                &env_err,
                &CancellationToken::new(),
                |_| {},
            )
            .await
            .unwrap_err();
            assert_eq!(err.error_code(), expected_code, "scenario={}", scenario);
        }

        // 5. Invalid UTF-8 & slow start still parse progress & succeed
        for scenario in ["invalid_utf8", "slow_start"] {
            let env_ok2 = vec![
                ("CLIPPER_FAKE_YTDLP_SCENARIO".into(), scenario.into()),
                ("CLIPPER_FAKE_FFPROBE_SCENARIO".into(), "avc1".into()),
                ("CLIPPER_FAKE_FFPROBE_DURATION".into(), "70.0".into()),
            ];
            let ok2 = run_download_stage_with_env(
                &ws,
                &store,
                &resolver,
                "prof1",
                &format!("job-{}", scenario),
                &proj,
                10_000,
                70_000,
                300,
                5,
                None,
                None,
                &env_ok2,
                &CancellationToken::new(),
                |_| {},
            )
            .await
            .unwrap();
            std::fs::remove_file(&ok2.abs_path).unwrap();
        }
    }

    #[tokio::test]
    async fn test_download_cancel_kills_process_tree_and_cleans_temp_dir() {
        let tmp = TempDir::new().unwrap();
        let ws = security::ensure_workspace_with_root(tmp.path()).unwrap();
        let store = LocalStore::open(&ws.join("clipper.db")).unwrap();
        let _ = setup_fake_components(&ws, &store);
        let resolver = ComponentResolver::new(ws.clone(), store.clone(), InUseRegistry::new());
        let proj = sample_project();

        let token = CancellationToken::new();
        let tok_cancel = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
            tok_cancel.cancel();
        });

        let env_tree = vec![("CLIPPER_FAKE_YTDLP_SCENARIO".into(), "tree_child".into())];
        let err = run_download_stage_with_env(
            &ws,
            &store,
            &resolver,
            "prof1",
            "job-cancel-tree",
            &proj,
            10_000,
            70_000,
            300,
            5,
            None,
            None,
            &env_tree,
            &token,
            |_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(err.error_code(), "E_JOB_CANCELLED");
        assert!(
            !job_temp_dir(&ws, "job-cancel-tree").unwrap().exists(),
            "Temp directory must be deleted on cancel"
        );
    }

    /// Local-only live 30 s range download test against `https://youtu.be/WufkeDaJisQ`.
    /// Run manually with:
    /// `$env:CLIPPER_LIVE_YT_TEST="1"; cargo test --lib download::tests::test_local_live_range_download_and_ffprobe -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn test_local_live_range_download_and_ffprobe() {
        if std::env::var("CLIPPER_LIVE_YT_TEST").ok().as_deref() != Some("1") {
            return;
        }
        let ws = security::ensure_workspace().expect("ensure workspace");
        let store = LocalStore::open(&ws.join("clipper.db")).expect("open store");
        let resolver = ComponentResolver::new(ws.clone(), store.clone(), InUseRegistry::new());
        let parsed = url_intake::parse_youtube_url("https://youtu.be/WufkeDaJisQ").unwrap();
        let tok = CancellationToken::new();
        let probe_res = probe::probe_youtube_url(
            &ws,
            &resolver,
            "https://youtu.be/WufkeDaJisQ",
            Some(&tok),
            |_| {},
        )
        .await
        .expect("live probe");

        let proj = crate::store::ProjectRepo::new(&store)
            .create_from_probe(&parsed, &probe_res.info)
            .expect("create project");
        let out = run_download_stage(
            &ws,
            &store,
            &resolver,
            "live-test",
            &format!("live-dl-{}", uuid::Uuid::new_v4()),
            &proj,
            30_000,
            60_000,
            300,
            5,
            Some(probe_res.handle),
            &CancellationToken::new(),
            |p| {
                println!(
                    "progress: startup={} done={:.1}s frac={:.2}",
                    p.snapshot.is_startup_indeterminate, p.snapshot.done_units, p.snapshot.fraction
                );
            },
        )
        .await
        .expect("live download should succeed");

        assert!(out.abs_path.exists());
        assert!(out.media.width > 0 && out.media.height > 0);
        println!("Verified live download media: {:?}", out.media);
    }
}
