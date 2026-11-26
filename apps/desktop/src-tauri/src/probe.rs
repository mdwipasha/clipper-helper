//! Production YouTube metadata probe, multi-signal error classification,
//! opaque temporary `ProbeHandle` lifecycle, and download-time estimation.
//!
//! Security & privacy invariants (SECURITY §9, ARCHITECTURE §6):
//! - Spawns `yt-dlp` ONLY via `SidecarSupervisor` + argument arrays (`--ignore-config`,
//!   `--cache-dir`, `--js-runtimes quickjs:<abs>`, `-f <S1 selector>`, `--`, `<canonical>`).
//! - Canonical URL is constructed strictly from the validated 11-char video ID.
//! - Raw `-J` output contains expiring signed format URLs: it is persisted ONLY as a
//!   temporary file under `cache/probe/<uuid>.json`, never returned to the UI, never logged,
//!   and never included in diagnostics reports.
//! - Untrusted video titles & channel names are stripped of control/bidi characters,
//!   whitespace-collapsed, and truncated to 200 Unicode scalar values.

use crate::resolver::{ComponentResolver, ResolverError};
use crate::security;
use crate::sidecars::{SidecarEnv, SidecarError, SidecarResult, SidecarSpec, SidecarSupervisor};
use crate::url_intake::{self, ParsedYouTubeUrl, UrlInvalidReason};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

/// S1 benchmark pinned format selector (avc1 <=1080p + mp4a preferred, clean <=1080p fallback).
pub const YT_FORMAT_SELECTOR: &str =
    "bv*[vcodec^=avc1][height<=1080]+ba[acodec^=mp4a]/b[vcodec^=avc1][height<=1080]/bv*[height<=1080]+ba/b[height<=1080]";

/// Interactive probe timeout per PROGRESS_AND_ETA §2.1.
pub const PROBE_TIMEOUT_SECS: u64 = 30;

/// Maximum stdout accepted from `yt-dlp -J` (20 MB).
pub const PROBE_STDOUT_CAP_BYTES: usize = 20 * 1024 * 1024;

/// Maximum title/channel character length exposed to UI & storage.
pub const PROBE_MAX_TITLE_CHARS: usize = 200;

/// Stale temporary probe JSON files older than 1 hour are pruned at startup.
pub const PROBE_STALE_TTL: Duration = Duration::from_secs(3600);

/// Safety margin before an `expire=` timestamp in a format URL is treated as stale.
pub const PROBE_URL_EXPIRY_SAFETY_MARGIN_SECS: u64 = 60;

/// Minimum interval between emitted progress events (`<= 4 events/s` per PROGRESS_AND_ETA §3).
pub const PROBE_MIN_EMIT_INTERVAL: Duration = Duration::from_millis(250);

/// Synthetic heartbeat interval while probe is waiting on network/JS extraction.
pub const PROBE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);

/// Configurable prior constants (seeded from `app_config`, benchmark S1 = 1.66x RT + 10s).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
pub struct DownloadEtaConfig {
    pub rt_factor: f64,
    pub startup_seconds: f64,
}

impl Default for DownloadEtaConfig {
    fn default() -> Self {
        Self {
            rt_factor: 1.66,
            startup_seconds: 10.0,
        }
    }
}

/// Estimates initial download time in seconds (`range_seconds / rt_factor + startup_seconds`).
/// Uses conservative S1 priors (`1.66x RT + 10 s`) until calibrated by `stage_stats`.
pub fn estimate_download_seconds(range_seconds: f64, cfg: &DownloadEtaConfig) -> f64 {
    if !range_seconds.is_finite()
        || range_seconds <= 0.0
        || !cfg.rt_factor.is_finite()
        || cfg.rt_factor <= 0.0
    {
        return cfg.startup_seconds.max(0.0);
    }
    let startup = if cfg.startup_seconds.is_finite() && cfg.startup_seconds >= 0.0 {
        cfg.startup_seconds
    } else {
        10.0
    };
    (range_seconds / cfg.rt_factor + startup).max(0.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PreviewMode {
    Embed,
    TimecodeOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum YtUnavailableReason {
    MembersOnly,
    AgeRestricted,
    RegionBlocked,
    Private,
    Deleted,
    Unknown,
}

impl YtUnavailableReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MembersOnly => "members_only",
            Self::AgeRestricted => "age_restricted",
            Self::RegionBlocked => "region_blocked",
            Self::Private => "private",
            Self::Deleted => "deleted",
            Self::Unknown => "unknown",
        }
    }

    pub fn message_key(self) -> &'static str {
        match self {
            Self::MembersOnly => "err.E_YT_UNAVAILABLE.members_only",
            Self::AgeRestricted => "err.E_YT_UNAVAILABLE.age_restricted",
            Self::RegionBlocked => "err.E_YT_UNAVAILABLE.region_blocked",
            Self::Private => "err.E_YT_UNAVAILABLE.private",
            Self::Deleted => "err.E_YT_UNAVAILABLE.deleted",
            Self::Unknown => "err.E_YT_UNAVAILABLE.unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum LiveNotEndedReason {
    OngoingLive,
    PremiereNotStarted,
    PostLiveProcessing,
}

impl LiveNotEndedReason {
    pub fn message_key(self) -> &'static str {
        match self {
            Self::OngoingLive => "err.E_YT_LIVE_NOT_ENDED.ongoing_live",
            Self::PremiereNotStarted => "err.E_YT_LIVE_NOT_ENDED.premiere_not_started",
            Self::PostLiveProcessing => "err.E_YT_LIVE_NOT_ENDED.post_live",
        }
    }
}

/// Typed video metadata extracted from `yt-dlp -J`. Never includes format URLs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct ProbeInfo {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub duration_seconds: u32,
    pub live_status: Option<String>,
    pub availability: Option<String>,
    pub playable_in_embed: bool,
    pub was_live: bool,
    pub is_live: bool,
    pub estimated_bytes_per_second: Option<f64>,
    pub has_avc1_1080p: bool,
    pub preview_mode: PreviewMode,
}

/// Opaque handle referencing the temporary on-disk probe JSON (`cache/probe/<uuid>.json`).
/// Passed to downstream pipeline stages (`M1.4`); never serialized to the UI or diagnostics.
#[derive(Debug, Clone)]
pub struct ProbeHandle {
    probe_id: String,
    rel_path: String,
    abs_path: PathBuf,
    video_id: String,
    canonical_url: String,
}

impl ProbeHandle {
    pub fn probe_id(&self) -> &str {
        &self.probe_id
    }

    pub fn rel_path(&self) -> &str {
        &self.rel_path
    }

    pub fn abs_path(&self) -> &Path {
        &self.abs_path
    }

    pub fn video_id(&self) -> &str {
        &self.video_id
    }

