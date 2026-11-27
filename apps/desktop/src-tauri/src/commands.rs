//! Thin Tauri commands layer (ARCHITECTURE §4; M1.1d-lite Parts 1–4).
//!
//! Exposes production typed Tauri IPC commands to the frontend:
//! - App & Workspace status + DataRoot relocation (`bootstrap.json`)
//! - User-facing Component Manager (manifest fetch, pre-install disk check, install-all with
//!   per-item + overall progress/ETA, repair, retry quarantine, verify)
//! - Passive Hardware / Speech / Encoder / Language settings
//! - Diagnostics store, crash recovery, WebView JS error capture (rate-limited + capped),
//!   redacted report generator, `.txt` export, and narrow logs folder opener.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::Manager;
use tauri_specta::Event;
use tokio_util::sync::CancellationToken;

use crate::capabilities::CapabilityReportView;
use crate::crash::{self, CrashRecoveryStatus};
use crate::diagnostics::{
    self, DiagnosticsReportBundle, DiagnosticsReportOptions, REQUIRED_DISK_MIN_BUFFER_BYTES,
};
use crate::download::{
    compute_download_cache_key, compute_download_window, ProbeHandleCache,
    DEFAULT_RANGE_HANDLE_SECONDS,
};
use crate::encoder::{self, EncoderStatusSnapshot};
use crate::eta::EtaEstimator;
use crate::hardware::{self, HardwareProfile};
use crate::installer::{self, InUseRegistry, VerifyRepairOutcome};
use crate::jobs::{
    compute_generate_idempotency_key, JobCoordinator, JobEventPayload, StartJobDisposition,
};
use crate::manifest::{self, ComponentManifest};
use crate::pipeline::{execute_generate_job, GenerateRunParams};
use crate::probe::{self, ProbeError, ProbeInfo, YT_FORMAT_SELECTOR};
use crate::resolver::{ComponentDiagnosticsSnapshot, ComponentResolver};
use crate::security::{self, DataRoot};
use crate::speech::{self, SpeechModel, SpeechModelState};
use crate::store::{
    ArtifactRepo, ErrorEventRow, JobRecord, JobRepo, LocalStore, NewErrorEvent,
    PreviewCountersSnapshot, ProjectRecord, ProjectRepo,
};
use crate::WorkspaceState;

pub const DEFAULT_PRODUCTION_MANIFEST_URL: &str =
    "https://cdn.clipperhelper.com/manifests/windows-x86_64.json";

/// Managed state caching the latest verified component manifest in production commands.
#[derive(Default, Clone)]
pub struct ManifestCacheState(pub Arc<Mutex<Option<ComponentManifest>>>);

/// Managed state for server-side rate limiting of WebView JS error capture.
#[derive(Default, Clone)]
pub struct WebViewErrorRateLimiter(pub Arc<Mutex<VecDeque<Instant>>>);

/// Managed state tracking in-flight YouTube probe cancellation tokens by `request_id`.
#[derive(Default, Clone)]
pub struct ProbeCancelRegistry(pub Arc<Mutex<HashMap<String, CancellationToken>>>);

/// Typed progress event emitted for `generate` / `probe` operations (`<= 4/s`, 2 s heartbeat).
#[derive(Serialize, Deserialize, Type, Event, Debug, Clone)]
pub struct ProbeProgressEvent {
    pub request_id: String,
    pub operation: String,
    pub stage: String,
    pub indeterminate: bool,
    pub elapsed_ms: f64,
    pub heartbeat: bool,
}

/// Safe metadata + persisted project record returned to the UI upon successful YouTube probe.
#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct ProbeProjectResponse {
    pub probe_info: ProbeInfo,
    pub project: ProjectRecord,
    pub suggested_start_seconds: Option<u32>,
}

/// Preview bridge configuration and range limits exposed to the frontend.
#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct PreviewConfigResponse {
    pub bridge_url: String,
    pub bridge_protocol_version: u32,
    pub control_video_id: String,
    pub ready_timeout_ms: u32,
    pub max_range_seconds: u32,
    pub download_prior_rt_factor: f64,
    pub download_prior_startup_seconds: f64,
    pub can_force_timecode_only: bool,
}

/// Structured preview diagnostics & counter increment payload recorded cleanly without PII/URLs.
#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct PreviewEventInput {
    pub mode: String,
    pub bridge_ready_ms: Option<u32>,
    pub fallback_reason: Option<String>,
    pub yt_error_code: Option<i32>,
    pub counter_kind: Option<String>,
    pub record_error_code: Option<String>,
}

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct DiskSpaceBounds {
    pub available_bytes: f64,
    pub required_bytes: f64,
}

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct AppCommandError {
    pub code: String,
    pub message_key: String,
    pub message: String,
    pub detail: String,
    pub disk_bounds: Option<Box<DiskSpaceBounds>>,
}

impl AppCommandError {
    fn simple(code: &str, message: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message_key: error_key_for_code(code),
            message: message.into(),
            detail: detail.into(),
            disk_bounds: None,
        }
    }

    fn with_message_key(
        code: &str,
        message_key: impl Into<String>,
        message: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            code: code.to_string(),
            message_key: message_key.into(),
            message: message.into(),
            detail: detail.into(),
            disk_bounds: None,
        }
    }

    fn disk_full(message: String, available_bytes: u64, required_bytes: u64) -> Self {
        Self {
            code: "E_DISK_FULL".to_string(),
            message_key: error_key_for_code("E_DISK_FULL"),
            detail: message.clone(),
            message,
            disk_bounds: Some(Box::new(DiskSpaceBounds {
                available_bytes: available_bytes as f64,
                required_bytes: required_bytes as f64,
            })),
        }
    }
}

/// Application metadata returned to the frontend.
#[derive(Serialize, Deserialize, Type, Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub platform: String,
}

/// Workspace initialization status returned to the frontend.
#[derive(Serialize, Deserialize, Type, Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceStatus {
    pub is_initialized: bool,
    pub root: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub configured_root_unreachable: bool,
    pub unreachable_root_redacted: Option<String>,
}

/// Thread-safe holder for mutable workspace initialization status (updated when recovering DataRoot).
#[derive(Debug)]
pub struct WorkspaceStatusState(pub Mutex<WorkspaceStatus>);

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct ManifestUrlConfig {
    pub url: String,
    pub sig_url: String,
    pub can_override_in_ui: bool,
}

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct ManifestComponentItem {
    pub component: String,
    pub version: String,
    pub archive: String,
    pub size_bytes: f64,
    pub is_model: bool,
    pub is_current_installed: bool,
}

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct ManifestOverview {
    pub sequence: f64,
    pub channel: String,
    pub generated_at: String,
    pub components: Vec<ManifestComponentItem>,
}

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct DiskSpaceStatus {
    pub available_bytes: f64,
    pub required_bytes: f64,
    pub sufficient: bool,
    pub data_root_redacted: String,
    pub is_default_data_root: bool,
    pub is_onedrive: bool,
}

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct ComponentsOverview {
    pub diagnostics: ComponentDiagnosticsSnapshot,
    pub required_components: Vec<String>,
    pub manifest: Option<ManifestOverview>,
    pub manifest_url_config: ManifestUrlConfig,
    pub disk_space: DiskSpaceStatus,
    pub speech_model: SpeechModelState,
}

#[derive(Serialize, Deserialize, Type, tauri_specta::Event, Debug, Clone)]
pub struct ComponentInstallProgressEvent {
    pub job_id: String,
    pub component: String,
    pub stage: String,
    pub completed_items: u32,
    pub total_items: u32,
    pub item_completed_bytes: f64,
    pub item_total_bytes: f64,
    pub item_percent: f64,
    pub rate_bps: f64,
    pub item_eta_seconds: Option<f64>,
    pub overall_percent: f64,
    pub overall_eta_seconds: Option<f64>,
    pub overall_eta_low_seconds: Option<f64>,
    pub overall_eta_high_seconds: Option<f64>,
}

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct ComponentActionOutcome {
    pub ok: bool,
    pub component: String,
    pub version: String,
    pub already_installed: bool,
    pub quarantined: bool,
    pub rolled_back_to: Option<String>,
    pub error_code: Option<String>,
    pub message: String,
    pub capability_report: Option<CapabilityReportView>,
}

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct BatchInstallOutcome {
    pub installed_count: u32,
    pub skipped_count: u32,
    pub outcomes: Vec<ComponentActionOutcome>,
}

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct DataRootChangeOutcome {
    pub new_root_redacted: String,
    pub is_onedrive: bool,
    pub requires_restart: bool,
}

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct SettingsOverview {
    pub language: String,
    pub speech_state: SpeechModelState,
    pub encoder_status: EncoderStatusSnapshot,
    pub hardware: HardwareProfile,
    pub data_root_redacted: String,
    pub is_default_data_root: bool,
    pub is_onedrive: bool,
}

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct SettingsPatch {
    pub language: Option<String>,
    /// When `Some("auto")` resets user override to automatic RAM default;
    /// otherwise selects an explicit model (`tiny`/`base`/`small-q5_1`/`small`/`medium`).
    pub speech_model_selection: Option<String>,
    pub dismiss_speech_downgrade: Option<bool>,
}

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct SaveReportOutcome {
    pub saved_path_redacted: String,
    pub bytes_written: u32,
    pub truncated: bool,
}

