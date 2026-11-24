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

use crate::commands::WorkspaceStatus;
use crate::sidecars::{SidecarSpec, SidecarSupervisor};
use crate::store::LocalStore;
use crate::WorkspaceState;

static DEV_SERVER_PROCESS: Mutex<Option<Child>> = Mutex::new(None);

pub const DEV_EXTRA_KEYS: &[crate::manifest::ManifestPublicKey] =
    &[crate::manifest::ManifestPublicKey {
        kid: "clipper-manifest-dev",
        public_key_hex: "aaaf48f0cbd01c1f79573f139a5b0401b488ac44e0b103d64dea533b075c997b",
    }];

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct DevManifestComponent {
    pub component: String,
    pub version: String,
    pub platform: String,
    pub url: String,
    pub size_bytes: f64,
    pub sha256: String,
    pub archive: String,
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

    for sub in &["cache", "projects", "logs"] {
        let dir = ws.root.join(sub);
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    crate::security::ensure_workspace_with_root(&ws.root).map_err(|e| e.to_string())?;

    if let Some(store) = app.try_state::<LocalStore>() {
        store.reset_database().map_err(|e| e.to_string())?;
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

/// Debug-only: starts or verifies the local dev-server serving test fixtures.
#[tauri::command]
#[specta::specta]
pub fn dev_start_fixture_server() -> Result<String, String> {
    let mut guard = DEV_SERVER_PROCESS.lock().map_err(|e| e.to_string())?;
    if let Some(ref mut child) = *guard {
        if let Ok(None) = child.try_wait() {
            return Ok("http://127.0.0.1:8080".to_string());
        }
    }

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root_dir = manifest_dir.join("../../../tools/dev-fixtures");
    let script_path = manifest_dir.join("../../../tools/src/dev-server.js");

    // If dev-fixtures does not exist or missing manifest, generate them
    if !root_dir.join("manifest.json").exists() {
        let fixture_gen_script = manifest_dir.join("../../../tools/src/create-dev-fixtures.js");
        let _ = Command::new("node").arg(fixture_gen_script).status();
    }

    let mut child = Command::new("node")
        .arg(script_path)
        .arg("--root")
        .arg(&root_dir)
        .arg("--port")
        .arg("8080")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("Failed to spawn dev-server: {}", e))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to capture dev-server stdout".to_string())?;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;

    let prefix = "SERVER_READY: ";
    if !line.starts_with(prefix) {
        return Err(format!("Unexpected server output: {}", line));
    }
    let url = line[prefix.len()..].trim().to_string();

    *guard = Some(child);
    Ok(url)
}

/// Debug-only: fetches and cryptographically verifies the component manifest.
#[tauri::command]
#[specta::specta]
pub async fn dev_fetch_manifest(
    app: tauri::AppHandle,
    manifest_url: String,
    sig_url: String,
) -> Result<DevManifestInfo, String> {
    let ws = app
        .try_state::<WorkspaceState>()
        .ok_or_else(|| "Workspace state not available".to_string())?;
    let store = app
        .try_state::<LocalStore>()
        .ok_or_else(|| "LocalStore not available".to_string())?;

    tracing::info!(
        "dev_fetch_manifest: fetching from {} and {}",
        manifest_url,
        sig_url
    );
    let manifest = match crate::updater::fetch_and_verify_manifest(
        &manifest_url,
        &sig_url,
        "0.1.0",
        &store,
        Some(DEV_EXTRA_KEYS),
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
            return Err(format!("Manifest verification failed: {}", e));
        }
    };

    let mut dev_components = Vec::new();
    for c in manifest.components {
        let is_downloaded =
            if c.component.ends_with(".bin") || crate::security::is_allowed_model(&c.component) {
                crate::security::resolve_model_path_with_root(&ws.root, &c.component)
                    .map(|p| p.exists())
                    .unwrap_or(false)
            } else {
                let exe = format!("{}.exe", c.component);
                crate::security::resolve_component_path_with_root(
                    &ws.root,
                    &c.component,
                    &c.version,
                    &exe,
                )
                .map(|p| p.exists())
                .unwrap_or(false)
            };

        dev_components.push(DevManifestComponent {
            component: c.component,
            version: c.version,
            platform: c.platform,
            url: c.url,
            size_bytes: c.size_bytes as f64,
            sha256: c.sha256,
            archive: c.archive,
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

/// Debug-only: downloads and verifies a component from the manifest with live progress.
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
    let ws = app
        .try_state::<WorkspaceState>()
        .ok_or_else(|| "Workspace state not available".to_string())?;

    let dest_path = if component.ends_with(".bin") || crate::security::is_allowed_model(&component)
    {
        crate::security::resolve_model_path_with_root(&ws.root, &component)
            .map_err(|e| e.to_string())?
    } else {
        let exe = format!("{}.exe", component);
        crate::security::resolve_component_path_with_root(&ws.root, &component, &version, &exe)
            .map_err(|e| e.to_string())?
    };

    let entry = crate::manifest::ComponentManifestEntry {
        component: component.clone(),
        version,
        platform: "windows-x86_64".to_string(),
        url,
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