    pub fn canonical_url(&self) -> &str {
        &self.canonical_url
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProbeError {
    #[error("[E_YT_URL_INVALID] {message}")]
    UrlInvalid {
        reason: UrlInvalidReason,
        message_key: String,
        message: String,
    },
    #[error("[E_YT_UNAVAILABLE] ({reason:?}) {detail}")]
    Unavailable {
        reason: YtUnavailableReason,
        detail: String,
    },
    #[error("[E_YT_LIVE_NOT_ENDED] ({reason:?}) {detail}")]
    LiveNotEnded {
        reason: LiveNotEndedReason,
        detail: String,
    },
    #[error("[E_YT_BOT_CHECK] {detail}")]
    BotCheck { detail: String },
    #[error("[E_YT_RATE_LIMITED] {detail}")]
    RateLimited { detail: String },
    #[error("[E_NET_OFFLINE] {detail}")]
    NetOffline { detail: String },
    #[error("[E_NET_TIMEOUT] {detail}")]
    NetTimeout { detail: String },
    #[error("[E_YT_EXTRACT_FAILED] {detail}")]
    ExtractFailed { detail: String },
    #[error("[E_COMPONENT_MISSING] {detail}")]
    ComponentMissing { detail: String },
    #[error("[E_APP_INIT_FAILED] {detail}")]
    Internal { detail: String },
    #[error("Probe cancelled")]
    Cancelled,
}

impl ProbeError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UrlInvalid { .. } => "E_YT_URL_INVALID",
            Self::Unavailable { .. } => "E_YT_UNAVAILABLE",
            Self::LiveNotEnded { .. } => "E_YT_LIVE_NOT_ENDED",
            Self::BotCheck { .. } => "E_YT_BOT_CHECK",
            Self::RateLimited { .. } => "E_YT_RATE_LIMITED",
            Self::NetOffline { .. } => "E_NET_OFFLINE",
            Self::NetTimeout { .. } => "E_NET_TIMEOUT",
            Self::ExtractFailed { .. } => "E_YT_EXTRACT_FAILED",
            Self::ComponentMissing { .. } => "E_COMPONENT_MISSING",
            Self::Internal { .. } => "E_APP_INIT_FAILED",
            Self::Cancelled => "E_UNKNOWN",
        }
    }

    pub fn message_key(&self) -> String {
        match self {
            Self::UrlInvalid { message_key, .. } => message_key.clone(),
            Self::Unavailable { reason, .. } => reason.message_key().to_string(),
            Self::LiveNotEnded { reason, .. } => reason.message_key().to_string(),
            Self::BotCheck { .. } => "err.E_YT_BOT_CHECK".to_string(),
            Self::RateLimited { .. } => "err.E_YT_RATE_LIMITED".to_string(),
            Self::NetOffline { .. } => "err.E_NET_OFFLINE".to_string(),
            Self::NetTimeout { .. } => "err.E_NET_TIMEOUT".to_string(),
            Self::ExtractFailed { .. } => "err.E_YT_EXTRACT_FAILED".to_string(),
            Self::ComponentMissing { .. } => "err.E_COMPONENT_MISSING".to_string(),
            Self::Internal { .. } => "err.E_APP_INIT_FAILED".to_string(),
            Self::Cancelled => "home.probeCancelled".to_string(),
        }
    }

    pub fn unavailable_reason(&self) -> Option<YtUnavailableReason> {
        match self {
            Self::Unavailable { reason, .. } => Some(*reason),
            _ => None,
        }
    }

    pub fn redacted_detail(&self) -> String {
        match self {
            Self::UrlInvalid { message, .. } => security::redact_sensitive_text(message),
            Self::Unavailable { detail, .. }
            | Self::LiveNotEnded { detail, .. }
            | Self::BotCheck { detail }
            | Self::RateLimited { detail }
            | Self::NetOffline { detail }
            | Self::NetTimeout { detail }
            | Self::ExtractFailed { detail }
            | Self::ComponentMissing { detail }
            | Self::Internal { detail } => security::redact_sensitive_text(detail),
            Self::Cancelled => "Probe cancelled by user".to_string(),
        }
    }
}

impl From<url_intake::UrlIntakeError> for ProbeError {
    fn from(err: url_intake::UrlIntakeError) -> Self {
        let message_key = err.message_key().to_string();
        let reason = err.reason();
        let msg = match &err {
            url_intake::UrlIntakeError::Invalid { message, .. } => message.clone(),
        };
        Self::UrlInvalid {
            reason,
            message_key,
            message: msg,
        }
    }
}

/// Progress update emitted by the probe stage (mapped to `JobEvent` / `ProbeProgressEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct ProbeProgressUpdate {
    pub operation: String,
    pub stage: String,
    pub indeterminate: bool,
    pub elapsed_ms: f64,
    pub heartbeat: bool,
}

/// Successful probe output carrying typed UI-safe metadata, normalized intake URL, and opaque handle.
#[derive(Debug, Clone)]
pub struct ProbeOutcome {
    pub parsed_url: ParsedYouTubeUrl,
    pub info: ProbeInfo,
    pub handle: ProbeHandle,
    pub raw_json_bytes: usize,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct YtDlpRawFormat {
    #[serde(default)]
    vcodec: Option<String>,
    #[serde(default)]
    acodec: Option<String>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    filesize: Option<u64>,
    #[serde(default)]
    filesize_approx: Option<u64>,
    #[serde(default)]
    tbr: Option<f64>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    manifest_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct YtDlpRawProbe {
    #[serde(rename = "_type", default)]
    entry_type: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    uploader: Option<String>,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    live_status: Option<String>,
    #[serde(default)]
    availability: Option<String>,
    #[serde(default)]
    playable_in_embed: Option<bool>,
    #[serde(default)]
    was_live: Option<bool>,
    #[serde(default)]
    is_live: Option<bool>,
    #[serde(default)]
    vcodec: Option<String>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    filesize: Option<u64>,
    #[serde(default)]
    filesize_approx: Option<u64>,
    #[serde(default)]
    tbr: Option<f64>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    manifest_url: Option<String>,
    #[serde(default)]
    requested_formats: Vec<YtDlpRawFormat>,
    #[serde(default)]
    formats: Vec<YtDlpRawFormat>,
}

/// Signals extracted from `yt-dlp -J` JSON used by the classifier BEFORE stderr pattern matching.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProbeJsonSignals {
    pub entry_type: Option<String>,
    pub live_status: Option<String>,
    pub availability: Option<String>,
    pub is_live: Option<bool>,
}

/// Strips ASCII/Unicode control, zero-width, and bidi override characters,
/// collapses internal whitespace to single spaces, and truncates to `max_chars`.
pub fn sanitize_untrusted_text(raw: &str, max_chars: usize) -> String {
    let mut cleaned = String::with_capacity(raw.len().min(max_chars * 4));
    let mut prev_space = true; // trims leading spaces naturally

    for c in raw.chars() {
        if c.is_whitespace() {
            if !prev_space {
                cleaned.push(' ');
                prev_space = true;
            }
        } else if is_disallowed_control_or_bidi(c) {
            continue;
        } else {
            cleaned.push(c);
            prev_space = false;
        }
    }

    while cleaned.ends_with(' ') {
        cleaned.pop();
    }

    if cleaned.chars().count() > max_chars {
        cleaned.chars().take(max_chars).collect()
    } else {
        cleaned
    }
}

fn is_disallowed_control_or_bidi(c: char) -> bool {
    if c.is_control() {
        return true;
    }
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}'
    )
}