fn deps_from_app(
    app: &tauri::AppHandle,
) -> Result<(PathBuf, LocalStore, InUseRegistry), AppCommandError> {
    let ws = app.try_state::<WorkspaceState>().ok_or_else(|| {
        AppCommandError::simple(
            "E_UNKNOWN",
            "Workspace is not initialized",
            "WorkspaceState missing",
        )
    })?;
    let store = app.try_state::<LocalStore>().ok_or_else(|| {
        AppCommandError::simple(
            "E_UNKNOWN",
            "LocalStore is not initialized",
            "LocalStore missing",
        )
    })?;
    let in_use = app
        .try_state::<InUseRegistry>()
        .map(|s| (*s).clone())
        .unwrap_or_default();
    Ok((ws.root.clone(), (*store).clone(), in_use))
}

fn error_key_for_code(code: &str) -> String {
    format!("err.{}.body", code)
}

fn record_error(store: &LocalStore, code: &str, stage: &str, detail: &str, stderr_tail: &str) {
    record_error_with_key(
        store,
        code,
        &error_key_for_code(code),
        stage,
        detail,
        stderr_tail,
    );
}

fn record_error_with_key(
    store: &LocalStore,
    code: &str,
    message_key: &str,
    stage: &str,
    detail: &str,
    stderr_tail: &str,
) {
    let _ = store.record_error_event(NewErrorEvent {
        code: code.to_string(),
        stage: stage.to_string(),
        job_id: None,
        message_key: message_key.to_string(),
        detail: detail.to_string(),
        stderr_tail: stderr_tail.to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        component_versions_json: "{}".to_string(),
    });
}

fn resolve_manifest_urls(override_url: Option<&str>) -> ManifestUrlConfig {
    #[cfg(debug_assertions)]
    let url = {
        // In debug builds, default to the local dev fixture server's real manifest.
        // Override with CLIPPER_MANIFEST_URL env var or the UI text field.
        let mut resolved = "http://127.0.0.1:8080/real/manifest.json".to_string();
        if let Ok(env_url) = std::env::var("CLIPPER_MANIFEST_URL") {
            let trimmed = env_url.trim();
            if !trimmed.is_empty() {
                resolved = trimmed.to_string();
            }
        }
        if let Some(ov) = override_url.map(str::trim).filter(|s| !s.is_empty()) {
            resolved = ov.to_string();
        }
        resolved
    };
    #[cfg(not(debug_assertions))]
    let url = {
        let _ = override_url;
        DEFAULT_PRODUCTION_MANIFEST_URL.to_string()
    };

    let sig_url = if url.ends_with(".sig") {
        url.clone()
    } else {
        format!("{}.sig", url)
    };

    ManifestUrlConfig {
        url,
        sig_url,
        can_override_in_ui: cfg!(debug_assertions),
    }
}

fn required_component_list(speech_state: &SpeechModelState) -> Vec<String> {
    vec![
        "yt-dlp".to_string(),
        "qjs".to_string(),
        "ffmpeg".to_string(),
        "whisper-cli".to_string(),
        "ggml-silero-v6.2.0.bin".to_string(),
        speech_state.active_model.manifest_component().to_string(),
    ]
}

/// Computes `sum(component sizes) * 1.5 + 1 GB` per M1.1d-lite Part 2 pre-install space guard.
pub fn compute_preinstall_required_bytes(total_payload_bytes: u64) -> u64 {
    let scaled = total_payload_bytes.saturating_mul(3).saturating_div(2);
    scaled.saturating_add(REQUIRED_DISK_MIN_BUFFER_BYTES)
}

fn compute_disk_status(
    ws_root: &Path,
    manifest_opt: Option<&ComponentManifest>,
    store: &LocalStore,
    required_names: &[String],
) -> DiskSpaceStatus {
    let pending_bytes: u64 = if let Some(m) = manifest_opt {
        m.components
            .iter()
            .filter(|c| required_names.contains(&c.component))
            .filter(|c| {
                !store
                    .get_component_version(&c.component, &c.version)
                    .ok()
                    .flatten()
                    .filter(|r| r.status == "installed" && r.is_current)
                    .is_some_and(|r| installer::quick_verify_row(ws_root, &r).is_ok())
            })
            .map(|c| c.size_bytes)
            .sum()
    } else {
        0
    };

    let required_bytes = if pending_bytes > 0 || manifest_opt.is_some() {
        compute_preinstall_required_bytes(pending_bytes)
    } else {
        REQUIRED_DISK_MIN_BUFFER_BYTES
    };
    let available_bytes = security::get_free_disk_space(ws_root).unwrap_or(0);
    let data_root_info = DataRoot::resolve_active().ok();
    let is_default_data_root = data_root_info.as_ref().is_none_or(|d| d.is_default);
    let is_onedrive = data_root_info.as_ref().is_some_and(|d| d.is_onedrive);

    DiskSpaceStatus {
        available_bytes: available_bytes as f64,
        required_bytes: required_bytes as f64,
        sufficient: available_bytes >= required_bytes,
        data_root_redacted: security::redact_sensitive_text(&ws_root.to_string_lossy()),
        is_default_data_root,
        is_onedrive,
    }
}

fn manifest_overview_from_manifest(
    ws_root: &Path,
    store: &LocalStore,
    m: &ComponentManifest,
) -> ManifestOverview {
    let components = m
        .components
        .iter()
        .map(|c| {
            let is_current_installed = store
                .get_component_version(&c.component, &c.version)
                .ok()
                .flatten()
                .filter(|r| r.status == "installed" && r.is_current)
                .is_some_and(|r| installer::quick_verify_row(ws_root, &r).is_ok());
            ManifestComponentItem {
                component: c.component.clone(),
                version: c.version.clone(),
                archive: c.archive.clone(),
                size_bytes: c.size_bytes as f64,
                is_model: c.component.ends_with(".bin"),
                is_current_installed,
            }
        })
        .collect();

    ManifestOverview {
        sequence: m.sequence as f64,
        channel: m.channel.clone(),
        generated_at: m.generated_at.clone(),
        components,
    }
}

/// Returns application metadata and current runtime platform.
#[tauri::command]
#[specta::specta]
pub fn get_app_info() -> AppInfo {
    tracing::info!("get_app_info invoked");
    AppInfo {
        name: "ClipperHelper".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        platform: std::env::consts::OS.to_string(),
    }
}

/// Returns the workspace initialization status.
#[tauri::command]
#[specta::specta]
pub fn get_workspace_status(app: tauri::AppHandle) -> WorkspaceStatus {
    if let Some(status_state) = app.try_state::<WorkspaceStatusState>() {
        if let Ok(guard) = status_state.0.lock() {
            return guard.clone();
        }
    }
    WorkspaceStatus {
        is_initialized: false,
        root: None,
        error_code: Some("E_APP_INIT_FAILED".to_string()),
        error_message: Some("Workspace status uninitialized".to_string()),
        configured_root_unreachable: false,
        unreachable_root_redacted: None,
    }
}

/// Clears a missing/unreachable `bootstrap.json` DataRoot override (never deleting user data)
/// and initializes the workspace at the default local folder.
#[tauri::command]
#[specta::specta]
pub fn use_default_data_root(app: tauri::AppHandle) -> Result<WorkspaceStatus, AppCommandError> {
    security::DataRoot::reset_bootstrap_to_default().map_err(|e| {
        AppCommandError::simple(
            e.error_code(),
            e.to_string(),
            security::redact_sensitive_text(&e.to_string()),
        )
    })?;

    let status = crate::initialize_workspace_into_app(&app).map_err(|e| {
        AppCommandError::simple(
            e.error_code(),
            e.to_string(),
            security::redact_sensitive_text(&e.to_string()),
        )
    })?;
    Ok(status)
}

/// Returns current component statuses, manifest cache, disk check, and speech-model target.
#[tauri::command]
#[specta::specta]
pub fn get_components_overview(
    app: tauri::AppHandle,
    override_manifest_url: Option<String>,
) -> Result<ComponentsOverview, AppCommandError> {
    let (ws_root, store, in_use) = deps_from_app(&app)?;
    let resolver = ComponentResolver::new(ws_root.clone(), store.clone(), in_use);
    let diagnostics = resolver.diagnostics_snapshot().map_err(|e| {
        let code = e.error_code();
        let redacted = security::redact_sensitive_text(&e.to_string());
        record_error(&store, code, "components_overview", &redacted, "");
        AppCommandError::simple(code, e.to_string(), redacted)
    })?;

    let hw = hardware::probe_hardware_profile();
    let speech_model = speech::resolve_speech_model_state(&store, hw.ram_bytes_u64())
        .map_err(|e| AppCommandError::simple("E_UNKNOWN", e.to_string(), e.to_string()))?;
    let required = required_component_list(&speech_model);

    let cached_manifest = app
        .try_state::<ManifestCacheState>()
        .and_then(|c| c.0.lock().ok().and_then(|g| g.clone()));

    let disk_space = compute_disk_status(&ws_root, cached_manifest.as_ref(), &store, &required);
    let manifest_overview = cached_manifest
        .as_ref()
        .map(|m| manifest_overview_from_manifest(&ws_root, &store, m));

    Ok(ComponentsOverview {
        diagnostics,
        required_components: required,
        manifest: manifest_overview,
        manifest_url_config: resolve_manifest_urls(override_manifest_url.as_deref()),
        disk_space,
        speech_model,
    })
}

