//! Thin Tauri commands layer (ARCHITECTURE §4; M1.1d-lite Parts 1–4).
//!
//! Exposes production typed Tauri IPC commands to the frontend:
//! - App & Workspace status + DataRoot relocation (`bootstrap.json`)
//! - User-facing Component Manager (manifest fetch, pre-install disk check, install-all with
//!   per-item + overall progress/ETA, repair, retry quarantine, verify)
//! - Passive Hardware / Speech / Encoder / Language settings
//! - Diagnostics store, crash recovery, WebView JS error capture (rate-limited + capped),
//!   redacted report generator, `.txt` export, and narrow logs folder opener.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::Manager;
use tauri_specta::Event;

use crate::capabilities::CapabilityReportView;
use crate::crash::{self, CrashRecoveryStatus};
use crate::diagnostics::{
    self, DiagnosticsReportBundle, DiagnosticsReportOptions, REQUIRED_DISK_MIN_BUFFER_BYTES,
};
use crate::encoder::{self, EncoderStatusSnapshot};
use crate::eta::EtaEstimator;
use crate::hardware::{self, HardwareProfile};
use crate::installer::{self, InUseRegistry, VerifyRepairOutcome};
use crate::manifest::{self, ComponentManifest};
use crate::resolver::{ComponentDiagnosticsSnapshot, ComponentResolver};
use crate::security::{self, DataRoot};
use crate::speech::{self, SpeechModel, SpeechModelState};
use crate::store::{ErrorEventRow, LocalStore, NewErrorEvent};
use crate::WorkspaceState;

pub const DEFAULT_PRODUCTION_MANIFEST_URL: &str =
    "https://cdn.clipperhelper.com/manifests/windows-x86_64.json";

/// Managed state caching the latest verified component manifest in production commands.
#[derive(Default, Clone)]
pub struct ManifestCacheState(pub Arc<Mutex<Option<ComponentManifest>>>);

/// Managed state for server-side rate limiting of WebView JS error capture.
#[derive(Default, Clone)]
pub struct WebViewErrorRateLimiter(pub Arc<Mutex<VecDeque<Instant>>>);

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
}

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
    let _ = store.record_error_event(NewErrorEvent {
        code: code.to_string(),
        stage: stage.to_string(),
        job_id: None,
        message_key: error_key_for_code(code),
        detail: detail.to_string(),
        stderr_tail: stderr_tail.to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        component_versions_json: "{}".to_string(),
    });
}

fn resolve_manifest_urls(override_url: Option<&str>) -> ManifestUrlConfig {
    #[cfg(debug_assertions)]
    let url = {
        let mut resolved = DEFAULT_PRODUCTION_MANIFEST_URL.to_string();
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
    if let Some(status) = app.try_state::<WorkspaceStatus>() {
        (*status).clone()
    } else {
        WorkspaceStatus {
            is_initialized: false,
            root: None,
            error_code: Some("E_APP_INIT_FAILED".to_string()),
            error_message: Some("Workspace status uninitialized".to_string()),
        }
    }
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
        let redacted = security::redact_sensitive_text(&e.to_string());
        record_error(&store, code, "manifest_fetch", &redacted, "");
        AppCommandError::simple(code, redacted.clone(), redacted)
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
            let ver_display = version.unwrap_or("(latest)");
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
            let redacted = security::redact_sensitive_text(&err.to_string());
            record_error(store, code, "install", &redacted, "");
            Err(AppCommandError::simple(code, redacted.clone(), redacted))
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
        let redacted = security::redact_sensitive_text(&e.to_string());
        record_error(&store, code, "repair", &redacted, "");
        AppCommandError::simple(code, redacted.clone(), redacted)
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
}
