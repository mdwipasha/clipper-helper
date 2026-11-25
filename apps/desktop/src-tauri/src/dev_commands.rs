//! Debug developer commands (gated behind cfg(debug_assertions)).
//!
//! NEVER included in release builds. Used by the frontend debug panel.

#![cfg(debug_assertions)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{Emitter, Manager};

use crate::capabilities::{CapabilityReportView, CapabilityStatus};
use crate::commands::WorkspaceStatus;
use crate::installer::{
    self, GcOutcome, InUseRegistry, VerifyRepairOutcome, DEFAULT_KEEP_PREVIOUS_VERSIONS,
};
use crate::manifest::ComponentManifest;
use crate::resolver::{ComponentDiagnosticsSnapshot, ComponentResolver, CustomFfmpegStatus};
use crate::sidecars::{SidecarSpec, SidecarSupervisor};
use crate::store::LocalStore;
use crate::WorkspaceState;

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct DevCommandError {
    pub code: String,
    pub message: String,
}

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct DevInstallOutcome {
    pub ok: bool,
    pub component: String,
    pub version: String,
    pub kind: String,
    pub already_installed: bool,
    pub quarantined: bool,
    pub rolled_back_to: Option<String>,
    pub error_code: Option<String>,
    pub message: String,
    pub capability_report: Option<CapabilityReportView>,
}

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct DevFixtureServerStatus {
    pub state: String, // "stopped" | "starting" | "running" | "error"
    pub port: Option<u16>,
    pub url: Option<String>,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct DevManifestError {
    pub code: String,
    pub message: String,
}

struct FixtureServerState {
    process: Option<Child>,
    port: Option<u16>,
    url: Option<String>,
    last_error: Option<String>,
}

static FIXTURE_SERVER: Mutex<FixtureServerState> = Mutex::new(FixtureServerState {
    process: None,
    port: None,
    url: None,
    last_error: None,
});

static LAST_VERIFIED_MANIFEST: Mutex<Option<ComponentManifest>> = Mutex::new(None);

pub const DEV_EXTRA_KEYS: &[crate::manifest::ManifestPublicKey] =
    &[crate::manifest::ManifestPublicKey {
        kid: "clipper-manifest-dev",
        public_key_hex: "aaaf48f0cbd01c1f79573f139a5b0401b488ac44e0b103d64dea533b075c997b",
    }];

/// Resolves embedded dev key plus optional runtime `.dev-keys/clipper-manifest-dev.pub.hex`
/// (debug-only helper so freshly generated dev keys work cleanly without editing source).
fn load_dev_keys() -> Vec<crate::manifest::ManifestPublicKey> {
    let mut keys = DEV_EXTRA_KEYS.to_vec();
    let pub_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../.dev-keys/clipper-manifest-dev.pub.hex");
    if let Ok(raw_hex) = std::fs::read_to_string(pub_path) {
        let trimmed = raw_hex.trim().to_string();
        if trimmed.len() == 64 && !keys.iter().any(|k| k.public_key_hex == trimmed) {
            let leaked: &'static str = Box::leak(trimmed.into_boxed_str());
            keys.push(crate::manifest::ManifestPublicKey {
                kid: "clipper-manifest-dev",
                public_key_hex: leaked,
            });
        }
    }
    keys
}

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct DevManifestComponent {
    pub component: String,
    pub version: String,
    pub platform: String,
    pub url: String,
    pub size_bytes: f64,
    pub sha256: String,
    pub archive: String,
    pub files_count: u32,
    pub is_downloaded: bool,
}

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct DevManifestInfo {
    pub sequence: f64,
    pub channel: String,
    pub generated_at: String,
    pub signature_valid: bool,
    pub components: Vec<DevManifestComponent>,
}

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct DevDownloadProgressEvent {
    pub component: String,
    pub stage: String,
    pub unit: String,
    pub completed_bytes: f64,
    pub total_bytes: f64,
    pub percent: f64,
    pub rate_bps: f64,
    pub eta_seconds: Option<f64>,
}

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct DevSystemInfo {
    pub schema_version: u32,
    pub workspace_root: String,
    pub redacted_workspace_root: String,
    pub free_disk_space_formatted: String,
    pub free_disk_space_bytes: f64,
    pub platform: String,
    pub arch: String,
}

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct DevSidecarRunResult {
    pub exit_code: i32,
    pub lines: Vec<String>,
    pub stderr_tail: String,
    pub elapsed_ms: u32,
}

/// Locate the fake-sidecar executable in the target directory.
fn find_fake_sidecar_binary() -> Result<PathBuf, String> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest_bin = manifest_dir
        .join("target")
        .join("debug")
        .join("fake-sidecar.exe");
    if manifest_bin.exists() {
        return Ok(manifest_bin);
    }

    let current_exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut curr = current_exe.parent();
    while let Some(dir) = curr {
        if dir.file_name().is_some_and(|n| n == "debug") {
            let p = dir.join("fake-sidecar.exe");
            if p.exists() {
                return Ok(p);
            }
        }
        curr = dir.parent();
    }

    Err(format!(
        "fake-sidecar binary not found at {:?}. Build it with `cargo build -p fake-sidecar`.",
        manifest_bin
    ))
}