/// Builds the supervised `SidecarSpec` for probing a canonical YouTube watch URL with `yt-dlp` and `qjs`.
pub fn build_probe_spec(
    yt_dlp_bin: PathBuf,
    qjs_bin: PathBuf,
    ws_root: &Path,
    parsed: &ParsedYouTubeUrl,
) -> Result<SidecarSpec, ProbeError> {
    if !yt_dlp_bin.is_absolute() || !qjs_bin.is_absolute() {
        return Err(ProbeError::Internal {
            detail: "Sidecar executable paths must be absolute".to_string(),
        });
    }

    let sidecar_env = SidecarEnv::for_component("yt-dlp", ws_root);
    sidecar_env
        .ensure_writable_dirs()
        .map_err(|e| ProbeError::Internal {
            detail: format!("Failed to prepare yt-dlp sandbox dirs: {}", e),
        })?;

    let cache_dir = sidecar_env
        .cache_dir
        .clone()
        .unwrap_or_else(|| ws_root.join("cache").join("yt-dlp"));

    // Always rebuild the target URL strictly from the verified 11-char ID.
    let safe_canonical_url = format!("https://www.youtube.com/watch?v={}", parsed.video_id);
    let qjs_runtime_arg = format!("quickjs:{}", qjs_bin.to_string_lossy());
    let cache_dir_arg = cache_dir.to_string_lossy().to_string();

    let args = vec![
        "-J".to_string(),
        "--no-playlist".to_string(),
        "--no-warnings".to_string(),
        "--ignore-config".to_string(),
        "--js-runtimes".to_string(),
        qjs_runtime_arg,
        "--cache-dir".to_string(),
        cache_dir_arg,
        "-f".to_string(),
        YT_FORMAT_SELECTOR.to_string(),
        "--".to_string(),
        safe_canonical_url,
    ];

    let spec = SidecarSpec::new(yt_dlp_bin, args)
        .with_component_env(&sidecar_env)
        .map_err(|e| ProbeError::Internal {
            detail: format!("Failed to apply sidecar environment: {}", e),
        })?
        .timeout(Duration::from_secs(PROBE_TIMEOUT_SECS))
        .max_total_bytes(PROBE_STDOUT_CAP_BYTES);

    Ok(spec)
}

fn format_is_avc1_le_1080p(f: &YtDlpRawFormat) -> bool {
    let vcodec = f.vcodec.as_deref().unwrap_or("");
    let height = f.height.unwrap_or(0);
    vcodec.starts_with("avc1") && (1..=1080).contains(&height)
}

fn estimate_bytes_per_sec_for_format(f: &YtDlpRawFormat, duration_secs: f64) -> Option<f64> {
    if duration_secs > 0.0 {
        if let Some(bytes) = f.filesize.or(f.filesize_approx) {
            if bytes > 0 {
                return Some(bytes as f64 / duration_secs);
            }
        }
    }
    if let Some(tbr_kbps) = f.tbr {
        if tbr_kbps.is_finite() && tbr_kbps > 0.0 {
            return Some((tbr_kbps * 1000.0) / 8.0);
        }
    }
    let _ = &f.acodec;
    None
}

fn compute_estimated_bytes_per_second(raw: &YtDlpRawProbe, duration_secs: f64) -> Option<f64> {
    if !raw.requested_formats.is_empty() {
        let mut total_bps = 0.0;
        let mut any = false;
        for f in &raw.requested_formats {
            if let Some(bps) = estimate_bytes_per_sec_for_format(f, duration_secs) {
                total_bps += bps;
                any = true;
            }
        }
        if any {
            return Some(total_bps.round());
        }
    }

    if let Some(avc1) = raw.formats.iter().find(|f| format_is_avc1_le_1080p(f)) {
        if let Some(bps) = estimate_bytes_per_sec_for_format(avc1, duration_secs) {
            return Some(bps.round());
        }
    }

    if duration_secs > 0.0 {
        if let Some(bytes) = raw.filesize.or(raw.filesize_approx) {
            if bytes > 0 {
                return Some((bytes as f64 / duration_secs).round());
            }
        }
    }
    raw.tbr
        .filter(|t| t.is_finite() && *t > 0.0)
        .map(|tbr| ((tbr * 1000.0) / 8.0).round())
}

/// Parses raw `yt-dlp -J` JSON bytes, classifies any blocking signals (live, playlist, availability),
/// and constructs typed UI-safe `ProbeInfo` without format URLs.
pub fn parse_and_validate_probe_json(
    raw_bytes: &[u8],
    stderr_tail: &str,
    expected_video_id: &str,
) -> Result<ProbeInfo, ProbeError> {
    let raw: YtDlpRawProbe =
        serde_json::from_slice(raw_bytes).map_err(|e| ProbeError::ExtractFailed {
            detail: format!("Failed to parse probe JSON: {}", e),
        })?;

    let signals = ProbeJsonSignals {
        entry_type: raw.entry_type.clone(),
        live_status: raw.live_status.clone(),
        availability: raw.availability.clone(),
        is_live: raw.is_live,
    };

    if let Some(blocked) = classify_json_signals(&signals, stderr_tail) {
        return Err(blocked);
    }

    let id = raw
        .id
        .as_deref()
        .and_then(crate::store::sanitize_youtube_video_id)
        .unwrap_or_else(|| expected_video_id.to_string());

    let title = {
        let s = sanitize_untrusted_text(
            raw.title.as_deref().unwrap_or("Untitled YouTube Video"),
            PROBE_MAX_TITLE_CHARS,
        );
        if s.is_empty() {
            "Untitled YouTube Video".to_string()
        } else {
            s
        }
    };

    let channel_raw = raw
        .channel
        .as_deref()
        .or(raw.uploader.as_deref())
        .unwrap_or("Unknown Channel");
    let channel = {
        let s = sanitize_untrusted_text(channel_raw, PROBE_MAX_TITLE_CHARS);
        if s.is_empty() {
            "Unknown Channel".to_string()
        } else {
            s
        }
    };

    let duration_f64 = raw.duration.unwrap_or(0.0);
    let duration_seconds = if duration_f64.is_finite() && duration_f64 > 0.0 {
        duration_f64.round().clamp(0.0, u32::MAX as f64) as u32
    } else {
        0
    };

    let playable_in_embed = raw.playable_in_embed.unwrap_or(false);
    let was_live = raw.was_live.unwrap_or(false)
        || raw
            .live_status
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case("was_live"));
    let is_live = raw.is_live.unwrap_or(false);

    let has_top_avc1 = raw.vcodec.as_deref().is_some_and(|v| v.starts_with("avc1"))
        && raw.height.is_some_and(|h| (1..=1080).contains(&h));

    let has_avc1_1080p = has_top_avc1
        || raw.requested_formats.iter().any(format_is_avc1_le_1080p)
        || raw.formats.iter().any(format_is_avc1_le_1080p);

    let estimated_bytes_per_second = compute_estimated_bytes_per_second(&raw, duration_f64);
    let preview_mode = if playable_in_embed {
        PreviewMode::Embed
    } else {
        PreviewMode::TimecodeOnly
    };

    Ok(ProbeInfo {
        id,
        title,
        channel,
        duration_seconds,
        live_status: raw.live_status,
        availability: raw.availability,
        playable_in_embed,
        was_live,
        is_live,
        estimated_bytes_per_second,
        has_avc1_1080p,
        preview_mode,
    })
}

