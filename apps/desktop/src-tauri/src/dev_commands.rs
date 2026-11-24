//! Debug developer commands (gated behind cfg(debug_assertions)).
//!
//! NEVER included in release builds. Used by the frontend debug panel.

#![cfg(debug_assertions)]

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::Manager;

use crate::commands::WorkspaceStatus;
use crate::sidecars::{SidecarSpec, SidecarSupervisor};
use crate::store::LocalStore;
use crate::WorkspaceState;

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