/// Returns internal system information for the dev panel.
#[tauri::command]
#[specta::specta]
pub fn dev_get_system_info(app: tauri::AppHandle) -> Result<DevSystemInfo, String> {
    let ws_status = app
        .try_state::<WorkspaceStatus>()
        .ok_or_else(|| "Workspace status not available".to_string())?;

    if !ws_status.is_initialized {
        return Err(format!(
            "Workspace not initialized: [{}] {}",
            ws_status.error_code.as_deref().unwrap_or("UNKNOWN"),
            ws_status.error_message.as_deref().unwrap_or("")
        ));
    }

    let ws = app
        .try_state::<WorkspaceState>()
        .ok_or_else(|| "Workspace state not available".to_string())?;
    let store = app
        .try_state::<LocalStore>()
        .ok_or_else(|| "LocalStore not available".to_string())?;

    let schema_version = store.schema_version().map_err(|e| e.to_string())?;
    let root_path = &ws.root;
    let free_bytes = crate::security::get_free_disk_space(root_path).unwrap_or(0);

    let free_gb = free_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    let free_disk_space_formatted = format!("{:.1} GB free", free_gb);

    let raw_root = root_path.to_string_lossy().to_string();
    let redacted_workspace_root = crate::security::redact_sensitive_text(&raw_root);

    Ok(DevSystemInfo {
        schema_version,
        workspace_root: raw_root,
        redacted_workspace_root,
        free_disk_space_formatted,
        free_disk_space_bytes: free_bytes as f64,
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    })
}