/// Fetches and cryptographically verifies the component manifest from config URL (debug override allowed).
#[tauri::command]
#[specta::specta]
pub async fn fetch_components_manifest(
    app: tauri::AppHandle,
    override_url: Option<String>,
) -> Result<ManifestOverview, AppCommandError> {
    let (ws_root, store, _) = deps_from_app(&app)?;
    let cfg = resolve_manifest_urls(override_url.as_deref());

    #[cfg(debug_assertions)]
    let extra_keys_owned: Option<Vec<manifest::ManifestPublicKey>> = {
        let mut keys = vec![manifest::ManifestPublicKey {
            kid: "clipper-manifest-dev",
            public_key_hex: "aaaf48f0cbd01c1f79573f139a5b0401b488ac44e0b103d64dea533b075c997b",
        }];
        let pub_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../.dev-keys/clipper-manifest-dev.pub.hex");
        if let Ok(raw_hex) = std::fs::read_to_string(pub_path) {
            let trimmed = raw_hex.trim().to_string();
            if trimmed.len() == 64 && !keys.iter().any(|k| k.public_key_hex == trimmed) {
                let leaked: &'static str = Box::leak(trimmed.into_boxed_str());
                keys.push(manifest::ManifestPublicKey {
                    kid: "clipper-manifest-dev",
                    public_key_hex: leaked,
                });
            }
        }
        Some(keys)
    };
    #[cfg(not(debug_assertions))]
    let extra_keys_owned: Option<Vec<manifest::ManifestPublicKey>> = None;

    let verified = crate::updater::fetch_and_verify_manifest(
        &cfg.url,
        &cfg.sig_url,
        env!("CARGO_PKG_VERSION"),
        &store,
        extra_keys_owned.as_deref(),
    )
    .await
    .map_err(|e| {
        let code = e.error_code();
        let msg_key = e.message_key();
        let redacted = security::redact_sensitive_text(&e.to_string());
        record_error_with_key(&store, code, msg_key, "manifest_fetch", &redacted, "");
        AppCommandError::with_message_key(code, msg_key, redacted.clone(), redacted)
    })?;

    if let Some(cache) = app.try_state::<ManifestCacheState>() {
        if let Ok(mut guard) = cache.0.lock() {
            *guard = Some(verified.clone());
        }
    }

    Ok(manifest_overview_from_manifest(&ws_root, &store, &verified))
}

fn get_cached_manifest_entry(
    app: &tauri::AppHandle,
    store: &LocalStore,
    component: &str,
    version: Option<&str>,
) -> Result<(u64, manifest::ComponentManifestEntry), AppCommandError> {
    let cache = app
        .try_state::<ManifestCacheState>()
        .and_then(|c| c.0.lock().ok().and_then(|g| g.clone()))
        .ok_or_else(|| {
            let msg = "Manifest belum dimuat. Klik Coba Lagi untuk mengambil manifest.".to_string();
            record_error(store, "E_NET_OFFLINE", "manifest_cache", &msg, "");
            AppCommandError::simple("E_NET_OFFLINE", msg.clone(), msg)
        })?;

    let entry = cache
        .components
        .iter()
        .find(|c| c.component == component && version.map(|v| v == c.version).unwrap_or(true))
        .cloned()
        .ok_or_else(|| {
            let ver_display = version
                .map(manifest::format_component_version)
                .unwrap_or_else(|| "(latest)".to_string());
            let msg = format!(
                "Komponen '{}' versi {} tidak tersedia di manifest.",
                component, ver_display
            );
            record_error(store, "E_COMPONENT_MISSING", "manifest_lookup", &msg, "");
            AppCommandError::simple("E_COMPONENT_MISSING", msg.clone(), msg)
        })?;

    Ok((cache.sequence, entry))
}

struct InstallBatchCursor<'a> {
    job_id: &'a str,
    manifest_sequence: u64,
    completed_items: u32,
    total_items: u32,
    prior_bytes_done: u64,
    batch_total_bytes: u64,
}

async fn install_single_entry(
    app: &tauri::AppHandle,
    ws_root: &Path,
    store: &LocalStore,
    in_use: &InUseRegistry,
    entry: &manifest::ComponentManifestEntry,
    cursor: InstallBatchCursor<'_>,
) -> Result<ComponentActionOutcome, AppCommandError> {
    let entry_size = entry.size_bytes.max(1);
    let app_handle = app.clone();
    let job_owned = cursor.job_id.to_string();
    let comp_name = entry.component.clone();

    let res = installer::download_and_install_component(
        ws_root,
        store,
        in_use,
        entry,
        cursor.manifest_sequence,
        move |p| {
            let overall_done = cursor
                .prior_bytes_done
                .saturating_add(p.completed_bytes.min(entry_size));
            let overall_pct = if cursor.batch_total_bytes > 0 {
                ((overall_done as f64 / cursor.batch_total_bytes as f64) * 100.0).clamp(0.0, 100.0)
            } else {
                p.percent
            };
            let overall_eta = if p.rate_bps > 1.0 && cursor.batch_total_bytes > overall_done {
                Some((cursor.batch_total_bytes - overall_done) as f64 / p.rate_bps)
            } else {
                p.eta_seconds
            };
            let _ = ComponentInstallProgressEvent {
                job_id: job_owned.clone(),
                component: comp_name.clone(),
                stage: p.stage,
                completed_items: cursor.completed_items,
                total_items: cursor.total_items,
                item_completed_bytes: p.completed_bytes as f64,
                item_total_bytes: p.total_bytes as f64,
                item_percent: p.percent,
                rate_bps: p.rate_bps,
                item_eta_seconds: p.eta_seconds,
                overall_percent: overall_pct,
                overall_eta_seconds: overall_eta,
                overall_eta_low_seconds: overall_eta.map(|e| e * 0.75),
                overall_eta_high_seconds: overall_eta.map(|e| e * 1.5),
            }
            .emit(&app_handle);
        },
        None,
    )
    .await;

    match res {
        Ok(outcome) => {
            let _ = ComponentInstallProgressEvent {
                job_id: cursor.job_id.to_string(),
                component: outcome.component.clone(),
                stage: "complete".to_string(),
                completed_items: cursor.completed_items.saturating_add(1),
                total_items: cursor.total_items,
                item_completed_bytes: entry.size_bytes as f64,
                item_total_bytes: entry.size_bytes as f64,
                item_percent: 100.0,
                rate_bps: 0.0,
                item_eta_seconds: Some(0.0),
                overall_percent: if cursor.batch_total_bytes > 0 {
                    (((cursor.prior_bytes_done.saturating_add(entry_size)) as f64
                        / cursor.batch_total_bytes as f64)
                        * 100.0)
                        .clamp(0.0, 100.0)
                } else {
                    100.0
                },
                overall_eta_seconds: Some(0.0),
                overall_eta_low_seconds: Some(0.0),
                overall_eta_high_seconds: Some(0.0),
            }
            .emit(app);
            Ok(ComponentActionOutcome {
                ok: true,
                component: outcome.component,
                version: outcome.version,
                already_installed: outcome.already_installed,
                quarantined: false,
                rolled_back_to: None,
                error_code: None,
                message: if outcome.kind == "model" {
                    "Hash model terverifikasi".to_string()
                } else {
                    "Terpasang dan terverifikasi".to_string()
                },
                capability_report: outcome
                    .capability_report
                    .as_ref()
                    .map(CapabilityReportView::from),
            })
        }
        Err(installer::InstallError::CapabilityFailed {
            component,
            version,
            reason,
            rolled_back_to,
            report,
        }) => {
            record_error(store, "E_VERIFY_FAILED", "install_verify", &reason, "");
            Ok(ComponentActionOutcome {
                ok: false,
                component,
                version,
                already_installed: false,
                quarantined: true,
                rolled_back_to,
                error_code: Some("E_VERIFY_FAILED".to_string()),
                message: security::redact_sensitive_text(&reason),
                capability_report: report.as_deref().map(CapabilityReportView::from),
            })
        }
        Err(installer::InstallError::Quarantined {
            component,
            version,
            reason,
        }) => {
            record_error(store, "E_VERIFY_FAILED", "install_quarantined", &reason, "");
            Ok(ComponentActionOutcome {
                ok: false,
                component,
                version,
                already_installed: false,
                quarantined: true,
                rolled_back_to: None,
                error_code: Some("E_VERIFY_FAILED".to_string()),
                message: security::redact_sensitive_text(&reason),
                capability_report: None,
            })
        }
        Err(err) => {
            let code = err.error_code();
            let msg_key = err.message_key();
            let redacted = security::redact_sensitive_text(&err.to_string());
            record_error_with_key(store, code, &msg_key, "install", &redacted, "");
            Err(AppCommandError::with_message_key(
                code,
                msg_key,
                redacted.clone(),
                redacted,
            ))
        }
    }
}