/// Checks probe JSON status signals (`_type`, `live_status`, `is_live`, `availability`) BEFORE
/// falling back to stderr regexes, matching PROGRESS_AND_ETA §8.1.
pub fn classify_json_signals(signals: &ProbeJsonSignals, stderr_tail: &str) -> Option<ProbeError> {
    let redacted_detail = security::redact_sensitive_text(stderr_tail);

    if signals.entry_type.as_deref().is_some_and(|t| {
        t.eq_ignore_ascii_case("playlist") || t.eq_ignore_ascii_case("multi_video")
    }) {
        return Some(ProbeError::UrlInvalid {
            reason: UrlInvalidReason::PlaylistOrChannelNotSupported,
            message_key: UrlInvalidReason::PlaylistOrChannelNotSupported
                .message_key()
                .to_string(),
            message: "URL resolved to a playlist or collection instead of a single video."
                .to_string(),
        });
    }

    if signals.is_live == Some(true) {
        return Some(ProbeError::LiveNotEnded {
            reason: LiveNotEndedReason::OngoingLive,
            detail: if redacted_detail.is_empty() {
                "probe_json.is_live == true".to_string()
            } else {
                redacted_detail
            },
        });
    }

    if let Some(ref ls) = signals.live_status {
        match ls.to_ascii_lowercase().as_str() {
            "is_live" => {
                return Some(ProbeError::LiveNotEnded {
                    reason: LiveNotEndedReason::OngoingLive,
                    detail: if redacted_detail.is_empty() {
                        "probe_json.live_status == is_live".to_string()
                    } else {
                        redacted_detail
                    },
                });
            }
            "is_upcoming" => {
                return Some(ProbeError::LiveNotEnded {
                    reason: LiveNotEndedReason::PremiereNotStarted,
                    detail: if redacted_detail.is_empty() {
                        "probe_json.live_status == is_upcoming".to_string()
                    } else {
                        redacted_detail
                    },
                });
            }
            "post_live" => {
                return Some(ProbeError::LiveNotEnded {
                    reason: LiveNotEndedReason::PostLiveProcessing,
                    detail: if redacted_detail.is_empty() {
                        "probe_json.live_status == post_live".to_string()
                    } else {
                        redacted_detail
                    },
                });
            }
            _ => {}
        }
    }

    if let Some(ref avail) = signals.availability {
        match avail.to_ascii_lowercase().as_str() {
            "public" | "unlisted" => {}
            "subscriber_only" | "premium_only" => {
                return Some(ProbeError::Unavailable {
                    reason: YtUnavailableReason::MembersOnly,
                    detail: if redacted_detail.is_empty() {
                        format!("probe_json.availability == {}", avail)
                    } else {
                        redacted_detail
                    },
                });
            }
            "private" => {
                return Some(ProbeError::Unavailable {
                    reason: YtUnavailableReason::Private,
                    detail: if redacted_detail.is_empty() {
                        "probe_json.availability == private".to_string()
                    } else {
                        redacted_detail
                    },
                });
            }
            "needs_auth" => {
                let reason = infer_unavailable_reason_from_stderr(stderr_tail)
                    .unwrap_or(YtUnavailableReason::AgeRestricted);
                return Some(ProbeError::Unavailable {
                    reason,
                    detail: if redacted_detail.is_empty() {
                        "probe_json.availability == needs_auth".to_string()
                    } else {
                        redacted_detail
                    },
                });
            }
            other => {
                let reason = infer_unavailable_reason_from_stderr(stderr_tail)
                    .unwrap_or(YtUnavailableReason::Unknown);
                return Some(ProbeError::Unavailable {
                    reason,
                    detail: if redacted_detail.is_empty() {
                        format!("probe_json.availability == {}", other)
                    } else {
                        redacted_detail
                    },
                });
            }
        }
    }

    None
}

fn infer_unavailable_reason_from_stderr(stderr: &str) -> Option<YtUnavailableReason> {
    let lower = stderr.to_ascii_lowercase();

    if lower.contains("members-only")
        || lower.contains("members only")
        || lower.contains("join this channel")
    {
        return Some(YtUnavailableReason::MembersOnly);
    }

    if lower.contains("confirm your age")
        || lower.contains("age-restricted")
        || lower.contains("inappropriate for some users")
    {
        return Some(YtUnavailableReason::AgeRestricted);
    }

    if lower.contains("available in your country")
        || lower.contains("blocked in your country")
        || lower.contains("blocked it in your country")
        || lower.contains("geo-restricted")
        || lower.contains("not available in your country")
    {
        return Some(YtUnavailableReason::RegionBlocked);
    }

    if lower.contains("private video") {
        return Some(YtUnavailableReason::Private);
    }

    if lower.contains("has been removed")
        || lower.contains("no longer available")
        || lower.contains("has been terminated")
        || lower.contains("isn't available anymore")
        || lower.contains("video unavailable")
    {
        return Some(YtUnavailableReason::Deleted);
    }

    None
}

/// Multi-signal classification combining optional JSON signals with stderr output from `yt-dlp`.
/// Matches the S1b/S1c precedence rules in `docs/PROGRESS_AND_ETA.md §8.1`.
pub fn classify_yt_failure(json_signals: Option<&ProbeJsonSignals>, stderr: &str) -> ProbeError {
    if let Some(signals) = json_signals {
        if let Some(err) = classify_json_signals(signals, stderr) {
            return err;
        }
    }

    let detail = security::redact_sensitive_text(stderr);
    let lower = stderr.to_ascii_lowercase();

    // URL / playlist intake signals if yt-dlp emits one
    if lower.contains("url is a playlist")
        || lower.contains("_type='playlist'")
        || lower.contains("is not a valid url")
        || lower.contains("not in allowed list")
    {
        let reason = if lower.contains("playlist") {
            UrlInvalidReason::PlaylistOrChannelNotSupported
        } else {
            UrlInvalidReason::HostNotAllowed
        };
        return ProbeError::UrlInvalid {
            reason,
            message_key: reason.message_key().to_string(),
            message: detail,
        };
    }

    // Live / premiere stderr signals
    if lower.contains("premieres in") || lower.contains("has not premiered yet") {
        return ProbeError::LiveNotEnded {
            reason: LiveNotEndedReason::PremiereNotStarted,
            detail,
        };
    }
    if lower.contains("live stream has not ended")
        || lower.contains("this live event will begin")
        || lower.contains("live_status == 'is_live'")
        || lower.contains("live_status='is_live'")
    {
        return ProbeError::LiveNotEnded {
            reason: LiveNotEndedReason::OngoingLive,
            detail,
        };
    }

    // Anti-bot challenge (covers ASCII apostrophe, U+2019, and UTF-8 mojibake variants)
    if lower.contains("not a bot") && lower.contains("confirm you") {
        return ProbeError::BotCheck { detail };
    }

    // Unavailable categories (order matters: region_blocked contains "video unavailable")
    if let Some(reason) = infer_unavailable_reason_from_stderr(stderr) {
        return ProbeError::Unavailable { reason, detail };
    }

    // HTTP rate limiting / 403 / 429
    if lower.contains("http error 429")
        || lower.contains("too many requests")
        || lower.contains("http error 403")
        || lower.contains("403: forbidden")
    {
        return ProbeError::RateLimited { detail };
    }

    // Network offline / DNS / connect errors
    if lower.contains("unable to resolve")
        || lower.contains("getaddrinfo failed")
        || lower.contains("failed to establish a new connection")
        || lower.contains("network is unreachable")
        || lower.contains("name or service not known")
        || lower.contains("temporary failure in name resolution")
        || lower.contains("could not resolve host")
        || lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("no address associated with hostname")
    {
        return ProbeError::NetOffline { detail };
    }

    // Timeout markers in stderr
    if lower.contains("timed out") || lower.contains("timeout") {
        return ProbeError::NetTimeout { detail };
    }

    ProbeError::ExtractFailed { detail }
}