/// Debug-only: resets the dev workspace, clearing files and re-running migrations.
#[tauri::command]
#[specta::specta]
pub fn dev_reset_workspace(app: tauri::AppHandle) -> Result<(), String> {
    let ws = app
        .try_state::<WorkspaceState>()
        .ok_or_else(|| "Workspace state not available".to_string())?;

    crate::security::assert_can_reset_dev_workspace(&ws.root).map_err(|e| e.to_string())?;

    for sub in &["bin", "models", "cache", "projects", "logs"] {
        let dir = ws.root.join(sub);
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    crate::security::ensure_workspace_with_root(&ws.root).map_err(|e| e.to_string())?;

    if let Some(store) = app.try_state::<LocalStore>() {
        store.reset_database().map_err(|e| e.to_string())?;
    }
    if let Ok(mut guard) = LAST_VERIFIED_MANIFEST.lock() {
        *guard = None;
    }

    Ok(())
}

/// Runs the fake-sidecar with the requested mode and returns stdout lines.
#[tauri::command]
#[specta::specta]
pub async fn dev_run_fake_sidecar(mode: String) -> Result<DevSidecarRunResult, String> {
    let bin = find_fake_sidecar_binary()?;
    let args: Vec<String> = std::iter::once("--mode".to_string())
        .chain(mode.split_whitespace().map(String::from))
        .collect();

    let spec = SidecarSpec::new(&bin, args);
    let supervisor = SidecarSupervisor::new();

    let start = Instant::now();
    match supervisor.run_to_completion(&spec, None).await {
        Ok(res) => Ok(DevSidecarRunResult {
            exit_code: res.exit_code,
            lines: res.stdout_lines,
            stderr_tail: res.stderr_tail,
            elapsed_ms: start.elapsed().as_millis() as u32,
        }),
        Err(crate::sidecars::SidecarError::ExecutionFailed {
            exit_code,
            stderr_tail,
        }) => Ok(DevSidecarRunResult {
            exit_code: exit_code.unwrap_or(-1),
            lines: Vec::new(),
            stderr_tail,
            elapsed_ms: start.elapsed().as_millis() as u32,
        }),
        Err(e) => Err(e.to_string()),
    }
}

/// Resolves the Node.js executable using an explicit path or debug override,
/// avoiding reliance on stripped sidecar PATH environment (ARCHITECTURE §13, SECURITY §9).
pub fn find_node_executable() -> Result<PathBuf, String> {
    if let Ok(custom) = std::env::var("CLIPPER_DEV_NODE_PATH") {
        let p = PathBuf::from(&custom);
        if p.exists() {
            return Ok(p);
        }
        tracing::warn!(
            "CLIPPER_DEV_NODE_PATH was set to {:?}, but file does not exist",
            custom
        );
    }

    let candidates = [
        r"C:\Program Files\nodejs\node.exe",
        r"C:\Program Files (x86)\nodejs\node.exe",
    ];
    for c in &candidates {
        let p = PathBuf::from(c);
        if p.exists() {
            return Ok(p);
        }
    }

    if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
        let fnm = PathBuf::from(&localappdata).join(r"fnm_multishells\default\node.exe");
        if fnm.exists() {
            return Ok(fnm);
        }
        let fnm2 = PathBuf::from(&localappdata).join(r"fnm\current\node.exe");
        if fnm2.exists() {
            return Ok(fnm2);
        }
    }

    if let Ok(nvm_symlink) = std::env::var("NVM_SYMLINK") {
        let p = PathBuf::from(&nvm_symlink).join("node.exe");
        if p.exists() {
            return Ok(p);
        }
    }
    if let Ok(nvm_home) = std::env::var("NVM_HOME") {
        if let Ok(entries) = std::fs::read_dir(&nvm_home) {
            for entry in entries.flatten() {
                let p = entry.path().join("node.exe");
                if p.exists() {
                    return Ok(p);
                }
            }
        }
    }

    // Try `where.exe node` on Windows to find it from system PATH
    if let Ok(output) = Command::new("where.exe").arg("node").output() {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(first_line) = stdout.lines().next() {
                let p = PathBuf::from(first_line.trim());
                if p.exists() {
                    return Ok(p);
                }
            }
        }
    }

    Ok(PathBuf::from("node"))
}

/// Debug-only: inspects the status of the local fixture server.
#[tauri::command]
#[specta::specta]
pub fn dev_get_fixture_server_status() -> DevFixtureServerStatus {
    let mut state = FIXTURE_SERVER.lock().unwrap();
    if let Some(ref mut child) = state.process {
        match child.try_wait() {
            Ok(Some(exit_status)) => {
                state.process = None;
                let err_msg = format!("Process exited with status {}", exit_status);
                state.last_error = Some(err_msg.clone());
                return DevFixtureServerStatus {
                    state: "stopped".to_string(),
                    port: None,
                    url: None,
                    error: Some(err_msg),
                };
            }
            Ok(None) => {
                return DevFixtureServerStatus {
                    state: "running".to_string(),
                    port: state.port,
                    url: state.url.clone(),
                    error: None,
                };
            }
            Err(e) => {
                let err_msg = format!("Error checking process: {}", e);
                state.last_error = Some(err_msg.clone());
                return DevFixtureServerStatus {
                    state: "error".to_string(),
                    port: None,
                    url: None,
                    error: Some(err_msg),
                };
            }
        }
    }

    DevFixtureServerStatus {
        state: "stopped".to_string(),
        port: None,
        url: None,
        error: state.last_error.clone(),
    }
}

