//! Unified component/model executable resolver and LGPLv3 §6 custom FFmpeg support
//! (ARCHITECTURE §9, §15; SECURITY §9; docs/compliance/FFMPEG.md §6).
//!
//! Responsibilities:
//! - Provide a single `ComponentResolver` returning the active executable path (managed versioned
//!   install or custom FFmpeg override) plus an RAII `VersionLease` to prevent mid-execution GC.
//! - Enforce strict path validation for `custom_ffmpeg_path`: absolute path on a local fixed drive,
//!   filename `ffmpeg.exe`, sibling `ffprobe.exe` in the same directory, no traversal/ADS/reserved names.
//! - Verify custom FFmpeg in `VerifyMode::Warn`, set a persistent warning flag, and log redacted
//!   diagnostics without ever bypassing SHA-256 integrity on any other sidecar (`yt-dlp`, `qjs`,
//!   `whisper-cli`, or models).
//! - Automatically fall back to the managed installed FFmpeg if the custom binary disappears.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use specta::Type;
use thiserror::Error;

use crate::capabilities::{
    verify_ffmpeg_capabilities, CapabilityError, CapabilityReport, CapabilityReportView,
    CapabilityStatus, VerifyMode,
};
use crate::installer::{quick_verify_row, ComponentMissingAdvice, InUseRegistry, VersionLease};
use crate::security;
use crate::sidecars::SidecarSupervisor;
use crate::store::{ComponentRow, LocalStore, StoreError};

pub const SETTING_CUSTOM_FFMPEG_PATH: &str = "custom_ffmpeg_path";
pub const SETTING_CUSTOM_FFMPEG_WARNING: &str = "custom_ffmpeg_warning";
pub const SETTING_CUSTOM_FFMPEG_REPORT: &str = "custom_ffmpeg_report_json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionSource {
    ManagedInstalled,
    CustomFfmpeg,
    FallbackFromMissingCustom,
}

/// Active resolved executable returned by `ComponentResolver`.
pub struct ResolvedExecutable {
    pub component: String,
    pub exe: String,
    pub version: String,
    pub path: PathBuf,
    pub source: ResolutionSource,
    pub warning: Option<String>,
    /// Held for the duration of a managed child process spawn so GC cannot remove `bin/<comp>/<ver>/`.
    pub _lease: Option<VersionLease>,
}