/// Checks pre-install free disk space against `sum(missing/selected sizes) * 1.5 + 1 GB`.
fn enforce_preinstall_disk_space(
    ws_root: &Path,
    store: &LocalStore,
    entries: &[manifest::ComponentManifestEntry],
) -> Result<(), AppCommandError> {
    let payload_sum: u64 = entries.iter().map(|e| e.size_bytes).sum();
    let required = compute_preinstall_required_bytes(payload_sum);
    let available = security::get_free_disk_space(ws_root).unwrap_or(0);
    if available < required {
        let msg = format!(
            "Ruang disk tidak mencukupi: tersedia {:.2} GB, dibutuhkan {:.2} GB.",
            available as f64 / (1024.0 * 1024.0 * 1024.0),
            required as f64 / (1024.0 * 1024.0 * 1024.0)
        );
        record_error(store, "E_DISK_FULL", "preinstall_disk_check", &msg, "");
        return Err(AppCommandError::disk_full(msg, available, required));
    }
    Ok(())
}

/// Installs one component/model from the currently cached verified manifest.
#[tauri::command]
#[specta::specta]
pub async fn install_component(
    app: tauri::AppHandle,
    component: String,
    version: Option<String>,
) -> Result<ComponentActionOutcome, AppCommandError> {
    let (ws_root, store, in_use) = deps_from_app(&app)?;
    let (seq, entry) = get_cached_manifest_entry(&app, &store, &component, version.as_deref())?;
    enforce_preinstall_disk_space(&ws_root, &store, std::slice::from_ref(&entry))?;

    let job_id = format!("install-{}-{}", entry.component, entry.version);
    let sz = entry.size_bytes.max(1);
    install_single_entry(
        &app,
        &ws_root,
        &store,
        &in_use,
        &entry,
        InstallBatchCursor {
            job_id: &job_id,
            manifest_sequence: seq,
            completed_items: 0,
            total_items: 1,
            prior_bytes_done: 0,
            batch_total_bytes: sz,
        },
    )
    .await
}

/// Installs all missing/outdated required components & active speech model ("Pasang semua")
/// after verifying `sum(sizes) * 1.5 + 1 GB` free disk space.
#[tauri::command]
#[specta::specta]
pub async fn install_all_required_components(
    app: tauri::AppHandle,
) -> Result<BatchInstallOutcome, AppCommandError> {
    let (ws_root, store, in_use) = deps_from_app(&app)?;
    let hw = hardware::probe_hardware_profile();
    let speech_state = speech::resolve_speech_model_state(&store, hw.ram_bytes_u64())
        .map_err(|e| AppCommandError::simple("E_UNKNOWN", e.to_string(), e.to_string()))?;
    let required = required_component_list(&speech_state);

    let cache = app
        .try_state::<ManifestCacheState>()
        .and_then(|c| c.0.lock().ok().and_then(|g| g.clone()))
        .ok_or_else(|| {
            let msg = "Manifest belum dimuat. Periksa koneksi lalu klik Coba Lagi.".to_string();
            record_error(&store, "E_NET_OFFLINE", "install_all", &msg, "");
            AppCommandError::simple("E_NET_OFFLINE", msg.clone(), msg)
        })?;

    let mut pending_entries: Vec<manifest::ComponentManifestEntry> = Vec::new();
    let mut skipped_count = 0u32;

    for req_name in &required {
        if let Some(entry) = cache.components.iter().find(|c| &c.component == req_name) {
            let already_ok = store
                .get_component_version(&entry.component, &entry.version)
                .ok()
                .flatten()
                .filter(|r| r.status == "installed" && r.is_current)
                .is_some_and(|r| installer::quick_verify_row(&ws_root, &r).is_ok());
            if already_ok {
                skipped_count = skipped_count.saturating_add(1);
            } else {
                pending_entries.push(entry.clone());
            }
        }
    }

    enforce_preinstall_disk_space(&ws_root, &store, &pending_entries)?;

    let total_items = pending_entries.len() as u32;
    let total_bytes: u64 = pending_entries.iter().map(|e| e.size_bytes.max(1)).sum();
    let job_id = format!("install-all-{}", uuid::Uuid::new_v4().simple());

    let mut outcomes = Vec::with_capacity(pending_entries.len());
    let mut bytes_done = 0u64;

    for (idx, entry) in pending_entries.iter().enumerate() {
        let out = install_single_entry(
            &app,
            &ws_root,
            &store,
            &in_use,
            entry,
            InstallBatchCursor {
                job_id: &job_id,
                manifest_sequence: cache.sequence,
                completed_items: idx as u32,
                total_items,
                prior_bytes_done: bytes_done,
                batch_total_bytes: total_bytes,
            },
        )
        .await?;
        bytes_done = bytes_done.saturating_add(entry.size_bytes.max(1));
        outcomes.push(out);
    }

    let installed_count = outcomes.iter().filter(|o| o.ok).count() as u32;
    Ok(BatchInstallOutcome {
        installed_count,
        skipped_count,
        outcomes,
    })
}

/// Repairs a missing or quarantined component by re-downloading & verifying from the manifest.
#[tauri::command]
#[specta::specta]
pub async fn repair_component(
    app: tauri::AppHandle,
    component: String,
    version: Option<String>,
) -> Result<VerifyRepairOutcome, AppCommandError> {
    let (ws_root, store, in_use) = deps_from_app(&app)?;
    let (seq, entry) = get_cached_manifest_entry(&app, &store, &component, version.as_deref())?;
    enforce_preinstall_disk_space(&ws_root, &store, std::slice::from_ref(&entry))?;

    let app_progress = app.clone();
    let comp_name = entry.component.clone();
    let res = installer::repair_component_from_manifest(
        &ws_root,
        &store,
        &in_use,
        &entry,
        seq,
        move |p| {
            let _ = ComponentInstallProgressEvent {
                job_id: format!("repair-{}", comp_name),
                component: p.component,
                stage: p.stage,
                completed_items: 0,
                total_items: 1,
                item_completed_bytes: p.completed_bytes as f64,
                item_total_bytes: p.total_bytes as f64,
                item_percent: p.percent,
                rate_bps: p.rate_bps,
                item_eta_seconds: p.eta_seconds,
                overall_percent: p.percent,
                overall_eta_seconds: p.eta_seconds,
                overall_eta_low_seconds: p.eta_seconds.map(|e| e * 0.75),
                overall_eta_high_seconds: p.eta_seconds.map(|e| e * 1.5),
            }
            .emit(&app_progress);
        },
        None,
    )
    .await;

    res.map_err(|e| {
        let code = e.error_code();
        let msg_key = e.message_key();
        let redacted = security::redact_sensitive_text(&e.to_string());
        record_error_with_key(&store, code, &msg_key, "repair", &redacted, "");
        AppCommandError::with_message_key(code, msg_key, redacted.clone(), redacted)
    })
}

/// Re-runs capability verification for an already-extracted quarantined binary version.
#[tauri::command]
#[specta::specta]
pub async fn retry_quarantined_component(
    app: tauri::AppHandle,
    component: String,
    version: String,
) -> Result<VerifyRepairOutcome, AppCommandError> {
    let (ws_root, store, _) = deps_from_app(&app)?;
    let out =
        installer::retry_quarantined_component_verification(&ws_root, &store, &component, &version)
            .await
            .map_err(|e| {
                let code = e.error_code();
                let redacted = security::redact_sensitive_text(&e.to_string());
                record_error(&store, code, "retry_quarantine", &redacted, "");
                AppCommandError::simple(code, redacted.clone(), redacted)
            })?;
    if !out.intact {
        if let Some(code) = &out.error_code {
            record_error(&store, code, "retry_quarantine", &out.message, "");
        }
    }
    Ok(out)
}

/// Runs full SHA-256 + capability verification on the currently active component version.
#[tauri::command]
#[specta::specta]
pub async fn verify_component(
    app: tauri::AppHandle,
    component: String,
) -> Result<VerifyRepairOutcome, AppCommandError> {
    let (ws_root, store, _) = deps_from_app(&app)?;
    let out = installer::verify_active_component_full(&ws_root, &store, &component).await;
    if !out.intact {
        if let Some(code) = &out.error_code {
            record_error(&store, code, "verify_component", &out.message, "");
        }
    }
    Ok(out)
}

/// Validates a user-selected directory via `DataRoot::validate` and writes `bootstrap.json`.
/// Applies on next app restart; performs NO automatic data migration.
#[tauri::command]
#[specta::specta]
pub fn select_custom_data_root(
    app: tauri::AppHandle,
    folder_path: String,
) -> Result<DataRootChangeOutcome, AppCommandError> {
    let (_, store, _) = deps_from_app(&app)?;
    let target = PathBuf::from(folder_path.trim());
    let validated = DataRoot::validate(&target).map_err(|e| {
        let code = e.error_code();
        let msg = security::redact_sensitive_text(&e.to_string());
        record_error(&store, code, "data_root_select", &msg, "");
        AppCommandError::simple(code, msg.clone(), msg)
    })?;

    let default_root = security::get_default_workspace_root()
        .map_err(|e| AppCommandError::simple(e.error_code(), e.to_string(), e.to_string()))?;

    DataRoot::write_bootstrap(&default_root, &validated.path).map_err(|e| {
        let code = e.error_code();
        let msg = security::redact_sensitive_text(&e.to_string());
        record_error(&store, code, "data_root_write_bootstrap", &msg, "");
        AppCommandError::simple(code, msg.clone(), msg)
    })?;

    tracing::info!(
        "bootstrap.json updated to new DataRoot '{}' (requires restart)",
        security::redact_sensitive_text(&validated.path.to_string_lossy())
    );

    Ok(DataRootChangeOutcome {
        new_root_redacted: security::redact_sensitive_text(&validated.path.to_string_lossy()),
        is_onedrive: validated.is_onedrive,
        requires_restart: true,
    })
}