/// Debug-only: stops the local fixture server if running.
#[tauri::command]
#[specta::specta]
pub fn dev_stop_fixture_server() -> Result<DevFixtureServerStatus, String> {
    let mut state = FIXTURE_SERVER.lock().map_err(|e| e.to_string())?;
    if let Some(ref mut child) = state.process {
        let _ = child.kill();
        let _ = child.wait();
    }
    state.process = None;
    state.port = None;
    state.url = None;
    state.last_error = None;
    Ok(DevFixtureServerStatus {
        state: "stopped".to_string(),
        port: None,
        url: None,
        error: None,
    })
}

/// Debug-only: starts or verifies the local dev-server serving test fixtures.
#[tauri::command]
#[specta::specta]
pub fn dev_start_fixture_server() -> Result<DevFixtureServerStatus, String> {
    let mut state = FIXTURE_SERVER.lock().map_err(|e| e.to_string())?;
    if let Some(ref mut child) = state.process {
        if let Ok(None) = child.try_wait() {
            return Ok(DevFixtureServerStatus {
                state: "running".to_string(),
                port: state.port,
                url: state.url.clone(),
                error: None,
            });
        }
    }

    let node_bin = find_node_executable().map_err(|e| format!("Node resolution failed: {}", e))?;

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root_dir = manifest_dir.join("../../../tools/dev-fixtures");
    let script_path = manifest_dir.join("../../../tools/src/dev-server.js");

    // If dev-fixtures does not exist or missing manifest, generate them using resolved node binary
    if !root_dir.join("manifest.json").exists() {
        let fixture_gen_script = manifest_dir.join("../../../tools/src/create-dev-fixtures.js");
        let gen_status = Command::new(&node_bin)
            .arg(&fixture_gen_script)
            .status()
            .map_err(|e| {
                format!(
                    "Failed to generate dev fixtures using {:?}: {}",
                    node_bin, e
                )
            })?;
        if !gen_status.success() {
            return Err("create-dev-fixtures script failed".to_string());
        }
    }

    let port: u16 = 8080;
    let mut child = Command::new(&node_bin)
        .arg(&script_path)
        .arg("--root")
        .arg(&root_dir)
        .arg("--port")
        .arg(port.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("Failed to spawn dev-server using {:?}: {}", node_bin, e))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to capture dev-server stdout".to_string())?;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| format!("Failed to read dev-server ready signal: {}", e))?;

    let prefix = "SERVER_READY: ";
    if !line.starts_with(prefix) {
        let _ = child.kill();
        let err = if line.trim().is_empty() {
            "Failed to start fixture server: process exited before emitting ready signal (port 8080 may already be in use)".to_string()
        } else {
            format!("Unexpected dev-server output: {}", line)
        };
        state.last_error = Some(err.clone());
        return Err(err);
    }
    let url = line[prefix.len()..].trim().to_string();

    state.process = Some(child);
    state.port = Some(port);
    state.url = Some(url.clone());
    state.last_error = None;

    Ok(DevFixtureServerStatus {
        state: "running".to_string(),
        port: Some(port),
        url: Some(url),
        error: None,
    })
}

