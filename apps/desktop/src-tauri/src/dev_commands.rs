//! Debug developer commands (gated behind cfg(debug_assertions)).
//!
//! NEVER included in release builds. Used by the frontend debug panel.

#![cfg(debug_assertions)]

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::sidecars::{SidecarSpec, SidecarSupervisor};
use crate::store::LocalStore;

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct DevSystemInfo {
    pub schema_version: u32,
    pub workspace_root: String,
    pub platform: String,
    pub arch: String,
}

#[derive(Serialize, Deserialize, Type, Debug, Clone)]
pub struct DevSidecarRunResult {
    pub exit_code: i32,
    pub lines: Vec<String>,
    pub stderr_tail: String,
    pub elapsed_ms: u64,
}

/// Locate the fake-sidecar executable in the target directory.
fn find_fake_sidecar_binary() -> Result<PathBuf, String> {
    let current_exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // When running under tauri dev, current_exe is in target\debug\
    let target_dir = if current_exe
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n == "deps")
    {
        current_exe
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| "Failed to find target debug directory".to_string())?
    } else {
        current_exe
            .parent()
            .ok_or_else(|| "Failed to find parent directory of executable".to_string())?
    };

    let bin_path = target_dir.join("fake-sidecar.exe");
    if !bin_path.exists() {
        return Err(format!(
            "fake-sidecar binary not found at {:?}. Build it with `cargo build -p fake-sidecar`.",
            bin_path
        ));
    }

    Ok(bin_path)
}

/// Returns internal system information for the dev panel.
#[tauri::command]
#[specta::specta]
pub fn dev_get_system_info() -> Result<DevSystemInfo, String> {
    let workspace_root = crate::security::resolve_workspace_path("cache/probe.tmp")
        .map_err(|e| e.to_string())?
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new(r"C:\"))
        .to_path_buf();

    let db_path = workspace_root.join("dev_clipper.db");
    let store = LocalStore::open(&db_path).map_err(|e| e.to_string())?;
    let schema_version = store.schema_version().map_err(|e| e.to_string())?;

    Ok(DevSystemInfo {
        schema_version,
        workspace_root: workspace_root.to_string_lossy().to_string(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    })
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
            elapsed_ms: start.elapsed().as_millis() as u64,
        }),
        Err(crate::sidecars::SidecarError::ExecutionFailed {
            exit_code,
            stderr_tail,
        }) => Ok(DevSidecarRunResult {
            exit_code: exit_code.unwrap_or(-1),
            lines: Vec::new(),
            stderr_tail,
            elapsed_ms: start.elapsed().as_millis() as u64,
        }),
        Err(e) => Err(e.to_string()),
    }
}