/// Active resolved speech model returned by `ComponentResolver`.
#[derive(Debug, Clone)]
pub struct ResolvedModel {
    pub model: String,
    pub version: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CustomFfmpegStatus {
    pub enabled: bool,
    pub configured_path: Option<String>,
    pub redacted_path: Option<String>,
    pub warning_flag: bool,
    pub warning_message: Option<String>,
    pub warnings: Vec<String>,
    pub file_present: bool,
    pub fallback_active: bool,
    pub capability_report: Option<CapabilityReportView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ComponentVersionView {
    pub version: String,
    pub kind: String,
    pub status: String,
    pub is_current: bool,
    pub is_previous: bool,
    pub manifest_sequence: f64,
    pub installed_at: f64,
    pub quarantine_reason: Option<String>,
    pub capability_report: Option<CapabilityReportView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ComponentStateSummary {
    pub component: String,
    pub kind: String,
    pub current_version: Option<String>,
    pub previous_version: Option<String>,
    pub active_source: String, // "managed" | "custom" | "fallback" | "missing"
    pub active_path: Option<String>,
    pub versions: Vec<ComponentVersionView>,
    pub missing_advice: Option<ComponentMissingAdvice>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ComponentDiagnosticsSnapshot {
    pub custom_ffmpeg: CustomFfmpegStatus,
    pub components: Vec<ComponentStateSummary>,
    pub pending_deferred_deletes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCustomFfmpeg {
    pub ffmpeg_exe: PathBuf,
    pub ffprobe_exe: PathBuf,
}

#[derive(Error, Debug)]
pub enum ResolverError {
    #[error("Component missing or broken: {}", .0.message)]
    Missing(ComponentMissingAdvice),

    #[error("Invalid custom FFmpeg path: {0}")]
    InvalidCustomPath(String),

    #[error("Custom FFmpeg verification failed: {0}")]
    CustomVerificationFailed(String),

    #[error("Security boundary violation: {0}")]
    Security(#[from] security::SecurityError),

    #[error("Database store error: {0}")]
    Store(#[from] StoreError),
}

impl ResolverError {
    pub fn error_code(&self) -> &'static str {
        match self {
            ResolverError::Missing(_) => "E_COMPONENT_MISSING",
            ResolverError::CustomVerificationFailed(_) => "E_VERIFY_FAILED",
            ResolverError::Security(se) => se.error_code(),
            _ => "E_VERIFY_FAILED",
        }
    }

    pub fn missing_advice(&self) -> Option<&ComponentMissingAdvice> {
        match self {
            ResolverError::Missing(a) => Some(a),
            _ => None,
        }
    }
}

fn validate_custom_ffmpeg_path_internal(
    raw_path: &str,
    fixed_drive_override: Option<bool>,
) -> Result<ValidatedCustomFfmpeg, ResolverError> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return Err(ResolverError::InvalidCustomPath(
            "Custom FFmpeg path must not be empty".to_string(),
        ));
    }

    if trimmed.contains('\0') {
        return Err(ResolverError::InvalidCustomPath(
            "Path contains NUL byte".to_string(),
        ));
    }

    // Reject UNC network paths (`\\server\share`) and device namespace prefixes (`\\?\`).
    if trimmed.starts_with(r"\\") || trimmed.starts_with("//") {
        return Err(ResolverError::InvalidCustomPath(
            "UNC/network paths are forbidden for custom FFmpeg; select a local fixed drive"
                .to_string(),
        ));
    }

    let path = Path::new(trimmed);
    if !path.is_absolute() {
        return Err(ResolverError::InvalidCustomPath(
            "Custom FFmpeg path must be an absolute path".to_string(),
        ));
    }

    // Reject NTFS Alternate Data Streams (`:`) anywhere after the drive letter `X:`.
    let bytes = trimmed.as_bytes();
    let after_drive = if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        &trimmed[2..]
    } else {
        trimmed
    };
    if after_drive.contains(':') {
        return Err(ResolverError::InvalidCustomPath(
            "Alternate data stream ':' is forbidden in custom FFmpeg path".to_string(),
        ));
    }

    for comp in path.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => {}
            Component::ParentDir | Component::CurDir => {
                return Err(ResolverError::InvalidCustomPath(
                    "Relative segments '.' or '..' are forbidden in custom FFmpeg path".to_string(),
                ));
            }
            Component::Normal(seg) => {
                let seg_str = seg.to_string_lossy();
                match security::check_windows_segment_invariants(&seg_str) {
                    Ok(()) => {}
                    // Defer 8.3 short-name rejection until after `dunce::canonicalize` resolves
                    // legitimate Windows OS short aliases (e.g. `C:\Users\RUNNER~1\...`).
                    Err(security::SecurityError::ShortNameForbidden(_)) => {}
                    Err(e) => {
                        return Err(ResolverError::InvalidCustomPath(format!(
                            "Unsafe path segment: {}",
                            e
                        )));
                    }
                }
            }
        }
    }

    let fixed_drive = match fixed_drive_override {
        Some(v) => v,
        None => security::is_local_fixed_drive(path)
            .map_err(|e| ResolverError::InvalidCustomPath(e.to_string()))?,
    };
    if !fixed_drive {
        return Err(ResolverError::InvalidCustomPath(
            "Custom FFmpeg must reside on a local fixed drive (network/removable drives are not allowed)"
                .to_string(),
        ));
    }

    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| ResolverError::InvalidCustomPath("Missing filename".to_string()))?;
    if !file_name.eq_ignore_ascii_case("ffmpeg.exe") {
        return Err(ResolverError::InvalidCustomPath(format!(
            "Custom FFmpeg file name must be 'ffmpeg.exe', got '{}'",
            file_name
        )));
    }

    if !path.is_file() {
        return Err(ResolverError::InvalidCustomPath(format!(
            "Custom ffmpeg.exe file does not exist: '{}'",
            security::redact_sensitive_text(trimmed)
        )));
    }

    let parent = path
        .parent()
        .ok_or_else(|| ResolverError::InvalidCustomPath("Missing parent directory".to_string()))?;
    let ffprobe_path = parent.join("ffprobe.exe");
    if !ffprobe_path.is_file() {
        return Err(ResolverError::InvalidCustomPath(
            "ffprobe.exe must exist in the same directory as custom ffmpeg.exe".to_string(),
        ));
    }

    // Re-check canonical destinations stay on a local fixed drive and contain no unsafe
    // canonical segments (including real `~N` directory names after Windows alias expansion).
    let canon_ffmpeg = dunce::canonicalize(path)
        .map_err(|e| ResolverError::InvalidCustomPath(format!("Canonicalize failed: {}", e)))?;
    let canon_ffprobe = dunce::canonicalize(&ffprobe_path).map_err(|e| {
        ResolverError::InvalidCustomPath(format!("Canonicalize ffprobe failed: {}", e))
    })?;

    for comp in canon_ffmpeg.components() {
        if let Component::Normal(seg) = comp {
            let seg_str = seg.to_string_lossy();
            security::check_windows_segment_invariants(&seg_str).map_err(|e| {
                ResolverError::InvalidCustomPath(format!("Unsafe canonical path segment: {}", e))
            })?;
        }
    }

