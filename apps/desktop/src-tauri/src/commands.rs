//! Thin Tauri commands layer (ARCHITECTURE §4).
//!
//! Exposes typed Tauri IPC commands to the frontend. Validates inputs,
//! enforces authorization/invariants, and delegates business logic to internal services.

use serde::{Deserialize, Serialize};
use specta::Type;

/// Application metadata returned to the frontend.
#[derive(Serialize, Deserialize, Type, Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub platform: String,
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
}