/// Persists raw probe JSON ONLY inside `cache/probe/<uuid>.json` and returns an opaque `ProbeHandle`.
pub fn persist_probe_json(
    ws_root: &Path,
    video_id: &str,
    canonical_url: &str,
    raw_bytes: &[u8],
) -> Result<ProbeHandle, ProbeError> {
    let probe_id = uuid::Uuid::new_v4().to_string();
    let rel_path = format!("cache/probe/{}.json", probe_id);
    let abs_path = security::resolve_workspace_path_with_root(ws_root, &rel_path).map_err(|e| {
        ProbeError::Internal {
            detail: format!("Failed to resolve temp probe path: {}", e),
        }
    })?;

    if let Some(parent) = abs_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ProbeError::Internal {
            detail: format!("Failed to create cache/probe dir: {}", e),
        })?;
    }

    std::fs::write(&abs_path, raw_bytes).map_err(|e| ProbeError::Internal {
        detail: format!("Failed to write temp probe JSON: {}", e),
    })?;

    Ok(ProbeHandle {
        probe_id,
        rel_path,
        abs_path,
        video_id: video_id.to_string(),
        canonical_url: canonical_url.to_string(),
    })
}

/// Deletes the temporary probe JSON file referenced by `handle` (called after download completes).
pub fn cleanup_probe_handle(handle: &ProbeHandle) -> std::io::Result<()> {
    if handle.abs_path.exists() {
        std::fs::remove_file(&handle.abs_path)?;
    }
    Ok(())
}