fn build_settings_overview(
    ws_root: &Path,
    store: &LocalStore,
) -> Result<SettingsOverview, AppCommandError> {
    let hw = hardware::probe_hardware_profile();
    let language = store
        .get_setting("language")
        .ok()
        .flatten()
        .filter(|l| l == "id" || l == "en")
        .unwrap_or_else(|| "id".to_string());
    let speech_state = speech::resolve_speech_model_state(store, hw.ram_bytes_u64())
        .map_err(|e| AppCommandError::simple("E_UNKNOWN", e.to_string(), e.to_string()))?;
    let encoder_status = encoder::get_encoder_status(store, &hw)
        .map_err(|e| AppCommandError::simple("E_UNKNOWN", e.to_string(), e.to_string()))?;
    let dr = DataRoot::resolve_active().ok();

    Ok(SettingsOverview {
        language,
        speech_state,
        encoder_status,
        hardware: hw,
        data_root_redacted: security::redact_sensitive_text(&ws_root.to_string_lossy()),
        is_default_data_root: dr.as_ref().is_none_or(|d| d.is_default),
        is_onedrive: dr.as_ref().is_some_and(|d| d.is_onedrive),
    })
}

/// Returns current settings, speech-model state, passive encoder chain status, and hardware summary.
#[tauri::command]
#[specta::specta]
pub fn get_settings_overview(app: tauri::AppHandle) -> Result<SettingsOverview, AppCommandError> {
    let (ws_root, store, _) = deps_from_app(&app)?;
    build_settings_overview(&ws_root, &store)
}

/// Persists non-sensitive settings updates (language, speech model preference, banner dismissal).
#[tauri::command]
#[specta::specta]
pub fn update_settings(
    app: tauri::AppHandle,
    patch: SettingsPatch,
) -> Result<SettingsOverview, AppCommandError> {
    let (ws_root, store, _) = deps_from_app(&app)?;

    if let Some(lang) = patch.language {
        if lang == "id" || lang == "en" {
            let _ = store.set_setting("language", &lang);
        }
    }
    if let Some(sel) = patch.speech_model_selection {
        if sel == "auto" {
            let _ = speech::set_speech_model_preference(&store, None);
        } else if let Some(model) = SpeechModel::from_str_id(&sel) {
            let _ = speech::set_speech_model_preference(&store, Some(model));
        }
    }
    if patch.dismiss_speech_downgrade == Some(true) {
        let _ = speech::dismiss_speech_downgrade_suggestion(&store);
    }

    build_settings_overview(&ws_root, &store)
}

/// Returns whether the previous run exited cleanly or crashed.
#[tauri::command]
#[specta::specta]
pub fn get_crash_recovery_status(app: tauri::AppHandle) -> CrashRecoveryStatus {
    if let Some(state) = app.try_state::<Mutex<CrashRecoveryStatus>>() {
        if let Ok(guard) = state.lock() {
            return guard.clone();
        }
    }
    CrashRecoveryStatus {
        unclean_shutdown_detected: false,
        previous_started_at: None,
        crash_record: None,
        banner_dismissed: false,
    }
}

/// Dismisses the crash-recovery banner for the current session and archives the crash marker file.
#[tauri::command]
#[specta::specta]
pub fn dismiss_crash_recovery(app: tauri::AppHandle) -> Result<(), AppCommandError> {
    let (ws_root, _, _) = deps_from_app(&app)?;
    crash::clear_crash_record_file(&ws_root.join("logs"));
    if let Some(state) = app.try_state::<Mutex<CrashRecoveryStatus>>() {
        if let Ok(mut guard) = state.lock() {
            guard.banner_dismissed = true;
            guard.unclean_shutdown_detected = false;
        }
    }
    Ok(())
}

/// Lists recent redacted error events stored in SQLite (`error_events`), newest first.
#[tauri::command]
#[specta::specta]
pub fn list_error_events(
    app: tauri::AppHandle,
    code_filter: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<ErrorEventRow>, AppCommandError> {
    let (_, store, _) = deps_from_app(&app)?;
    store
        .list_error_events(code_filter.as_deref(), limit.unwrap_or(100) as usize)
        .map_err(|e| AppCommandError::simple("E_UNKNOWN", e.to_string(), e.to_string()))
}

/// Records an unhandled WebView / React JS error into `error_events` (rate-limited & size-capped).
#[tauri::command]
#[specta::specta]
pub fn record_webview_error(
    app: tauri::AppHandle,
    source: String,
    message: String,
    stack: Option<String>,
) -> Result<bool, AppCommandError> {
    const MAX_PER_MINUTE: usize = 10;
    const MAX_FIELD_BYTES: usize = 1024;

    if let Some(limiter) = app.try_state::<WebViewErrorRateLimiter>() {
        if let Ok(mut q) = limiter.0.lock() {
            let now = Instant::now();
            while q
                .front()
                .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60))
            {
                q.pop_front();
            }
            if q.len() >= MAX_PER_MINUTE {
                return Ok(false);
            }
            q.push_back(now);
        }
    }

    let (_, store, _) = deps_from_app(&app)?;
    let truncate = |raw: String| -> String {
        let mut s = security::redact_sensitive_text(raw.trim());
        if s.len() > MAX_FIELD_BYTES {
            let mut end = MAX_FIELD_BYTES;
            while end > 0 && !s.is_char_boundary(end) {
                end -= 1;
            }
            s.truncate(end);
            s.push_str("...[TRUNCATED]");
        }
        s
    };

    let detail = truncate(format!("[{}] {}", source, message));
    let tail = truncate(stack.unwrap_or_default());

    let _ = store.record_error_event(NewErrorEvent {
        code: "E_UNKNOWN".to_string(),
        stage: "webview".to_string(),
        job_id: None,
        message_key: "err.E_UNKNOWN.body".to_string(),
        detail,
        stderr_tail: tail,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        component_versions_json: "{}".to_string(),
    });
    tracing::error!("Captured redacted WebView error from source={}", source);
    Ok(true)
}

fn build_bundle_for_app(
    app: &tauri::AppHandle,
    user_note: String,
    include_video_ids: bool,
) -> Result<DiagnosticsReportBundle, AppCommandError> {
    let (ws_root, store, in_use) = deps_from_app(app)?;
    let hw = hardware::probe_hardware_profile();
    let resolver = ComponentResolver::new(ws_root.clone(), store.clone(), in_use);
    let snapshot = resolver
        .diagnostics_snapshot()
        .map_err(|e| AppCommandError::simple(e.error_code(), e.to_string(), e.to_string()))?;
    let speech_state = speech::resolve_speech_model_state(&store, hw.ram_bytes_u64())
        .unwrap_or_else(|_| speech::SpeechModelState {
            active_model: speech::recommended_speech_model(hw.ram_bytes_u64()),
            recommended_model: speech::recommended_speech_model(hw.ram_bytes_u64()),
            is_user_override: false,
            downgrade_suggested: false,
            suggested_lighter_model: None,
            consecutive_slow_runs: 0,
            options: Vec::new(),
        });
    let encoder_status = encoder::get_encoder_status(&store, &hw).unwrap_or_else(|_| {
        encoder::EncoderStatusSnapshot {
            chain_order: vec![],
            cached_encoder: None,
            cached_fingerprint: None,
            current_fingerprint: hw.fingerprint.clone(),
            fingerprint_matches: false,
            is_windows_n_kn: hw.is_windows_n_kn,
            fallback_history: vec![],
        }
    });
    let crash_record = app
        .try_state::<Mutex<CrashRecoveryStatus>>()
        .and_then(|s| s.lock().ok().and_then(|g| g.crash_record.clone()))
        .or_else(|| crash::read_latest_crash_record(&ws_root.join("logs")));

    Ok(diagnostics::build_diagnostics_report(
        &ws_root,
        &store,
        &hw,
        &snapshot,
        &speech_state,
        &encoder_status,
        &DiagnosticsReportOptions {
            user_note,
            include_video_ids,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            crash_record,
        },
    ))
}

/// Generates a redacted plain-text/Markdown diagnostics report completely locally.
#[tauri::command]
#[specta::specta]
pub fn generate_diagnostics_report(
    app: tauri::AppHandle,
    user_note: String,
    include_video_ids: bool,
) -> Result<DiagnosticsReportBundle, AppCommandError> {
    let _ = EtaEstimator::new();
    build_bundle_for_app(&app, user_note, include_video_ids)
}