    if fixed_drive_override.is_none() {
        let canon_fixed = security::is_local_fixed_drive(&canon_ffmpeg)
            .map_err(|e| ResolverError::InvalidCustomPath(e.to_string()))?;
        if !canon_fixed {
            return Err(ResolverError::InvalidCustomPath(
                "Resolved canonical target is not on a local fixed drive".to_string(),
            ));
        }
    }

    Ok(ValidatedCustomFfmpeg {
        ffmpeg_exe: canon_ffmpeg,
        ffprobe_exe: canon_ffprobe,
    })
}

/// Validates a user-provided `custom_ffmpeg_path`:
/// absolute path, local fixed drive, filename `ffmpeg.exe`, sibling `ffprobe.exe` present.
pub fn validate_custom_ffmpeg_path(raw_path: &str) -> Result<ValidatedCustomFfmpeg, ResolverError> {
    validate_custom_ffmpeg_path_internal(raw_path, None)
}

/// Single authoritative runtime resolver returning the active executable (managed or custom FFmpeg)
/// or active speech model (`models/<file>`).
#[derive(Clone)]
pub struct ComponentResolver {
    ws_root: PathBuf,
    store: LocalStore,
    in_use: InUseRegistry,
}

impl ComponentResolver {
    pub fn new(ws_root: PathBuf, store: LocalStore, in_use: InUseRegistry) -> Self {
        Self {
            ws_root,
            store,
            in_use,
        }
    }

    /// Configures and verifies `custom_ffmpeg_path` using WARN mode via production `SidecarSupervisor`.
    pub async fn set_custom_ffmpeg_path(
        &self,
        raw_path: &str,
    ) -> Result<CustomFfmpegStatus, ResolverError> {
        let supervisor = SidecarSupervisor::new();
        let ws_root = self.ws_root.clone();
        self.set_custom_ffmpeg_path_with_verifier(raw_path, |ffmpeg, ffprobe| async move {
            verify_ffmpeg_capabilities(
                &ffmpeg,
                &ffprobe,
                "custom",
                VerifyMode::Warn,
                &supervisor,
                &ws_root,
            )
            .await
        })
        .await
    }

    /// Testable variant allowing injected capability verifier for offline CI determinism.
    pub async fn set_custom_ffmpeg_path_with_verifier<F, Fut>(
        &self,
        raw_path: &str,
        verifier: F,
    ) -> Result<CustomFfmpegStatus, ResolverError>
    where
        F: FnOnce(PathBuf, PathBuf) -> Fut,
        Fut: std::future::Future<Output = Result<CapabilityReport, CapabilityError>>,
    {
        let validated = validate_custom_ffmpeg_path(raw_path)?;
        let report = verifier(validated.ffmpeg_exe.clone(), validated.ffprobe_exe.clone())
            .await
            .map_err(|e| ResolverError::CustomVerificationFailed(e.to_string()))?;

        let stored_path = validated.ffmpeg_exe.to_string_lossy().to_string();
        let report_json = serde_json::to_string(&report)
            .map_err(|e| ResolverError::InvalidCustomPath(e.to_string()))?;

        self.store
            .set_setting(SETTING_CUSTOM_FFMPEG_PATH, &stored_path)?;
        self.store
            .set_setting(SETTING_CUSTOM_FFMPEG_WARNING, "true")?;
        self.store
            .set_setting(SETTING_CUSTOM_FFMPEG_REPORT, &report_json)?;

        tracing::warn!(
            "Custom FFmpeg override enabled at '{}' (license={:?}, warnings={:?}). SHA-256 integrity checks bypassed for FFmpeg/ffprobe only.",
            security::redact_sensitive_text(&stored_path),
            report.ffmpeg.as_ref().map(|f| &f.effective_license),
            report.warnings
        );

        self.get_custom_ffmpeg_status()
    }

    /// Clears `custom_ffmpeg_path` and its persistent warning flag, returning to managed FFmpeg.
    pub fn clear_custom_ffmpeg_path(&self) -> Result<CustomFfmpegStatus, ResolverError> {
        self.store.delete_setting(SETTING_CUSTOM_FFMPEG_PATH)?;
        self.store.delete_setting(SETTING_CUSTOM_FFMPEG_WARNING)?;
        self.store.delete_setting(SETTING_CUSTOM_FFMPEG_REPORT)?;
        tracing::info!(
            "Custom FFmpeg override cleared; managed FFmpeg integrity enforcement restored"
        );
        self.get_custom_ffmpeg_status()
    }