/// Debug-only: fetches and cryptographically verifies the component manifest.
#[tauri::command]
#[specta::specta]
pub async fn dev_fetch_manifest(
    app: tauri::AppHandle,
    manifest_url: String,
    sig_url: String,
) -> Result<DevManifestInfo, DevManifestError> {
    let ws = app
        .try_state::<WorkspaceState>()
        .ok_or_else(|| DevManifestError {
            code: "E_DEV_STATE_UNAVAILABLE".to_string(),
            message: "Workspace state not available".to_string(),
        })?;
    let store = app
        .try_state::<LocalStore>()
        .ok_or_else(|| DevManifestError {
            code: "E_DEV_STORE_UNAVAILABLE".to_string(),
            message: "LocalStore not available".to_string(),
        })?;

    tracing::info!(
        "dev_fetch_manifest: fetching from {} and {}",
        manifest_url,
        sig_url
    );
    let dev_keys = load_dev_keys();
    let manifest = match crate::updater::fetch_and_verify_manifest(
        &manifest_url,
        &sig_url,
        "0.1.0",
        &store,
        Some(&dev_keys),
    )
    .await
    {
        Ok(m) => {
            tracing::info!(
                "dev_fetch_manifest: verification succeeded! seq={}, components={}",
                m.sequence,
                m.components.len()
            );
            m
        }
        Err(e) => {
            tracing::error!("dev_fetch_manifest: verification failed: {:?}", e);
            return Err(DevManifestError {
                code: e.error_code().to_string(),
                message: e.to_string(),
            });
        }
    };

    if let Ok(mut guard) = LAST_VERIFIED_MANIFEST.lock() {
        *guard = Some(manifest.clone());
    }

    let mut dev_components = Vec::new();
    for c in manifest.components {
        let is_downloaded = store
            .get_component_version(&c.component, &c.version)
            .ok()
            .flatten()
            .filter(|r| r.status == "installed" && r.is_current)
            .is_some_and(|r| installer::quick_verify_row(&ws.root, &r).is_ok());

        dev_components.push(DevManifestComponent {
            component: c.component,
            version: c.version,
            platform: c.platform,
            url: c.url,
            size_bytes: c.size_bytes as f64,
            sha256: c.sha256,
            archive: c.archive,
            files_count: c.files.len() as u32,
            is_downloaded,
        });
    }

    Ok(DevManifestInfo {
        sequence: manifest.sequence as f64,
        channel: manifest.channel,
        generated_at: manifest.generated_at,
        signature_valid: true,
        components: dev_components,
    })
}

fn find_manifest_entry(
    component: &str,
    version: Option<&str>,
) -> Result<(u64, crate::manifest::ComponentManifestEntry), DevCommandError> {
    let guard = LAST_VERIFIED_MANIFEST.lock().map_err(|_| DevCommandError {
        code: "E_UNKNOWN".to_string(),
        message: "Manifest cache lock poisoned".to_string(),
    })?;
    let manifest = guard.as_ref().ok_or_else(|| DevCommandError {
        code: "E_COMPONENT_MISSING".to_string(),
        message: "No verified manifest loaded yet. Click 'Fetch Manifest' first.".to_string(),
    })?;

    let entry = manifest
        .components
        .iter()
        .find(|c| c.component == component && version.map(|v| v == c.version).unwrap_or(true))
        .cloned()
        .ok_or_else(|| DevCommandError {
            code: "E_COMPONENT_MISSING".to_string(),
            message: format!(
                "Component '{}' (version {:?}) was not found in the active loaded manifest",
                component, version
            ),
        })?;

    Ok((manifest.sequence, entry))
}

fn get_app_deps(
    app: &tauri::AppHandle,
) -> Result<(PathBuf, LocalStore, InUseRegistry), DevCommandError> {
    let ws = app
        .try_state::<WorkspaceState>()
        .ok_or_else(|| DevCommandError {
            code: "E_APP_INIT_FAILED".to_string(),
            message: "Workspace state not available".to_string(),
        })?;
    let store = app
        .try_state::<LocalStore>()
        .ok_or_else(|| DevCommandError {
            code: "E_APP_INIT_FAILED".to_string(),
            message: "LocalStore not available".to_string(),
        })?;
    let in_use = app
        .try_state::<InUseRegistry>()
        .map(|s| (*s).clone())
        .unwrap_or_default();
    Ok((ws.root.clone(), (*store).clone(), in_use))
}