/// Saves the full (`<= 1 MB`) redacted diagnostics report to a user-chosen `.txt` file.
#[tauri::command]
#[specta::specta]
pub fn save_diagnostics_report_file(
    app: tauri::AppHandle,
    dest_path: String,
    user_note: String,
    include_video_ids: bool,
) -> Result<SaveReportOutcome, AppCommandError> {
    let target = PathBuf::from(dest_path.trim());
    let ext_ok = target
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("txt"));
    if !target.is_absolute()
        || target.to_string_lossy().starts_with(r"\\")
        || security::is_system_directory(&target)
        || !ext_ok
    {
        return Err(AppCommandError::simple(
            "E_UNKNOWN",
            "Tujuan file harus path absolut lokal dengan ekstensi .txt.",
            "Invalid report save destination",
        ));
    }

    let bundle = build_bundle_for_app(&app, user_note, include_video_ids)?;
    std::fs::write(&target, bundle.full_text.as_bytes()).map_err(|e| {
        AppCommandError::simple(
            "E_DISK_FULL",
            format!("Gagal menyimpan file laporan: {}", e),
            security::redact_sensitive_text(&e.to_string()),
        )
    })?;

    Ok(SaveReportOutcome {
        saved_path_redacted: security::redact_sensitive_text(&target.to_string_lossy()),
        bytes_written: bundle.full_bytes,
        truncated: bundle.full_truncated,
    })
}

/// Opens the workspace `logs/` directory in Windows Explorer via `tauri-plugin-opener`.
#[tauri::command]
#[specta::specta]
pub fn open_logs_folder(app: tauri::AppHandle) -> Result<(), AppCommandError> {
    use tauri_plugin_opener::OpenerExt;

    let (ws_root, _, _) = deps_from_app(&app)?;
    let logs_dir = ws_root.join("logs");
    let _ = std::fs::create_dir_all(&logs_dir);

    app.opener()
        .open_path(logs_dir.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| {
            AppCommandError::simple(
                "E_UNKNOWN",
                format!("Gagal membuka folder log: {}", e),
                security::redact_sensitive_text(&e.to_string()),
            )
        })
}

/// Builds a strictly validated YouTube watch URL (`https://www.youtube.com/watch?v=<id>&t=<n>s`)
/// from a stored project's provider and 11-char video ID.
pub fn build_youtube_watch_url_from_stored(
    provider: &str,
    video_id: &str,
    start_seconds: Option<f64>,
) -> Result<String, AppCommandError> {
    if provider != "youtube" {
        return Err(AppCommandError::simple(
            "E_YT_URL_INVALID",
            "Tautan eksternal hanya didukung untuk sumber YouTube.".to_string(),
            format!("Unsupported provider for browser open: {}", provider),
        ));
    }
    let valid_id = video_id.len() == 11
        && video_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !valid_id {
        return Err(AppCommandError::simple(
            "E_YT_URL_INVALID",
            "ID video YouTube tersimpan tidak valid.".to_string(),
            "Invalid stored YouTube video ID".to_string(),
        ));
    }
    match start_seconds {
        Some(secs) if secs.is_finite() && secs >= 0.0 => {
            let clamped = (secs.floor() as u64).min(86_400);
            Ok(format!(
                "https://www.youtube.com/watch?v={}&t={}s",
                video_id, clamped
            ))
        }
        _ => Ok(format!("https://www.youtube.com/watch?v={}", video_id)),
    }
}

/// Opens a stored project's canonical YouTube watch URL in the user's system browser.
/// Replaces frontend `opener:allow-open-url` capability by constructing the URL strictly in Rust
/// from the stored, validated `source_video_id`.
#[tauri::command]
#[specta::specta]
pub fn open_video_in_browser(
    app: tauri::AppHandle,
    project_id: String,
    start_seconds: Option<f64>,
) -> Result<String, AppCommandError> {
    use tauri_plugin_opener::OpenerExt;

    let (_, store, _) = deps_from_app(&app)?;
    let project = ProjectRepo::new(&store)
        .get(&project_id)
        .map_err(|e| {
            let msg = security::redact_sensitive_text(&e.to_string());
            AppCommandError::simple("E_UNKNOWN", msg.clone(), msg)
        })?
        .ok_or_else(|| {
            AppCommandError::simple(
                "E_UNKNOWN",
                "Proyek tidak ditemukan.".to_string(),
                "Project not found for open_video_in_browser".to_string(),
            )
        })?;

    let url = build_youtube_watch_url_from_stored(
        &project.source_provider,
        &project.source_video_id,
        start_seconds,
    )?;

    app.opener().open_url(&url, None::<&str>).map_err(|e| {
        AppCommandError::simple(
            "E_UNKNOWN",
            format!("Gagal membuka video di browser: {}", e),
            security::redact_sensitive_text(&e.to_string()),
        )
    })?;

    Ok(url)
}

struct ProbeTokenCleanup {
    registry: Arc<Mutex<HashMap<String, CancellationToken>>>,
    request_id: String,
}

impl Drop for ProbeTokenCleanup {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.registry.lock() {
            guard.remove(&self.request_id);
        }
    }
}

/// Validates and probes a pasted YouTube single-video URL via supervised `yt-dlp` + `qjs`,
/// records any failure in local `error_events` (redacted), and inserts a `probed` project
/// record into SQLite on success.
#[tauri::command]
#[specta::specta]
pub async fn probe_youtube_url(
    app: tauri::AppHandle,
    raw_url: String,
    request_id: String,
) -> Result<ProbeProjectResponse, AppCommandError> {
    let (ws_root, store, in_use) = deps_from_app(&app)?;
    let cancel_state = app
        .try_state::<ProbeCancelRegistry>()
        .map(|s| s.0.clone())
        .unwrap_or_default();

    let clean_req_id = if request_id.trim().is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        request_id.trim().chars().take(64).collect()
    };

    let cancel_token = CancellationToken::new();
    if let Ok(mut guard) = cancel_state.lock() {
        guard.insert(clean_req_id.clone(), cancel_token.clone());
    }
    let _cleanup = ProbeTokenCleanup {
        registry: cancel_state,
        request_id: clean_req_id.clone(),
    };

    let resolver = ComponentResolver::new(ws_root.clone(), store.clone(), in_use);
    let app_for_emit = app.clone();
    let req_for_emit = clean_req_id.clone();

    let outcome = probe::probe_youtube_url(
        &ws_root,
        &resolver,
        &raw_url,
        Some(&cancel_token),
        move |update| {
            let _ = ProbeProgressEvent {
                request_id: req_for_emit.clone(),
                operation: update.operation,
                stage: update.stage,
                indeterminate: update.indeterminate,
                elapsed_ms: update.elapsed_ms,
                heartbeat: update.heartbeat,
            }
            .emit(&app_for_emit);
        },
    )
    .await
    .map_err(|err| {
        if !matches!(err, ProbeError::Cancelled) {
            record_error_with_key(
                &store,
                err.code(),
                &err.message_key(),
                "probe",
                &err.redacted_detail(),
                "",
            );
        }
        AppCommandError::with_message_key(
            err.code(),
            err.message_key(),
            security::redact_sensitive_text(&err.to_string()),
            err.redacted_detail(),
        )
    })?;

    let project = ProjectRepo::new(&store)
        .create_from_probe(&outcome.parsed_url, &outcome.info)
        .map_err(|e| {
            let msg = security::redact_sensitive_text(&e.to_string());
            record_error(&store, "E_UNKNOWN", "probe", &msg, "");
            AppCommandError::simple("E_UNKNOWN", msg.clone(), msg)
        })?;

    if let Some(cache_state) = app.try_state::<ProbeHandleCache>() {
        cache_state.insert(
            &project.source_provider,
            &project.source_video_id,
            outcome.handle,
        );
    } else {
        let _ = probe::cleanup_probe_handle(&outcome.handle);
    }

    Ok(ProbeProjectResponse {
        probe_info: outcome.info,
        project,
        suggested_start_seconds: outcome.parsed_url.suggested_start_seconds,
    })
}