    /// Returns the persisted custom FFmpeg configuration, live file presence check, fallback state,
    /// and capability report.
    pub fn get_custom_ffmpeg_status(&self) -> Result<CustomFfmpegStatus, ResolverError> {
        let configured = self
            .store
            .get_setting(SETTING_CUSTOM_FFMPEG_PATH)?
            .filter(|s| !s.trim().is_empty());
        let warning_flag = self
            .store
            .get_setting(SETTING_CUSTOM_FFMPEG_WARNING)?
            .as_deref()
            == Some("true");
        let report_opt = self
            .store
            .get_setting(SETTING_CUSTOM_FFMPEG_REPORT)?
            .and_then(|s| serde_json::from_str::<CapabilityReport>(&s).ok());

        let Some(cfg_path) = configured else {
            return Ok(CustomFfmpegStatus {
                enabled: false,
                configured_path: None,
                redacted_path: None,
                warning_flag: false,
                warning_message: None,
                warnings: vec![],
                file_present: false,
                fallback_active: false,
                capability_report: None,
            });
        };

        let validated = validate_custom_ffmpeg_path(&cfg_path);
        let file_present = validated.is_ok();
        let fallback_active = !file_present;

        let mut warnings = vec![
            "Peringatan: FFmpeg kustom sedang aktif. Validasi integritas SHA-256 dinonaktifkan khusus untuk FFmpeg/ffprobe.".to_string(),
        ];
        if let Some(ref r) = report_opt {
            for w in &r.warnings {
                warnings.push(w.clone());
            }
        }
        if fallback_active {
            warnings.push(
                "Custom FFmpeg binary is missing on disk; falling back to managed installed FFmpeg."
                    .to_string(),
            );
        }

        let warning_message = Some(warnings.join(" "));
        let redacted = security::redact_sensitive_text(&cfg_path);

        Ok(CustomFfmpegStatus {
            enabled: true,
            configured_path: Some(cfg_path),
            redacted_path: Some(redacted),
            warning_flag: warning_flag || true,
            warning_message,
            warnings,
            file_present,
            fallback_active,
            capability_report: report_opt.as_ref().map(CapabilityReportView::from),
        })
    }

    /// Resolves the active executable for `(component, exe)`.
    ///
    /// Rules:
    /// - Strictly allowlisted `(component, exe)` pairs only.
    /// - Custom FFmpeg applies **only** when `component == "ffmpeg"` (`ffmpeg.exe` / `ffprobe.exe`),
    ///   never to `yt-dlp`, `qjs`, or `whisper-cli`.
    /// - If custom FFmpeg is configured but vanished on disk, falls back to the managed installed
    ///   version and logs a warning.
    /// - Verifies managed files exist with expected sizes; if vanished (e.g. antivirus), returns
    ///   `ResolverError::Missing` (`E_COMPONENT_MISSING`) mentioning antivirus + Repair.
    pub fn resolve_executable(
        &self,
        component: &str,
        exe: &str,
    ) -> Result<ResolvedExecutable, ResolverError> {
        if !security::is_valid_component_and_exe(component, exe) {
            return Err(ResolverError::Security(
                security::SecurityError::ComponentNotAllowlisted(format!("{}/{}", component, exe)),
            ));
        }

        let mut custom_fallback_reason: Option<String> = None;

        if component == "ffmpeg" {
            let custom_status = self.get_custom_ffmpeg_status()?;
            if custom_status.enabled {
                if let Some(ref cfg_path) = custom_status.configured_path {
                    match validate_custom_ffmpeg_path(cfg_path) {
                        Ok(valid) => {
                            let target = if exe.eq_ignore_ascii_case("ffprobe.exe") {
                                valid.ffprobe_exe
                            } else {
                                valid.ffmpeg_exe
                            };
                            tracing::warn!(
                                "Using custom FFmpeg override for {} (redacted='{}')",
                                exe,
                                custom_status.redacted_path.as_deref().unwrap_or("<custom>")
                            );
                            return Ok(ResolvedExecutable {
                                component: component.to_string(),
                                exe: exe.to_string(),
                                version: "custom".to_string(),
                                path: target,
                                source: ResolutionSource::CustomFfmpeg,
                                warning: custom_status.warning_message,
                                _lease: None,
                            });
                        }
                        Err(err) => {
                            let redacted =
                                custom_status.redacted_path.as_deref().unwrap_or("<custom>");
                            tracing::warn!(
                                "Custom FFmpeg at '{}' is unavailable ({}); falling back to managed installed FFmpeg",
                                redacted,
                                err
                            );
                            custom_fallback_reason = Some(format!(
                                "Custom FFmpeg unavailable ({}); using managed installed FFmpeg",
                                err
                            ));
                        }
                    }
                }
            }
        }

        let row = self.store.get_current_component(component)?.ok_or_else(|| {
            ResolverError::Missing(ComponentMissingAdvice {
                code: "E_COMPONENT_MISSING".to_string(),
                component: component.to_string(),
                version: None,
                message: format!(
                    "Component '{}' is not installed or vanished (antivirus software may have quarantined files). Click Repair to download/reinstall.",
                    component
                ),
                mentions_antivirus: true,
                repair_action: true,
            })
        })?;

        if let Err(reason) = quick_verify_row(&self.ws_root, &row) {
            tracing::error!(
                "Resolved component {} ({}) failed quick check: {}",
                component,
                crate::manifest::format_component_version(&row.version),
                reason
            );
            return Err(ResolverError::Missing(ComponentMissingAdvice {
                code: "E_COMPONENT_MISSING".to_string(),
                component: component.to_string(),
                version: Some(row.version),
                message: format!(
                    "Active {} files missing or damaged ({}). Antivirus software may have removed or quarantined an executable. Click Repair to re-download.",
                    component, reason
                ),
                mentions_antivirus: true,
                repair_action: true,
            }));
        }

        let path = security::resolve_component_path_with_root(
            &self.ws_root,
            component,
            &row.version,
            exe,
        )?;
        if !path.is_file() {
            return Err(ResolverError::Missing(ComponentMissingAdvice {
                code: "E_COMPONENT_MISSING".to_string(),
                component: component.to_string(),
                version: Some(row.version),
                message: format!(
                    "Executable '{}' missing for {} (antivirus software may have removed it). Click Repair to re-download.",
                    exe, component
                ),
                mentions_antivirus: true,
                repair_action: true,
            }));
        }

        let lease = self.in_use.acquire(component, &row.version);
        let source = if custom_fallback_reason.is_some() {
            ResolutionSource::FallbackFromMissingCustom
        } else {
            ResolutionSource::ManagedInstalled
        };

        Ok(ResolvedExecutable {
            component: component.to_string(),
            exe: exe.to_string(),
            version: row.version,
            path,
            source,
            warning: custom_fallback_reason,
            _lease: Some(lease),
        })
    }