/// Debug-only: downloads and verifies a raw component payload from the manifest with live progress.
#[tauri::command]
#[specta::specta]
pub async fn dev_download_component(
    app: tauri::AppHandle,
    component: String,
    version: String,
    url: String,
    size_bytes: f64,
    sha256: String,
    archive: String,
) -> Result<(), String> {
    tracing::info!(
        "dev_download_component invoked for '{}' from '{}'",
        component,
        url
    );
    let (ws_root, _, _) = get_app_deps(&app).map_err(|e| e.message)?;

    let dl_dir = ws_root.join("cache").join("downloads");
    let dest_path = dl_dir.join(format!("{}-{}.pkg", component, version));

    let entry = crate::manifest::ComponentManifestEntry {
        component: component.clone(),
        version,
        platform: "windows-x86_64".to_string(),
        url,
        mirrors: vec![],
        size_bytes: size_bytes as u64,
        sha256,
        archive,
        files: vec![],
        min_app_version: "0.1.0".to_string(),
        max_app_version: None,
        rollout_percent: 100,
    };

    let app_handle = app.clone();
    crate::updater::download_and_verify_component(
        &entry,
        &dest_path,
        move |progress| {
            let _ = app_handle.emit(
                "download-progress",
                DevDownloadProgressEvent {
                    component: progress.component,
                    stage: progress.stage,
                    unit: progress.unit,
                    completed_bytes: progress.completed_bytes as f64,
                    total_bytes: progress.total_bytes as f64,
                    percent: progress.percent,
                    rate_bps: progress.rate_bps,
                    eta_seconds: progress.eta_seconds,
                },
            );
        },
        None,
    )
    .await
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// Debug-only: runs the complete M1.1c atomic install pipeline (download -> verify -> safe extract ->
/// atomic rename -> promote -> supervised capability check -> quarantine+rollback on failure -> GC).
#[tauri::command]
#[specta::specta]
pub async fn dev_install_component(
    app: tauri::AppHandle,
    component: String,
    version: String,
) -> Result<DevInstallOutcome, DevCommandError> {
    let (ws_root, store, in_use) = get_app_deps(&app)?;
    let (manifest_sequence, entry) = find_manifest_entry(&component, Some(&version))?;
    let total_bytes = entry.size_bytes as f64;

    let app_progress = app.clone();
    let res = installer::download_and_install_component(
        &ws_root,
        &store,
        &in_use,
        &entry,
        manifest_sequence,
        move |progress| {
            let _ = app_progress.emit(
                "download-progress",
                DevDownloadProgressEvent {
                    component: progress.component,
                    stage: progress.stage,
                    unit: progress.unit,
                    completed_bytes: progress.completed_bytes as f64,
                    total_bytes: progress.total_bytes as f64,
                    percent: progress.percent,
                    rate_bps: progress.rate_bps,
                    eta_seconds: progress.eta_seconds,
                },
            );
        },
        None,
    )
    .await;

    let _ = app.emit(
        "download-progress",
        DevDownloadProgressEvent {
            component: component.clone(),
            stage: "complete".to_string(),
            unit: "bytes".to_string(),
            completed_bytes: total_bytes,
            total_bytes,
            percent: 100.0,
            rate_bps: 0.0,
            eta_seconds: Some(0.0),
        },
    );

    match res {
        Ok(outcome) => Ok(DevInstallOutcome {
            ok: true,
            component: outcome.component,
            version: outcome.version,
            kind: outcome.kind,
            already_installed: outcome.already_installed,
            quarantined: false,
            rolled_back_to: None,
            error_code: None,
            message: if outcome.already_installed {
                "Already installed & verified".to_string()
            } else {
                "Installed and passed capability verification".to_string()
            },
            capability_report: outcome
                .capability_report
                .as_ref()
                .map(CapabilityReportView::from),
        }),
        Err(installer::InstallError::CapabilityFailed {
            component,
            version,
            reason,
            rolled_back_to,
            report,
        }) => Ok(DevInstallOutcome {
            ok: false,
            component,
            version,
            kind: "bin".to_string(),
            already_installed: false,
            quarantined: true,
            rolled_back_to,
            error_code: Some("E_VERIFY_FAILED".to_string()),
            message: format!(
                "REFUSED & QUARANTINED: Failed capability check ({})",
                reason
            ),
            capability_report: report.as_deref().map(CapabilityReportView::from),
        }),
        Err(installer::InstallError::Quarantined {
            component,
            version,
            reason,
        }) => Ok(DevInstallOutcome {
            ok: false,
            component,
            version,
            kind: "bin".to_string(),
            already_installed: false,
            quarantined: true,
            rolled_back_to: None,
            error_code: Some("E_VERIFY_FAILED".to_string()),
            message: format!("Quarantined build refused: {}", reason),
            capability_report: None,
        }),
        Err(err) => Err(DevCommandError {
            code: err.error_code().to_string(),
            message: err.to_string(),
        }),
    }
}

/// Debug-only: returns complete per-component versions, capabilities, and custom FFmpeg state.
#[tauri::command]
#[specta::specta]
pub fn dev_get_component_diagnostics(
    app: tauri::AppHandle,
) -> Result<ComponentDiagnosticsSnapshot, DevCommandError> {
    let (ws_root, store, in_use) = get_app_deps(&app)?;
    let resolver = ComponentResolver::new(ws_root, store, in_use);
    resolver
        .diagnostics_snapshot()
        .map_err(|e| DevCommandError {
            code: e.error_code().to_string(),
            message: e.to_string(),
        })
}

/// Debug-only: switches a component back to `previous` or rolls forward/backward to `target_version`.
#[tauri::command]
#[specta::specta]
pub fn dev_rollback_component(
    app: tauri::AppHandle,
    component: String,
    target_version: Option<String>,
) -> Result<String, String> {
    let (ws_root, store, _) = get_app_deps(&app).map_err(|e| e.message)?;
    installer::rollback_component(&ws_root, &store, &component, target_version.as_deref())
        .map_err(|e| e.to_string())
}

/// Debug-only: runs version garbage collection (`keep_previous = 2`) skipping in-use versions.
#[tauri::command]
#[specta::specta]
pub fn dev_gc_component(app: tauri::AppHandle, component: String) -> Result<GcOutcome, String> {
    let (ws_root, store, in_use) = get_app_deps(&app).map_err(|e| e.message)?;
    installer::gc_component_versions(
        &ws_root,
        &store,
        &component,
        DEFAULT_KEEP_PREVIOUS_VERSIONS,
        &in_use,
    )
    .map_err(|e| e.to_string())
}

/// Debug-only: executes full SHA-256 verification and live capability probe on the active version.
#[tauri::command]
#[specta::specta]
pub async fn dev_verify_component(
    app: tauri::AppHandle,
    component: String,
) -> Result<VerifyRepairOutcome, String> {
    let (ws_root, store, _) = get_app_deps(&app).map_err(|e| e.message)?;
    Ok(installer::verify_active_component_full(&ws_root, &store, &component).await)
}

/// Debug-only: re-downloads and verifies a missing or damaged component from the active manifest.
#[tauri::command]
#[specta::specta]
pub async fn dev_repair_component(
    app: tauri::AppHandle,
    component: String,
    version: Option<String>,
) -> Result<VerifyRepairOutcome, DevCommandError> {
    let (ws_root, store, in_use) = get_app_deps(&app)?;
    let ver_to_find = match version {
        Some(v) => Some(v),
        None => store
            .get_current_component(&component)
            .ok()
            .flatten()
            .map(|r| r.version),
    };
    let (manifest_sequence, entry) = find_manifest_entry(&component, ver_to_find.as_deref())
        .or_else(|_| find_manifest_entry(&component, None))?;

    let app_progress = app.clone();
    installer::repair_component_from_manifest(
        &ws_root,
        &store,
        &in_use,
        &entry,
        manifest_sequence,
        move |progress| {
            let _ = app_progress.emit(
                "download-progress",
                DevDownloadProgressEvent {
                    component: progress.component,
                    stage: progress.stage,
                    unit: progress.unit,
                    completed_bytes: progress.completed_bytes as f64,
                    total_bytes: progress.total_bytes as f64,
                    percent: progress.percent,
                    rate_bps: progress.rate_bps,
                    eta_seconds: progress.eta_seconds,
                },
            );
        },
        None,
    )
    .await
    .map_err(|e| DevCommandError {
        code: e.error_code().to_string(),
        message: e.to_string(),
    })
}

/// Debug-only: validates custom `ffmpeg.exe` + `ffprobe.exe` on a local fixed drive and verifies
/// capabilities in WARN mode (sets persistent warning flag).
#[tauri::command]
#[specta::specta]
pub async fn dev_set_custom_ffmpeg(
    app: tauri::AppHandle,
    path: String,
) -> Result<CustomFfmpegStatus, DevCommandError> {
    let (ws_root, store, in_use) = get_app_deps(&app)?;
    let resolver = ComponentResolver::new(ws_root, store, in_use);
    resolver
        .set_custom_ffmpeg_path(&path)
        .await
        .map_err(|e| DevCommandError {
            code: e.error_code().to_string(),
            message: e.to_string(),
        })
}

/// Debug-only: clears the custom FFmpeg path override and restores managed FFmpeg resolution.
#[tauri::command]
#[specta::specta]
pub fn dev_clear_custom_ffmpeg(app: tauri::AppHandle) -> Result<CustomFfmpegStatus, String> {
    let (ws_root, store, in_use) = get_app_deps(&app).map_err(|e| e.message)?;
    let resolver = ComponentResolver::new(ws_root, store, in_use);
    let status = resolver
        .clear_custom_ffmpeg_path()
        .map_err(|e| e.to_string())?;
    let _ = CapabilityStatus::Pass;
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_node_executable_resolves_valid_binary() {
        let node_bin = find_node_executable().expect("find_node_executable should succeed");
        let output = Command::new(&node_bin)
            .arg("--version")
            .output()
            .expect("node --version must execute");
        assert!(output.status.success(), "node --version must exit with 0");
        let version_str = String::from_utf8_lossy(&output.stdout);
        assert!(
            version_str.starts_with('v'),
            "expected node version vX.Y.Z, got {}",
            version_str
        );
    }

    #[test]
    fn test_dev_fixture_server_lifecycle() {
        // Ensure stopped initially
        let _ = dev_stop_fixture_server();
        let status = dev_get_fixture_server_status();
        assert_eq!(status.state, "stopped");

        // Start server
        let start_res = dev_start_fixture_server();
        assert!(
            start_res.is_ok(),
            "Failed to start dev fixture server: {:?}",
            start_res
        );
        let started = start_res.unwrap();
        assert_eq!(started.state, "running");
        assert_eq!(started.port, Some(8080));
        assert!(started.url.is_some());

        // Verify status reflects running state & start is idempotent
        let status_running = dev_get_fixture_server_status();
        assert_eq!(status_running.state, "running");
        let idempotent_start = dev_start_fixture_server().expect("idempotent start");
        assert_eq!(idempotent_start.state, "running");
        assert_eq!(idempotent_start.port, Some(8080));

        // Stop server
        let stop_res = dev_stop_fixture_server();
        assert!(stop_res.is_ok());
        let stopped = stop_res.unwrap();
        assert_eq!(stopped.state, "stopped");

        // Idempotent stop
        let stop_again = dev_stop_fixture_server().expect("idempotent stop");
        assert_eq!(stop_again.state, "stopped");

        let status_stopped = dev_get_fixture_server_status();
        assert_eq!(status_stopped.state, "stopped");
    }
}