/// Cancels an active YouTube probe identified by `request_id`.
#[tauri::command]
#[specta::specta]
pub fn cancel_youtube_probe(
    app: tauri::AppHandle,
    request_id: String,
) -> Result<bool, AppCommandError> {
    let Some(cancel_state) = app.try_state::<ProbeCancelRegistry>() else {
        return Ok(false);
    };
    let Ok(guard) = cancel_state.0.lock() else {
        return Ok(false);
    };
    if let Some(token) = guard.get(request_id.trim()) {
        token.cancel();
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Lists the 10 most recent projects from SQLite `projects`.
#[tauri::command]
#[specta::specta]
pub fn list_recent_projects(app: tauri::AppHandle) -> Result<Vec<ProjectRecord>, AppCommandError> {
    let (_, store, _) = deps_from_app(&app)?;
    ProjectRepo::new(&store).list_recent(10).map_err(|e| {
        let msg = security::redact_sensitive_text(&e.to_string());
        AppCommandError::simple("E_UNKNOWN", msg.clone(), msg)
    })
}

/// Deletes a project record from SQLite and removes its `projects/<id>` workspace directory.
#[tauri::command]
#[specta::specta]
pub fn delete_project(app: tauri::AppHandle, project_id: String) -> Result<bool, AppCommandError> {
    let (ws_root, store, _) = deps_from_app(&app)?;
    ProjectRepo::new(&store)
        .delete(&ws_root, &project_id)
        .map_err(|e| {
            let msg = security::redact_sensitive_text(&e.to_string());
            AppCommandError::simple("E_UNKNOWN", msg.clone(), msg)
        })
}

/// Fetches a single project record by ID (used when reopening from "Proyek terakhir").
#[tauri::command]
#[specta::specta]
pub fn get_project(
    app: tauri::AppHandle,
    project_id: String,
) -> Result<Option<ProjectRecord>, AppCommandError> {
    let (_, store, _) = deps_from_app(&app)?;
    ProjectRepo::new(&store).get(&project_id).map_err(|e| {
        let msg = security::redact_sensitive_text(&e.to_string());
        AppCommandError::simple("E_UNKNOWN", msg.clone(), msg)
    })
}

/// Persists validated clip range boundaries (`range_start_ms`, `range_end_ms`) and sets
/// `status = 'ranged'` for the project.
#[tauri::command]
#[specta::specta]
pub fn update_project_range(
    app: tauri::AppHandle,
    project_id: String,
    range_start_ms: u32,
    range_end_ms: u32,
) -> Result<ProjectRecord, AppCommandError> {
    let (_, store, _) = deps_from_app(&app)?;
    ProjectRepo::new(&store)
        .update_range(&project_id, range_start_ms, range_end_ms)
        .map_err(|e| {
            let msg = security::redact_sensitive_text(&e.to_string());
            AppCommandError::simple("E_RANGE_INVALID", msg.clone(), msg)
        })
}

/// Returns preview bridge URL/protocol/control video/timeout/range limits configuration.
/// In release builds, `bridge_url` is fixed to `https://clipper-bridge.vercel.app` and
/// `can_force_timecode_only` is strictly `false`.
#[tauri::command]
#[specta::specta]
pub fn get_preview_config() -> PreviewConfigResponse {
    #[cfg(debug_assertions)]
    let bridge_url = std::env::var("CLIPPER_BRIDGE_URL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "https://clipper-bridge.vercel.app".to_string());

    #[cfg(not(debug_assertions))]
    let bridge_url = "https://clipper-bridge.vercel.app".to_string();

    PreviewConfigResponse {
        bridge_url,
        bridge_protocol_version: 1,
        control_video_id: "aqz-KE-bpKQ".to_string(),
        ready_timeout_ms: 6000,
        max_range_seconds: 300,
        download_prior_rt_factor: 1.66,
        download_prior_startup_seconds: 10.0,
        can_force_timecode_only: cfg!(debug_assertions),
    }
}

/// Records non-PII preview diagnostics (`mode`, `bridge_ready_ms`, `fallback_reason`,
/// `yt_error_code`), updates allowlisted aggregate counters in `settings`, and logs
/// typed preview errors (`E_PREVIEW_CONFIG`, `E_YT_UNAVAILABLE`) to `error_events`.
#[tauri::command]
#[specta::specta]
pub fn record_preview_event(
    app: tauri::AppHandle,
    input: PreviewEventInput,
) -> Result<PreviewCountersSnapshot, AppCommandError> {
    let (_, store, _) = deps_from_app(&app)?;
    let counters = store
        .record_preview_telemetry(
            &input.mode,
            input.bridge_ready_ms,
            input.fallback_reason.as_deref(),
            input.yt_error_code,
            input.counter_kind.as_deref(),
        )
        .map_err(|e| {
            let msg = security::redact_sensitive_text(&e.to_string());
            AppCommandError::simple("E_UNKNOWN", msg.clone(), msg)
        })?;

    if let Some(ref code) = input.record_error_code {
        let detail = format!(
            "mode={} ready_ms={:?} fallback_reason={} yt_code={:?}",
            input.mode,
            input.bridge_ready_ms,
            input.fallback_reason.as_deref().unwrap_or("-"),
            input.yt_error_code
        );
        record_error(&store, code, "preview", &detail, "");
    }

    Ok(counters)
}

fn validated_project_range_ms(
    project: &ProjectRecord,
    max_range_seconds: u32,
) -> Result<(u32, u32), AppCommandError> {
    let (Some(start_f), Some(end_f)) = (project.range_start_ms, project.range_end_ms) else {
        return Err(AppCommandError::simple(
            "E_RANGE_INVALID",
            "Simpan rentang klip terlebih dahulu sebelum mengunduh.".to_string(),
            "Project has no persisted range_start_ms/range_end_ms".to_string(),
        ));
    };
    if !start_f.is_finite() || !end_f.is_finite() || start_f < 0.0 || end_f <= start_f {
        return Err(AppCommandError::simple(
            "E_RANGE_INVALID",
            "Rentang klip tidak valid.".to_string(),
            "Invalid persisted clip range bounds".to_string(),
        ));
    }
    let start_ms = start_f.round() as u32;
    let end_ms = end_f.round() as u32;
    let dur_ms = end_ms.saturating_sub(start_ms);
    if dur_ms < 1000 {
        return Err(AppCommandError::simple(
            "E_RANGE_INVALID",
            "Durasi klip minimal 1 detik.".to_string(),
            "Range shorter than 1000 ms".to_string(),
        ));
    }
    if dur_ms > max_range_seconds.saturating_mul(1000) {
        return Err(AppCommandError::simple(
            "E_RANGE_TOO_LONG",
            format!(
                "Durasi klip melebihi batas maksimal {} detik.",
                max_range_seconds
            ),
            format!("Range {} ms exceeds limit {} s", dur_ms, max_range_seconds),
        ));
    }
    if let Err(dl_err) = compute_download_window(
        start_ms,
        end_ms,
        project.duration_seconds,
        DEFAULT_RANGE_HANDLE_SECONDS,
    ) {
        return Err(AppCommandError::simple(
            dl_err.error_code(),
            dl_err.to_string(),
            security::redact_sensitive_text(&dl_err.to_string()),
        ));
    }
    Ok((start_ms, end_ms))
}

#[allow(clippy::too_many_arguments)]
fn spawn_generate_pipeline_task(
    app: &tauri::AppHandle,
    ws_root: PathBuf,
    store: LocalStore,
    in_use: InUseRegistry,
    coordinator: JobCoordinator,
    probe_cache: ProbeHandleCache,
    job: JobRecord,
    project: ProjectRecord,
    start_ms: u32,
    end_ms: u32,
    max_range_seconds: u32,
    cancel_token: CancellationToken,
) {
    let app_handle = app.clone();
    let device_profile = hardware::probe_hardware_profile().device_profile_key();

    tauri::async_runtime::spawn(async move {
        let sink: Arc<dyn Fn(JobEventPayload) + Send + Sync> = Arc::new(move |payload| {
            let _ = payload.emit(&app_handle);
        });
        let _ = execute_generate_job(GenerateRunParams {
            ws_root,
            store,
            in_use,
            coordinator,
            probe_cache,
            job,
            project,
            selected_start_ms: start_ms,
            selected_end_ms: end_ms,
            device_profile,
            max_range_seconds,
            handle_seconds: DEFAULT_RANGE_HANDLE_SECONDS,
            extra_sidecar_env: Vec::new(),
            free_space_override: None,
            cancel_token,
            event_sink: sink,
        })
        .await;
    });
}

/// Starts (or idempotently reuses) the persistent M1.4 `generate` pipeline job for `project_id`.
#[tauri::command]
#[specta::specta]
pub fn start_generate_job(
    app: tauri::AppHandle,
    project_id: String,
) -> Result<JobRecord, AppCommandError> {
    let (ws_root, store, in_use) = deps_from_app(&app)?;
    let coordinator = app
        .try_state::<JobCoordinator>()
        .map(|s| (*s).clone())
        .unwrap_or_default();
    let probe_cache = app
        .try_state::<ProbeHandleCache>()
        .map(|s| (*s).clone())
        .unwrap_or_default();

    let project = ProjectRepo::new(&store)
        .get(&project_id)
        .map_err(|e| AppCommandError::simple("E_UNKNOWN", e.to_string(), e.to_string()))?
        .ok_or_else(|| {
            AppCommandError::simple(
                "E_UNKNOWN",
                "Proyek tidak ditemukan.".to_string(),
                "Project not found for start_generate_job".to_string(),
            )
        })?;

    let preview_cfg = get_preview_config();
    let (start_ms, end_ms) = validated_project_range_ms(&project, preview_cfg.max_range_seconds)?;
    let window = compute_download_window(
        start_ms,
        end_ms,
        project.duration_seconds,
        DEFAULT_RANGE_HANDLE_SECONDS,
    )
    .map_err(|e| AppCommandError::simple(e.error_code(), e.to_string(), e.to_string()))?;

    let idempotency_key = compute_generate_idempotency_key(
        &project.id,
        &project.source_provider,
        &project.source_video_id,
        start_ms,
        end_ms,
        YT_FORMAT_SELECTOR,
        DEFAULT_RANGE_HANDLE_SECONDS,
    );
    let cache_key = compute_download_cache_key(
        &project.source_provider,
        &project.source_video_id,
        &window,
        YT_FORMAT_SELECTOR,
    );
    let cached_rel = format!("cache/{}.mp4", cache_key);
    let cached_file_present = security::resolve_workspace_path_with_root(&ws_root, &cached_rel)
        .map(|p| p.is_file())
        .unwrap_or(false);
    let already_downloaded_and_cached =
        project.status == "downloaded" && project.composition_hash.is_some() && cached_file_present;

    let disposition = coordinator
        .begin_generate(
            &store,
            &project.id,
            &idempotency_key,
            already_downloaded_and_cached,
        )
        .map_err(|e| {
            let code = e.error_code();
            let msg = security::redact_sensitive_text(&e.to_string());
            AppCommandError::simple(code, msg.clone(), msg)
        })?;

    match disposition {
        StartJobDisposition::ReusedExisting { job } => Ok(job),
        StartJobDisposition::Started { job, token } => {
            spawn_generate_pipeline_task(
                &app,
                ws_root,
                store,
                in_use,
                coordinator,
                probe_cache,
                job.clone(),
                project,
                start_ms,
                end_ms,
                preview_cfg.max_range_seconds,
                token,
            );
            Ok(job)
        }
    }
}

/// Fetches a single persistent job record by ID.
#[tauri::command]
#[specta::specta]
pub fn get_job(
    app: tauri::AppHandle,
    job_id: String,
) -> Result<Option<JobRecord>, AppCommandError> {
    let (_, store, _) = deps_from_app(&app)?;
    JobRepo::new(&store).get(&job_id).map_err(|e| {
        let msg = security::redact_sensitive_text(&e.to_string());
        AppCommandError::simple("E_UNKNOWN", msg.clone(), msg)
    })
}

/// Lists persistent job records (all recent or filtered to a single `project_id`).
#[tauri::command]
#[specta::specta]
pub fn list_jobs(
    app: tauri::AppHandle,
    project_id: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<JobRecord>, AppCommandError> {
    let (_, store, _) = deps_from_app(&app)?;
    let clamped = limit.unwrap_or(20).clamp(1, 50) as usize;
    let repo = JobRepo::new(&store);
    let res = match project_id {
        Some(ref pid) if !pid.trim().is_empty() => repo.list_for_project(pid.trim(), clamped),
        _ => repo.list_recent(clamped),
    };
    res.map_err(|e| {
        let msg = security::redact_sensitive_text(&e.to_string());
        AppCommandError::simple("E_UNKNOWN", msg.clone(), msg)
    })
}

/// Requests immediate cancellation of an active (`queued`/`running`) job.
#[tauri::command]
#[specta::specta]
pub fn cancel_job(app: tauri::AppHandle, job_id: String) -> Result<bool, AppCommandError> {
    let (ws_root, store, _) = deps_from_app(&app)?;
    let coordinator = app
        .try_state::<JobCoordinator>()
        .map(|s| (*s).clone())
        .unwrap_or_default();
    coordinator
        .cancel_job(&ws_root, &store, &job_id)
        .map_err(|e| {
            let msg = security::redact_sensitive_text(&e.to_string());
            AppCommandError::simple(e.error_code(), msg.clone(), msg)
        })
}

/// Retries a `failed`, `cancelled`, or crash-`interrupted` generate job cleanly.
#[tauri::command]
#[specta::specta]
pub fn retry_job(app: tauri::AppHandle, job_id: String) -> Result<JobRecord, AppCommandError> {
    let (ws_root, store, in_use) = deps_from_app(&app)?;
    let coordinator = app
        .try_state::<JobCoordinator>()
        .map(|s| (*s).clone())
        .unwrap_or_default();
    let probe_cache = app
        .try_state::<ProbeHandleCache>()
        .map(|s| (*s).clone())
        .unwrap_or_default();

    let (job, token) = coordinator.retry_generate(&store, &job_id).map_err(|e| {
        let msg = security::redact_sensitive_text(&e.to_string());
        AppCommandError::simple(e.error_code(), msg.clone(), msg)
    })?;

    let project = ProjectRepo::new(&store)
        .get(&job.project_id)
        .map_err(|e| AppCommandError::simple("E_UNKNOWN", e.to_string(), e.to_string()))?
        .ok_or_else(|| {
            AppCommandError::simple(
                "E_UNKNOWN",
                "Proyek tidak ditemukan untuk mengulangi proses.".to_string(),
                "Project not found for retry_job".to_string(),
            )
        })?;

    let preview_cfg = get_preview_config();
    let (start_ms, end_ms) =
        match validated_project_range_ms(&project, preview_cfg.max_range_seconds) {
            Ok(bounds) => bounds,
            Err(err) => {
                coordinator.finish_active(&job.id);
                let _ = JobRepo::new(&store).mark_failed(
                    &job.id,
                    job.stage.as_deref().unwrap_or("probe"),
                    &err.code,
                    &err.detail,
                    None,
                );
                return Err(err);
            }
        };

    spawn_generate_pipeline_task(
        &app,
        ws_root,
        store,
        in_use,
        coordinator,
        probe_cache,
        job.clone(),
        project,
        start_ms,
        end_ms,
        preview_cfg.max_range_seconds,
        token,
    );
    Ok(job)
}

/// Deletes cached source/proxy artifact files and composition payload for `project_id` while
/// preserving the project metadata and saved range (`Hapus data unduhan`).
#[tauri::command]
#[specta::specta]
pub fn delete_project_download_data(
    app: tauri::AppHandle,
    project_id: String,
) -> Result<ProjectRecord, AppCommandError> {
    let (ws_root, store, _) = deps_from_app(&app)?;
    let coordinator = app
        .try_state::<JobCoordinator>()
        .map(|s| (*s).clone())
        .unwrap_or_default();

    if let Some(active_id) = coordinator.active_job_id() {
        if let Ok(Some(active_job)) = JobRepo::new(&store).get(&active_id) {
            if active_job.project_id == project_id {
                return Err(AppCommandError::simple(
                    "E_JOB_BUSY",
                    "Hentikan proses yang sedang berjalan sebelum menghapus data unduhan."
                        .to_string(),
                    "Cannot delete download data while project job is active".to_string(),
                ));
            }
        }
    }

    let unreferenced_paths = ArtifactRepo::new(&store)
        .delete_for_project_and_collect_unreferenced_paths(&project_id)
        .map_err(|e| {
            let msg = security::redact_sensitive_text(&e.to_string());
            AppCommandError::simple("E_UNKNOWN", msg.clone(), msg)
        })?;

    for rel in unreferenced_paths {
        if let Ok(abs) = security::resolve_workspace_path_with_root(&ws_root, &rel) {
            if abs.is_file() {
                let _ = std::fs::remove_file(&abs);
            }
        }
    }

    if let Ok(comp_abs) = security::resolve_workspace_path_with_root(
        &ws_root,
        &format!("projects/{}/composition.json", project_id),
    ) {
        if comp_abs.is_file() {
            let _ = std::fs::remove_file(&comp_abs);
        }
    }

    ProjectRepo::new(&store)
        .clear_downloaded_data(&project_id)
        .map_err(|e| {
            let msg = security::redact_sensitive_text(&e.to_string());
            AppCommandError::simple("E_UNKNOWN", msg.clone(), msg)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_app_info() {
        let info = get_app_info();
        assert_eq!(info.name, "ClipperHelper");
        assert!(!info.version.is_empty());
        assert!(!info.platform.is_empty());
    }

    #[test]
    fn test_preinstall_required_bytes_formula() {
        // sum(100 MB) * 1.5 + 1 GB
        let sum = 100 * 1024 * 1024u64;
        assert_eq!(
            compute_preinstall_required_bytes(sum),
            (150 * 1024 * 1024u64) + (1024 * 1024 * 1024u64)
        );
    }

    #[test]
    fn test_build_youtube_watch_url_from_stored_validates_id_and_clamps_time() {
        assert_eq!(
            build_youtube_watch_url_from_stored("youtube", "WufkeDaJisQ", Some(125.9)).unwrap(),
            "https://www.youtube.com/watch?v=WufkeDaJisQ&t=125s"
        );
        assert_eq!(
            build_youtube_watch_url_from_stored("youtube", "WufkeDaJisQ", Some(0.0)).unwrap(),
            "https://www.youtube.com/watch?v=WufkeDaJisQ&t=0s"
        );
        assert_eq!(
            build_youtube_watch_url_from_stored("youtube", "WufkeDaJisQ", Some(999_999.0)).unwrap(),
            "https://www.youtube.com/watch?v=WufkeDaJisQ&t=86400s"
        );
        assert_eq!(
            build_youtube_watch_url_from_stored("youtube", "WufkeDaJisQ", None).unwrap(),
            "https://www.youtube.com/watch?v=WufkeDaJisQ"
        );
        assert!(build_youtube_watch_url_from_stored("local_file", "WufkeDaJisQ", None).is_err());
        assert!(build_youtube_watch_url_from_stored("youtube", "bad-id!", None).is_err());
        assert!(
            build_youtube_watch_url_from_stored("youtube", "WufkeDaJisQ&evil=1", None).is_err()
        );
    }
}