    /// Resolves a verified active speech model under `models/<file>`.
    pub fn resolve_model(&self, model_filename: &str) -> Result<ResolvedModel, ResolverError> {
        let target = security::resolve_model_path_with_root(&self.ws_root, model_filename)?;
        let row = self.store.get_current_component(model_filename)?.ok_or_else(|| {
            ResolverError::Missing(ComponentMissingAdvice {
                code: "E_COMPONENT_MISSING".to_string(),
                component: model_filename.to_string(),
                version: None,
                message: format!(
                    "Model '{}' is not installed or missing (antivirus may have quarantined files). Click Repair to download.",
                    model_filename
                ),
                mentions_antivirus: true,
                repair_action: true,
            })
        })?;

        if let Err(reason) = quick_verify_row(&self.ws_root, &row) {
            return Err(ResolverError::Missing(ComponentMissingAdvice {
                code: "E_COMPONENT_MISSING".to_string(),
                component: model_filename.to_string(),
                version: Some(row.version),
                message: format!(
                    "Model '{}' failed integrity quick-check ({}). Antivirus may have modified/removed it. Click Repair to re-download.",
                    model_filename, reason
                ),
                mentions_antivirus: true,
                repair_action: true,
            }));
        }

        Ok(ResolvedModel {
            model: model_filename.to_string(),
            version: row.version,
            path: target,
        })
    }