/// Prunes any probe JSON files in `cache/probe/` whose modification age >= `max_age` (1 hour at startup).
pub fn cleanup_stale_probe_files(ws_root: &Path, max_age: Duration) -> usize {
    let probe_dir = ws_root.join("cache").join("probe");
    let entries = match std::fs::read_dir(&probe_dir) {
        Ok(rd) => rd,
        Err(_) => return 0,
    };

    let now = SystemTime::now();
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if let Ok(age) = now.duration_since(modified) {
            if age >= max_age && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

fn extract_expire_timestamp_from_url(raw_url: &str) -> Option<u64> {
    let parsed = url::Url::parse(raw_url).ok()?;
    for (k, v) in parsed.query_pairs() {
        if k == "expire" {
            if let Ok(ts) = v.parse::<u64>() {
                return Some(ts);
            }
        }
    }
    // Googlevideo manifests also embed `/expire/<unix_secs>/` in path segments.
    let segs: Vec<&str> = parsed.path().split('/').collect();
    for pair in segs.windows(2) {
        if pair[0] == "expire" {
            if let Ok(ts) = pair[1].parse::<u64>() {
                return Some(ts);
            }
        }
    }
    None
}

/// Checks whether signed format URLs inside the stored probe JSON are missing or close to expiry.
pub fn are_probe_urls_expired(raw_bytes: &[u8], now_epoch_secs: u64) -> bool {
    let Ok(raw) = serde_json::from_slice::<YtDlpRawProbe>(raw_bytes) else {
        return true;
    };

    let mut urls: Vec<&str> = Vec::new();
    for f in &raw.requested_formats {
        if let Some(u) = f.url.as_deref().or(f.manifest_url.as_deref()) {
            urls.push(u);
        }
    }
    if urls.is_empty() {
        if let Some(u) = raw.url.as_deref().or(raw.manifest_url.as_deref()) {
            urls.push(u);
        }
    }

    if urls.is_empty() {
        return true;
    }

    let cutoff = now_epoch_secs.saturating_add(PROBE_URL_EXPIRY_SAFETY_MARGIN_SECS);
    let mut saw_any_expire = false;
    for u in urls {
        if let Some(exp) = extract_expire_timestamp_from_url(u) {
            saw_any_expire = true;
            if exp <= cutoff {
                return true;
            }
        }
    }

    // If URLs had no parseable expire param at all, assume fresh only when non-empty.
    let _ = saw_any_expire;
    false
}

fn current_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Reuses `handle` when its temp JSON still exists and contains unexpired URLs; otherwise
/// re-probes ONCE (e.g. after URL expiration or an HTTP 403 retry from M1.4) and replaces the
/// temp probe JSON cleanly.
pub async fn ensure_fresh_probe<F, Fut>(
    ws_root: &Path,
    handle: ProbeHandle,
    force_reprobe: bool,
    run_probe_once: F,
) -> Result<(ProbeInfo, ProbeHandle), ProbeError>
where
    F: FnOnce(ParsedYouTubeUrl) -> Fut,
    Fut: Future<Output = Result<(ProbeInfo, Vec<u8>), ProbeError>>,
{
    let now = current_epoch_secs();
    if !force_reprobe && handle.abs_path.is_file() {
        if let Ok(existing_bytes) = std::fs::read(&handle.abs_path) {
            if !are_probe_urls_expired(&existing_bytes, now) {
                if let Ok(info) =
                    parse_and_validate_probe_json(&existing_bytes, "", &handle.video_id)
                {
                    return Ok((info, handle));
                }
            }
        }
    }

    let parsed = url_intake::parse_youtube_url(&handle.canonical_url)?;
    let (fresh_info, fresh_bytes) = run_probe_once(parsed.clone()).await?;
    let _ = cleanup_probe_handle(&handle);
    let new_handle = persist_probe_json(
        ws_root,
        &parsed.video_id,
        &parsed.canonical_url,
        &fresh_bytes,
    )?;
    Ok((fresh_info, new_handle))
}

/// Executes a supervised `SidecarSpec` probe with 2 s heartbeat progress events (`<= 4/s`),
/// cancellation, 30 s timeout, and S1b/S1c classification. Separated for clean unit & fake-sidecar testing.
pub async fn execute_probe_spec_with_progress<F>(
    ws_root: &Path,
    spec: &SidecarSpec,
    parsed: &ParsedYouTubeUrl,
    cancel_token: Option<&CancellationToken>,
    mut on_progress: F,
) -> Result<ProbeOutcome, ProbeError>
where
    F: FnMut(ProbeProgressUpdate),
{
    let supervisor = SidecarSupervisor::new();
    let started = Instant::now();
    let mut last_emit = Instant::now() - PROBE_MIN_EMIT_INTERVAL;

    on_progress(ProbeProgressUpdate {
        operation: "generate".to_string(),
        stage: "probe".to_string(),
        indeterminate: true,
        elapsed_ms: 0.0,
        heartbeat: false,
    });

    let mut run_fut = std::pin::pin!(supervisor.run_to_completion(spec, cancel_token));
    let mut heartbeat_tick = tokio::time::interval(PROBE_HEARTBEAT_INTERVAL);
    heartbeat_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Consume immediate initial tick cleanly.
    heartbeat_tick.tick().await;

    let sidecar_res: Result<SidecarResult, SidecarError> = loop {
        tokio::select! {
            res = &mut run_fut => break res,
            _ = heartbeat_tick.tick() => {
                if last_emit.elapsed() >= PROBE_MIN_EMIT_INTERVAL {
                    last_emit = Instant::now();
                    on_progress(ProbeProgressUpdate {
                        operation: "generate".to_string(),
                        stage: "probe".to_string(),
                        indeterminate: true,
                        elapsed_ms: started.elapsed().as_millis() as f64,
                        heartbeat: true,
                    });
                }
            }
        }
    };

    if last_emit.elapsed() >= PROBE_MIN_EMIT_INTERVAL {
        on_progress(ProbeProgressUpdate {
            operation: "generate".to_string(),
            stage: "probe".to_string(),
            indeterminate: true,
            elapsed_ms: started.elapsed().as_millis() as f64,
            heartbeat: false,
        });
    }

    match sidecar_res {
        Ok(res) => {
            if res.truncated {
                return Err(ProbeError::ExtractFailed {
                    detail: format!(
                        "yt-dlp probe JSON exceeded {} MB stdout cap",
                        PROBE_STDOUT_CAP_BYTES / (1024 * 1024)
                    ),
                });
            }
            let info =
                parse_and_validate_probe_json(&res.stdout, &res.stderr_tail, &parsed.video_id)?;
            let raw_json_bytes = res.stdout.len();
            let handle = persist_probe_json(
                ws_root,
                &parsed.video_id,
                &parsed.canonical_url,
                &res.stdout,
            )?;
            Ok(ProbeOutcome {
                parsed_url: parsed.clone(),
                info,
                handle,
                raw_json_bytes,
            })
        }
        Err(SidecarError::Timeout(_)) => Err(ProbeError::NetTimeout {
            detail: format!(
                "yt-dlp probe timed out after {} seconds",
                PROBE_TIMEOUT_SECS
            ),
        }),
        Err(SidecarError::Cancelled) => Err(ProbeError::Cancelled),
        Err(SidecarError::ExecutionFailed { stderr_tail, .. }) => {
            Err(classify_yt_failure(None, &stderr_tail))
        }
        Err(SidecarError::BinaryNotFound(p)) => Err(ProbeError::ComponentMissing {
            detail: format!("Missing probe sidecar executable: {}", p.display()),
        }),
        Err(other) => Err(ProbeError::Internal {
            detail: format!("Sidecar execution error: {}", other),
        }),
    }
}

/// High-level entry point resolving `yt-dlp` and `qjs` via `ComponentResolver` and running the probe.
pub async fn probe_youtube_url<F>(
    ws_root: &Path,
    resolver: &ComponentResolver,
    raw_url: &str,
    cancel_token: Option<&CancellationToken>,
    on_progress: F,
) -> Result<ProbeOutcome, ProbeError>
where
    F: FnMut(ProbeProgressUpdate),
{
    let parsed = url_intake::parse_youtube_url(raw_url)?;

    let yt_dlp = resolver
        .resolve_executable("yt-dlp", "yt-dlp.exe")
        .map_err(|e| match e {
            ResolverError::Missing(advice) => ProbeError::ComponentMissing {
                detail: advice.message,
            },
            other => ProbeError::Internal {
                detail: other.to_string(),
            },
        })?;

    let qjs = resolver
        .resolve_executable("qjs", "qjs.exe")
        .map_err(|e| match e {
            ResolverError::Missing(advice) => ProbeError::ComponentMissing {
                detail: advice.message,
            },
            other => ProbeError::Internal {
                detail: other.to_string(),
            },
        })?;

    let spec = build_probe_spec(yt_dlp.path, qjs.path, ws_root, &parsed)?;
    execute_probe_spec_with_progress(ws_root, &spec, &parsed, cancel_token, on_progress).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tempfile::TempDir;

    fn fixtures_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/yt")
    }

    fn load_fixture(name: &str) -> String {
        let p = fixtures_dir().join(name);
        std::fs::read_to_string(&p)
            .unwrap_or_else(|e| panic!("Missing fixture {}: {}", p.display(), e))
    }

    #[test]
    fn test_s1c_golden_fixtures_classification() {
        struct FixtureCase {
            file: &'static str,
            expected_status_label: &'static str,
            expected_code: &'static str,
            expected_unavail: Option<YtUnavailableReason>,
            expected_live: Option<LiveNotEndedReason>,
        }

        let cases = [
            FixtureCase {
                file: "members_only.txt",
                expected_status_label: "[STATUS: VERIFIED]",
                expected_code: "E_YT_UNAVAILABLE",
                expected_unavail: Some(YtUnavailableReason::MembersOnly),
                expected_live: None,
            },
            FixtureCase {
                file: "age_restricted.txt",
                expected_status_label: "[STATUS: VERIFIED]",
                expected_code: "E_YT_UNAVAILABLE",
                expected_unavail: Some(YtUnavailableReason::AgeRestricted),
                expected_live: None,
            },
            FixtureCase {
                file: "region_blocked.txt",
                expected_status_label: "[STATUS: ASSUMED]",
                expected_code: "E_YT_UNAVAILABLE",
                expected_unavail: Some(YtUnavailableReason::RegionBlocked),
                expected_live: None,
            },
            FixtureCase {
                file: "private.txt",
                expected_status_label: "[STATUS: ASSUMED]",
                expected_code: "E_YT_UNAVAILABLE",
                expected_unavail: Some(YtUnavailableReason::Private),
                expected_live: None,
            },
            FixtureCase {
                file: "deleted.txt",
                expected_status_label: "[STATUS: ASSUMED]",
                expected_code: "E_YT_UNAVAILABLE",
                expected_unavail: Some(YtUnavailableReason::Deleted),
                expected_live: None,
            },
            FixtureCase {
                file: "ongoing_live.txt",
                expected_status_label: "[STATUS: VERIFIED]",
                expected_code: "E_YT_LIVE_NOT_ENDED",
                expected_unavail: None,
                expected_live: Some(LiveNotEndedReason::OngoingLive),
            },
            FixtureCase {
                file: "premiere_not_started.txt",
                expected_status_label: "[STATUS: ASSUMED]",
                expected_code: "E_YT_LIVE_NOT_ENDED",
                expected_unavail: None,
                expected_live: Some(LiveNotEndedReason::PremiereNotStarted),
            },
            FixtureCase {
                file: "bot_challenge.txt",
                expected_status_label: "[STATUS: ASSUMED]",
                expected_code: "E_YT_BOT_CHECK",
                expected_unavail: None,
                expected_live: None,
            },
            FixtureCase {
                file: "playlist_url.txt",
                expected_status_label: "[STATUS: VERIFIED]",
                expected_code: "E_YT_URL_INVALID",
                expected_unavail: None,
                expected_live: None,
            },
            FixtureCase {
                file: "invalid_url.txt",
                expected_status_label: "[STATUS: VERIFIED]",
                expected_code: "E_YT_URL_INVALID",
                expected_unavail: None,
                expected_live: None,
            },
        ];

        for case in cases {
            let content = load_fixture(case.file);
            assert!(
                content.contains(case.expected_status_label),
                "Fixture {} must preserve its status label {}",
                case.file,
                case.expected_status_label
            );

            let err = classify_yt_failure(None, &content);
            assert_eq!(err.code(), case.expected_code, "fixture {}", case.file);
            if let Some(reason) = case.expected_unavail {
                assert_eq!(
                    err.unavailable_reason(),
                    Some(reason),
                    "fixture {}",
                    case.file
                );
            }
            if let Some(live_reason) = case.expected_live {
                assert!(
                    matches!(err, ProbeError::LiveNotEnded { reason, .. } if reason == live_reason),
                    "fixture {} expected {:?}, got {:?}",
                    case.file,
                    live_reason,
                    err
                );
            }
        }
    }

    #[test]
    fn test_multisignal_json_and_stderr_classification_precedence() {
        // availability = needs_auth + members-only stderr -> MembersOnly
        let err_members = classify_yt_failure(
            Some(&ProbeJsonSignals {
                availability: Some("needs_auth".to_string()),
                ..Default::default()
            }),
            "ERROR: [youtube] ABC123def45: Join this channel to get access to members-only content",
        );
        assert_eq!(err_members.code(), "E_YT_UNAVAILABLE");
        assert_eq!(
            err_members.unavailable_reason(),
            Some(YtUnavailableReason::MembersOnly)
        );

        // availability = needs_auth without members clue -> AgeRestricted
        let err_age = classify_yt_failure(
            Some(&ProbeJsonSignals {
                availability: Some("needs_auth".to_string()),
                ..Default::default()
            }),
            "ERROR: Sign in to confirm your age",
        );
        assert_eq!(
            err_age.unavailable_reason(),
            Some(YtUnavailableReason::AgeRestricted)
        );

        // live_status == is_live overrides everything
        let err_live = classify_yt_failure(
            Some(&ProbeJsonSignals {
                live_status: Some("is_live".to_string()),
                availability: Some("public".to_string()),
                is_live: Some(true),
                ..Default::default()
            }),
            "",
        );
        assert!(matches!(
            err_live,
            ProbeError::LiveNotEnded {
                reason: LiveNotEndedReason::OngoingLive,
                ..
            }
        ));

        // live_status == post_live
        let err_post = classify_yt_failure(
            Some(&ProbeJsonSignals {
                live_status: Some("post_live".to_string()),
                ..Default::default()
            }),
            "",
        );
        assert!(matches!(
            err_post,
            ProbeError::LiveNotEnded {
                reason: LiveNotEndedReason::PostLiveProcessing,
                ..
            }
        ));

        // HTTP 429 & 403 -> RateLimited
        assert_eq!(
            classify_yt_failure(None, "ERROR: HTTP Error 429: Too Many Requests").code(),
            "E_YT_RATE_LIMITED"
        );
        assert_eq!(
            classify_yt_failure(None, "ERROR: HTTP Error 403: Forbidden").code(),
            "E_YT_RATE_LIMITED"
        );

        // DNS / connection -> NetOffline
        assert_eq!(
            classify_yt_failure(
                None,
                "ERROR: [youtube] Unable to resolve host: getaddrinfo failed"
            )
            .code(),
            "E_NET_OFFLINE"
        );

        // Timeout -> NetTimeout
        assert_eq!(
            classify_yt_failure(None, "ERROR: The read operation timed out").code(),
            "E_NET_TIMEOUT"
        );

        // Unknown / unable to extract -> ExtractFailed with redacted stderr
        let extract = classify_yt_failure(
            None,
            "ERROR: [youtube] dQw4w9WgXcQ: Unable to extract uploader id for C:\\Users\\Budi\\test",
        );
        assert_eq!(extract.code(), "E_YT_EXTRACT_FAILED");
        assert!(!extract.redacted_detail().contains("Budi"));
    }

    #[test]
    fn test_parse_probe_json_sanitizes_text_strips_urls_and_detects_preview_mode() {
        let raw_json = serde_json::json!({
            "id": "dQw4w9WgXcQ",
            "title": "  \u{202E}Evil\u{0007}   Podcast\n\nTitle \u{200B} ",
            "channel": "Channel \t Name \u{202E}",
            "duration": 612.4,
            "live_status": "not_live",
            "availability": "public",
            "playable_in_embed": true,
            "was_live": false,
            "is_live": false,
            "url": "https://rr1---sn.googlevideo.com/videoplayback?expire=1900000000&sig=SECRET",
            "requested_formats": [
                {
                    "vcodec": "avc1.640028",
                    "height": 1080,
                    "filesize": 122_400_000u64,
                    "url": "https://rr1---sn.googlevideo.com/videoplayback?expire=1900000000"
                },
                {
                    "vcodec": "none",
                    "acodec": "mp4a.40.2",
                    "filesize_approx": 6_120_000u64,
                    "url": "https://rr1---sn.googlevideo.com/audioplayback?expire=1900000000"
                }
            ]
        });
        let bytes = serde_json::to_vec(&raw_json).unwrap();
        let info = parse_and_validate_probe_json(&bytes, "", "dQw4w9WgXcQ").unwrap();

        assert_eq!(info.id, "dQw4w9WgXcQ");
        assert_eq!(info.title, "Evil Podcast Title");
        assert_eq!(info.channel, "Channel Name");
        assert_eq!(info.duration_seconds, 612);
        assert!(info.playable_in_embed);
        assert_eq!(info.preview_mode, PreviewMode::Embed);
        assert!(info.has_avc1_1080p);
        assert!(info.estimated_bytes_per_second.is_some());

        // Serialized ProbeInfo must never expose signed googlevideo URLs.
        let serialized_ui = serde_json::to_string(&info).unwrap();
        assert!(!serialized_ui.contains("googlevideo.com"));
        assert!(!serialized_ui.contains("SECRET"));

        // playable_in_embed = false -> TimecodeOnly
        let no_embed_json = serde_json::json!({
            "id": "dQw4w9WgXcQ",
            "title": &"X".repeat(250),
            "channel": "Creator",
            "duration": 120.0,
            "playable_in_embed": false
        });
        let info2 = parse_and_validate_probe_json(
            &serde_json::to_vec(&no_embed_json).unwrap(),
            "",
            "dQw4w9WgXcQ",
        )
        .unwrap();
        assert_eq!(info2.preview_mode, PreviewMode::TimecodeOnly);
        assert_eq!(info2.title.chars().count(), PROBE_MAX_TITLE_CHARS);
    }

    #[test]
    fn test_estimate_download_seconds_helper() {
        let cfg = DownloadEtaConfig::default();
        assert!((cfg.rt_factor - 1.66).abs() < 1e-9);
        assert!((cfg.startup_seconds - 10.0).abs() < 1e-9);

        // 166s range at 1.66x RT + 10s startup = 110s
        assert!((estimate_download_seconds(166.0, &cfg) - 110.0).abs() < 1e-6);
        assert!((estimate_download_seconds(0.0, &cfg) - 10.0).abs() < 1e-6);
        assert!((estimate_download_seconds(-5.0, &cfg) - 10.0).abs() < 1e-6);
        assert!((estimate_download_seconds(f64::NAN, &cfg) - 10.0).abs() < 1e-6);
    }

    #[test]
    fn test_probe_handle_lifecycle_and_stale_gc() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();

        let sample_bytes = serde_json::to_vec(&serde_json::json!({
            "id": "dQw4w9WgXcQ",
            "title": "Sample Video",
            "duration": 60.0,
            "url": "https://rr1---sn.googlevideo.com/videoplayback?expire=4102444800"
        }))
        .unwrap();

        let handle = persist_probe_json(
            ws,
            "dQw4w9WgXcQ",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            &sample_bytes,
        )
        .unwrap();

        assert!(handle.rel_path().starts_with("cache/probe/"));
        assert!(handle.abs_path().is_file());
        assert!(!are_probe_urls_expired(&sample_bytes, 1_700_000_000));

        // Expired timestamp detected inside safety window
        let expired_bytes = serde_json::to_vec(&serde_json::json!({
            "id": "dQw4w9WgXcQ",
            "url": "https://rr1---sn.googlevideo.com/videoplayback?expire=1700000030"
        }))
        .unwrap();
        assert!(are_probe_urls_expired(&expired_bytes, 1_700_000_000));

        // Direct cleanup removes handle file
        cleanup_probe_handle(&handle).unwrap();
        assert!(!handle.abs_path().exists());

        // Stale GC with 0-sec threshold cleans files inside cache/probe
        let stale = persist_probe_json(
            ws,
            "dQw4w9WgXcQ",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            &sample_bytes,
        )
        .unwrap();
        assert!(stale.abs_path().is_file());
        let removed = cleanup_stale_probe_files(ws, Duration::from_secs(0));
        assert_eq!(removed, 1);
        assert!(!stale.abs_path().exists());
    }

    #[tokio::test]
    async fn test_ensure_fresh_probe_reprobes_once_when_expired() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let expired_bytes = serde_json::to_vec(&serde_json::json!({
            "id": "dQw4w9WgXcQ",
            "title": "Old Title",
            "duration": 30.0,
            "url": "https://rr1---sn.googlevideo.com/videoplayback?expire=100"
        }))
        .unwrap();

        let old_handle = persist_probe_json(
            ws,
            "dQw4w9WgXcQ",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            &expired_bytes,
        )
        .unwrap();
        let old_path = old_handle.abs_path().to_path_buf();

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_ref = calls.clone();

        let (fresh_info, new_handle) = ensure_fresh_probe(ws, old_handle, false, move |_| {
            let c = calls_ref.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                let fresh_json = serde_json::to_vec(&serde_json::json!({
                    "id": "dQw4w9WgXcQ",
                    "title": "Refreshed Title",
                    "duration": 30.0,
                    "playable_in_embed": true,
                    "url": "https://rr1---sn.googlevideo.com/videoplayback?expire=4102444800"
                }))
                .unwrap();
                let info = parse_and_validate_probe_json(&fresh_json, "", "dQw4w9WgXcQ")?;
                Ok((info, fresh_json))
            }
        })
        .await
        .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(fresh_info.title, "Refreshed Title");
        assert!(!old_path.exists());
        assert!(new_handle.abs_path().is_file());

        // Second call now sees valid unexpired handle and does NOT reprobe
        let calls_ref2 = calls.clone();
        let (cached_info, _) = ensure_fresh_probe(ws, new_handle, false, move |_| {
            let c = calls_ref2.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err(ProbeError::Internal {
                    detail: "Should not have re-probed".to_string(),
                })
            }
        })
        .await
        .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(cached_info.title, "Refreshed Title");
    }

    #[test]
    fn test_build_probe_spec_flags_and_canonical_url() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let parsed = url_intake::parse_youtube_url("https://youtu.be/dQw4w9WgXcQ?t=45").unwrap();
        let yt_dlp_bin = ws.join("components").join("yt-dlp").join("yt-dlp.exe");
        let qjs_bin = ws.join("components").join("qjs").join("qjs.exe");

        let spec = build_probe_spec(yt_dlp_bin, qjs_bin.clone(), ws, &parsed).unwrap();
        assert_eq!(spec.timeout, Some(Duration::from_secs(PROBE_TIMEOUT_SECS)));
        assert_eq!(spec.max_total_bytes, PROBE_STDOUT_CAP_BYTES);

        assert!(spec.args.contains(&"-J".to_string()));
        assert!(spec.args.contains(&"--no-playlist".to_string()));
        assert!(spec.args.contains(&"--no-warnings".to_string()));
        assert!(spec.args.contains(&"--ignore-config".to_string()));
        assert!(spec
            .args
            .contains(&format!("quickjs:{}", qjs_bin.to_string_lossy())));
        assert!(spec.args.contains(&"--".to_string()));
        assert_eq!(
            spec.args.last().map(String::as_str),
            Some("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
        );
    }

    #[tokio::test]
    async fn test_probe_supervisor_timeout_maps_to_net_timeout() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let fake_bin = crate::sidecars::test_fake_sidecar_bin();
        let sidecar_env = SidecarEnv::for_component("yt-dlp", ws);
        let parsed =
            url_intake::parse_youtube_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ").unwrap();

        let spec = SidecarSpec::new(fake_bin, ["--mode", "sleep", "5000"])
            .with_component_env(&sidecar_env)
            .unwrap()
            .timeout(Duration::from_millis(150));

        let mut progress_events = Vec::new();
        let err = execute_probe_spec_with_progress(ws, &spec, &parsed, None, |u| {
            progress_events.push(u);
        })
        .await
        .expect_err("expected timeout");

        assert_eq!(err.code(), "E_NET_TIMEOUT");
        assert!(!progress_events.is_empty());
        assert_eq!(progress_events[0].operation, "generate");
        assert_eq!(progress_events[0].stage, "probe");
        assert!(progress_events[0].indeterminate);
    }

    #[tokio::test]
    async fn test_probe_supervisor_cancellation() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let fake_bin = crate::sidecars::test_fake_sidecar_bin();
        let sidecar_env = SidecarEnv::for_component("yt-dlp", ws);
        let parsed =
            url_intake::parse_youtube_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ").unwrap();

        let spec = SidecarSpec::new(fake_bin, ["--mode", "sleep", "5000"])
            .with_component_env(&sidecar_env)
            .unwrap()
            .timeout(Duration::from_secs(10));

        let token = CancellationToken::new();
        let cancel_trigger = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel_trigger.cancel();
        });

        let err = execute_probe_spec_with_progress(ws, &spec, &parsed, Some(&token), |_| {})
            .await
            .expect_err("expected cancellation");

        assert!(matches!(err, ProbeError::Cancelled));
    }

    /// Local-only live test using the pinned dev-fixtures `yt-dlp.exe` and `qjs.exe`.
    /// Requires `CLIPPER_LIVE_YT_TEST=1`; ignored in CI because cloud runners hit YouTube bot checks.
    #[tokio::test]
    #[ignore = "Local-only live YouTube probe (run with CLIPPER_LIVE_YT_TEST=1)"]
    async fn test_local_live_yt_dlp_probe() {
        if std::env::var("CLIPPER_LIVE_YT_TEST").as_deref() != Ok("1") {
            return;
        }
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let yt_dlp =
            repo_root.join("tools/dev-fixtures/real/components/yt-dlp/2026.10.26/yt-dlp.exe");
        let qjs = repo_root.join("tools/dev-fixtures/real/components/qjs/v0.18.0/qjs.exe");
        let parsed =
            url_intake::parse_youtube_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ").unwrap();
        let spec = build_probe_spec(yt_dlp, qjs, ws, &parsed).unwrap();
        let mut updates = Vec::new();
        let out = execute_probe_spec_with_progress(ws, &spec, &parsed, None, |u| updates.push(u))
            .await
            .expect("live probe should succeed locally");
        assert_eq!(out.info.id, "dQw4w9WgXcQ");
        assert!(!out.info.title.is_empty());
        assert!(out.handle.abs_path().is_file());
        cleanup_probe_handle(&out.handle).unwrap();
    }
}