    /// Collects full diagnostics snapshot across all installed/quarantined components and custom FFmpeg.
    pub fn diagnostics_snapshot(&self) -> Result<ComponentDiagnosticsSnapshot, ResolverError> {
        let custom_ffmpeg = self.get_custom_ffmpeg_status()?;
        if custom_ffmpeg.enabled && custom_ffmpeg.warning_flag {
            tracing::warn!(
                "Diagnostics snapshot: custom FFmpeg override active (redacted='{}', fallback={})",
                custom_ffmpeg.redacted_path.as_deref().unwrap_or("<none>"),
                custom_ffmpeg.fallback_active
            );
        }

        let all_rows = self.store.list_all_components()?;
        let mut by_comp: std::collections::BTreeMap<String, Vec<ComponentRow>> =
            std::collections::BTreeMap::new();
        for r in all_rows {
            by_comp.entry(r.component.clone()).or_default().push(r);
        }

        let mut components = Vec::new();
        for (comp_name, rows) in by_comp {
            let kind = rows
                .first()
                .map(|r| r.kind.clone())
                .unwrap_or_else(|| "bin".to_string());
            let current_version = rows
                .iter()
                .find(|r| r.is_current && r.status == "installed")
                .map(|r| r.version.clone());
            let previous_version = rows
                .iter()
                .find(|r| r.is_previous && r.status == "installed")
                .map(|r| r.version.clone());

            let (active_source, active_path, missing_advice) = if kind == "bin" {
                if let Some(exe) = crate::installer::primary_executable_for_component(&comp_name) {
                    match self.resolve_executable(&comp_name, exe) {
                        Ok(resolved) => {
                            let src = match resolved.source {
                                ResolutionSource::ManagedInstalled => "managed",
                                ResolutionSource::CustomFfmpeg => "custom",
                                ResolutionSource::FallbackFromMissingCustom => "fallback",
                            };
                            (
                                src.to_string(),
                                Some(security::redact_sensitive_text(
                                    &resolved.path.to_string_lossy(),
                                )),
                                None,
                            )
                        }
                        Err(e) => ("missing".to_string(), None, e.missing_advice().cloned()),
                    }
                } else {
                    ("missing".to_string(), None, None)
                }
            } else {
                match self.resolve_model(&comp_name) {
                    Ok(m) => (
                        "managed".to_string(),
                        Some(security::redact_sensitive_text(&m.path.to_string_lossy())),
                        None,
                    ),
                    Err(e) => ("missing".to_string(), None, e.missing_advice().cloned()),
                }
            };

            let versions = rows
                .into_iter()
                .map(|r| {
                    let cap = r
                        .capability_report_json
                        .as_deref()
                        .and_then(|s| serde_json::from_str::<CapabilityReport>(s).ok())
                        .as_ref()
                        .map(CapabilityReportView::from);
                    ComponentVersionView {
                        version: r.version,
                        kind: r.kind,
                        status: r.status,
                        is_current: r.is_current,
                        is_previous: r.is_previous,
                        manifest_sequence: r.manifest_sequence as f64,
                        installed_at: r.installed_at as f64,
                        quarantine_reason: r.quarantine_reason,
                        capability_report: cap,
                    }
                })
                .collect();

            components.push(ComponentStateSummary {
                component: comp_name,
                kind,
                current_version,
                previous_version,
                active_source,
                active_path,
                versions,
                missing_advice,
            });
        }

        let pending_deferred_deletes = self.store.list_deferred_deletes()?.len() as u32;

        Ok(ComponentDiagnosticsSnapshot {
            custom_ffmpeg,
            components,
            pending_deferred_deletes,
        })
    }

    pub fn format_custom_status_for_capability(status: CapabilityStatus) -> &'static str {
        match status {
            CapabilityStatus::Pass => "pass",
            CapabilityStatus::Warn => "warn",
            CapabilityStatus::Fail => "fail",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::ArchiveLimits;
    use crate::installer::install_verified_artifact_with_options;
    use crate::manifest::{ComponentManifestEntry, ManifestFileEntry};
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    fn sha256_bytes(data: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(data);
        hex::encode(h.finalize())
    }

    fn setup_resolver() -> (
        TempDir,
        PathBuf,
        LocalStore,
        InUseRegistry,
        ComponentResolver,
    ) {
        let tmp = TempDir::new().unwrap();
        let ws = security::ensure_workspace_with_root(tmp.path()).unwrap();
        let store = LocalStore::open(&ws.join("clipper.db")).unwrap();
        let in_use = InUseRegistry::new();
        let resolver = ComponentResolver::new(ws.clone(), store.clone(), in_use.clone());
        (tmp, ws, store, in_use, resolver)
    }

    async fn install_dummy_bin(
        tmp: &Path,
        ws: &Path,
        store: &LocalStore,
        in_use: &InUseRegistry,
        comp: &str,
        ver: &str,
        files: Vec<(&str, &[u8])>,
    ) {
        let mut buf = std::io::Cursor::new(Vec::new());
        let mut mf = Vec::new();
        {
            let mut w = ZipWriter::new(&mut buf);
            for (name, data) in files {
                w.start_file(name, SimpleFileOptions::default()).unwrap();
                w.write_all(data).unwrap();
                mf.push(ManifestFileEntry {
                    path: name.to_string(),
                    sha256: sha256_bytes(data),
                    size_bytes: data.len() as u64,
                });
            }
            w.finish().unwrap();
        }
        let zbytes = buf.into_inner();
        let pkg = tmp.join(format!("{}-{}.zip", comp, ver));
        fs::write(&pkg, &zbytes).unwrap();
        let entry = ComponentManifestEntry {
            component: comp.to_string(),
            version: ver.to_string(),
            platform: "windows-x86_64".to_string(),
            url: "https://cdn.clipperhelper.com/pkg.zip".to_string(),
            mirrors: vec![],
            size_bytes: zbytes.len() as u64,
            sha256: sha256_bytes(&zbytes),
            archive: "zip".to_string(),
            files: mf,
            min_app_version: "0.1.0".to_string(),
            max_app_version: None,
            rollout_percent: 100,
        };
        install_verified_artifact_with_options(
            ws,
            store,
            in_use,
            &entry,
            1,
            &pkg,
            &ArchiveLimits::default(),
            None,
            Some(|c, _d, v| async move {
                Ok(CapabilityReport {
                    component: c,
                    version: v,
                    mode: "strict".to_string(),
                    status: CapabilityStatus::Pass,
                    checked_at: 1000,
                    reported_version: Some("1.0.0".to_string()),
                    reasons: vec![],
                    warnings: vec![],
                    ffmpeg: None,
                })
            }),
        )
        .await
        .unwrap();
    }

    #[test]
    fn test_validate_custom_ffmpeg_path_enforces_security_and_sibling_ffprobe() {
        let tmp = TempDir::new().unwrap();
        let custom_dir = tmp.path().join("custom_ff");
        fs::create_dir_all(&custom_dir).unwrap();
        let ffmpeg_exe = custom_dir.join("ffmpeg.exe");
        let ffprobe_exe = custom_dir.join("ffprobe.exe");
        let other_exe = custom_dir.join("my_ffmpeg.exe");

        fs::write(&ffmpeg_exe, b"ff").unwrap();
        fs::write(&other_exe, b"ff").unwrap();

        // Missing sibling ffprobe.exe -> rejected
        let err_probe = validate_custom_ffmpeg_path(&ffmpeg_exe.to_string_lossy()).unwrap_err();
        assert!(matches!(err_probe, ResolverError::InvalidCustomPath(_)));

        // Wrong filename -> rejected
        fs::write(&ffprobe_exe, b"fp").unwrap();
        let err_name = validate_custom_ffmpeg_path(&other_exe.to_string_lossy()).unwrap_err();
        assert!(matches!(err_name, ResolverError::InvalidCustomPath(_)));

        // UNC / relative / ADS / traversal / non-fixed drive -> rejected
        assert!(validate_custom_ffmpeg_path(r"\\server\share\ffmpeg.exe").is_err());
        assert!(validate_custom_ffmpeg_path(r"relative\ffmpeg.exe").is_err());
        assert!(validate_custom_ffmpeg_path(&format!("{}::$DATA", ffmpeg_exe.display())).is_err());
        assert!(validate_custom_ffmpeg_path(&format!(
            "{}\\..\\custom_ff\\ffmpeg.exe",
            custom_dir.display()
        ))
        .is_err());
        assert!(
            validate_custom_ffmpeg_path_internal(&ffmpeg_exe.to_string_lossy(), Some(false))
                .is_err()
        );

        // Valid absolute fixed-drive path with both ffmpeg.exe and ffprobe.exe -> accepted!
        let valid = validate_custom_ffmpeg_path(&ffmpeg_exe.to_string_lossy()).unwrap();
        assert!(valid.ffmpeg_exe.ends_with("ffmpeg.exe"));
        assert!(valid.ffprobe_exe.ends_with("ffprobe.exe"));

        // Literal `~1` directory segment whose canonical name itself is an 8.3 form -> rejected!
        let literal_short_dir = tmp.path().join("SHORT~1");
        fs::create_dir_all(&literal_short_dir).unwrap();
        fs::write(literal_short_dir.join("ffmpeg.exe"), b"ff").unwrap();
        fs::write(literal_short_dir.join("ffprobe.exe"), b"fp").unwrap();
        assert!(validate_custom_ffmpeg_path(
            &literal_short_dir.join("ffmpeg.exe").to_string_lossy()
        )
        .is_err());
    }

    #[tokio::test]
    async fn test_custom_ffmpeg_warn_mode_isolation_and_fallback_to_installed() {
        let (tmp, ws, store, in_use, resolver) = setup_resolver();

        // Install managed FFmpeg + yt-dlp
        install_dummy_bin(
            tmp.path(),
            &ws,
            &store,
            &in_use,
            "ffmpeg",
            "n9.0.2",
            vec![
                ("ffmpeg.exe", b"managed-ff"),
                ("ffprobe.exe", b"managed-fp"),
            ],
        )
        .await;
        install_dummy_bin(
            tmp.path(),
            &ws,
            &store,
            &in_use,
            "yt-dlp",
            "2026.10.26",
            vec![("yt-dlp.exe", b"managed-yt")],
        )
        .await;

        // Before custom override: resolver returns ManagedInstalled
        let res_managed = resolver.resolve_executable("ffmpeg", "ffmpeg.exe").unwrap();
        assert_eq!(res_managed.source, ResolutionSource::ManagedInstalled);
        assert_eq!(res_managed.version, "n9.0.2");

        // Prepare custom GPL FFmpeg folder
        let custom_dir = tmp.path().join("user_gpl_ffmpeg");
        fs::create_dir_all(&custom_dir).unwrap();
        let custom_ff = custom_dir.join("ffmpeg.exe");
        let custom_fp = custom_dir.join("ffprobe.exe");
        fs::write(&custom_ff, b"custom-gpl-ff").unwrap();
        fs::write(&custom_fp, b"custom-gpl-fp").unwrap();

        // Set custom FFmpeg via WARN verifier (simulates GPL build allowed with warning flag)
        let status = resolver
            .set_custom_ffmpeg_path_with_verifier(&custom_ff.to_string_lossy(), |_, _| async {
                Ok(CapabilityReport {
                    component: "ffmpeg".to_string(),
                    version: "custom".to_string(),
                    mode: "warn".to_string(),
                    status: CapabilityStatus::Warn,
                    checked_at: 2000,
                    reported_version: Some("9.0.2-gpl".to_string()),
                    reasons: vec![],
                    warnings: vec!["--enable-gpl flag detected in custom build".to_string()],
                    ffmpeg: None,
                })
            })
            .await
            .expect("custom ffmpeg accepted in warn mode");

        assert!(status.enabled);
        assert!(status.warning_flag);
        assert!(!status.fallback_active);
        assert!(status.warnings.iter().any(|w| w.contains("--enable-gpl")));

        // Verify persisted warning flag in SQLite settings
        assert_eq!(
            store
                .get_setting(SETTING_CUSTOM_FFMPEG_WARNING)
                .unwrap()
                .as_deref(),
            Some("true")
        );

        // ComponentResolver returns CustomFfmpeg for BOTH ffmpeg.exe and ffprobe.exe
        let res_custom_ff = resolver.resolve_executable("ffmpeg", "ffmpeg.exe").unwrap();
        assert_eq!(res_custom_ff.source, ResolutionSource::CustomFfmpeg);
        assert_eq!(res_custom_ff.version, "custom");
        assert!(res_custom_ff.warning.is_some());

        let res_custom_fp = resolver
            .resolve_executable("ffmpeg", "ffprobe.exe")
            .unwrap();
        assert_eq!(res_custom_fp.source, ResolutionSource::CustomFfmpeg);
        assert_eq!(res_custom_fp.path, dunce::canonicalize(&custom_fp).unwrap());

        // Custom FFmpeg NEVER affects other sidecars (yt-dlp stays ManagedInstalled)
        let res_yt = resolver.resolve_executable("yt-dlp", "yt-dlp.exe").unwrap();
        assert_eq!(res_yt.source, ResolutionSource::ManagedInstalled);
        assert_eq!(res_yt.version, "2026.10.26");

        // Diagnostics exposes warning flag & capability report cleanly
        let diag = resolver.diagnostics_snapshot().unwrap();
        assert!(diag.custom_ffmpeg.warning_flag);
        assert!(!diag.custom_ffmpeg.fallback_active);
        assert!(diag.custom_ffmpeg.capability_report.is_some());

        // Delete custom ffmpeg.exe -> resolver MUST fall back automatically to managed installed FFmpeg!
        fs::remove_file(&custom_ff).unwrap();
        let res_fallback = resolver.resolve_executable("ffmpeg", "ffmpeg.exe").unwrap();
        assert_eq!(
            res_fallback.source,
            ResolutionSource::FallbackFromMissingCustom
        );
        assert_eq!(res_fallback.version, "n9.0.2");
        assert!(res_fallback.warning.is_some());

        let diag_after = resolver.diagnostics_snapshot().unwrap();
        assert!(diag_after.custom_ffmpeg.fallback_active);

        // Clear override restores clean ManagedInstalled state
        let cleared = resolver.clear_custom_ffmpeg_path().unwrap();
        assert!(!cleared.enabled);
        assert!(!cleared.warning_flag);
        let res_clean = resolver.resolve_executable("ffmpeg", "ffmpeg.exe").unwrap();
        assert_eq!(res_clean.source, ResolutionSource::ManagedInstalled);
    }

    #[tokio::test]
    async fn test_missing_component_file_returns_e_component_missing_mentioning_antivirus() {
        let (tmp, ws, store, in_use, resolver) = setup_resolver();
        install_dummy_bin(
            tmp.path(),
            &ws,
            &store,
            &in_use,
            "qjs",
            "v0.18.0",
            vec![("qjs.exe", b"qjs-bytes")],
        )
        .await;

        // Simulate antivirus deleting qjs.exe after install
        fs::remove_file(ws.join("bin/qjs/v0.18.0/qjs.exe")).unwrap();

        let err = resolver
            .resolve_executable("qjs", "qjs.exe")
            .err()
            .expect("must fail when file vanished");
        assert_eq!(err.error_code(), "E_COMPONENT_MISSING");
        let advice = err.missing_advice().expect("advice");
        assert!(advice.mentions_antivirus);
        assert!(advice.repair_action);
        assert!(advice.message.to_lowercase().contains("antivirus"));
    }
}
