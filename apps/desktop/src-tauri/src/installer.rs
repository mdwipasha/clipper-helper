//! Atomic versioned component & model installer, GC, deferred cleanup, and startup
//! reconciliation (ARCHITECTURE §9, §13; SECURITY §9–§10; PROGRESS_AND_ETA §8).
//!
//! Layout:
//! - Managed binary components live in immutable folders `bin/<component>/<version>/` with
//!   `install.json` (`InstallReceipt`).
//! - Managed speech models live in `models/<file>` with companion `models/.<file>.install.json`.
//! - SQLite (`components` + `deferred_deletes`) is authoritative for `current` / `previous` /
//!   `quarantined` state; `install.json` allows the table to be rebuilt after DB corruption.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use specta::Type;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::archive::{
    compute_file_sha256_and_size, extract_verified_archive_with_options, ArchiveError,
    ArchiveLimits, ExtractedFileRecord,
};
use crate::capabilities::{
    verify_installed_component_capabilities, CapabilityError, CapabilityReport,
    CapabilityReportView, VerifyMode,
};
use crate::manifest::ComponentManifestEntry;
use crate::net::NetError;
use crate::security;
use crate::sidecars::SidecarSupervisor;
use crate::store::{ComponentRow, LocalStore, StoreError};
use crate::updater::{download_and_verify_component, DownloadProgress};

/// Default number of previous installed versions preserved alongside `current` during GC.
pub const DEFAULT_KEEP_PREVIOUS_VERSIONS: usize = 2;

/// Companion JSON written atomically alongside every installed version/model for DB rebuilds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallReceipt {
    pub schema: u32,
    pub component: String,
    pub version: String,
    pub kind: String, // "bin" | "model"
    pub manifest_sequence: u64,
    pub files: Vec<ExtractedFileRecord>,
    pub installed_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallOutcome {
    pub component: String,
    pub version: String,
    pub kind: String,
    pub install_path: PathBuf,
    pub already_installed: bool,
    pub capability_report: Option<CapabilityReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ComponentMissingAdvice {
    pub code: String,
    pub component: String,
    pub version: Option<String>,
    pub message: String,
    pub mentions_antivirus: bool,
    pub repair_action: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconciliationReport {
    pub deferred_deletes_succeeded: usize,
    pub deferred_deletes_remaining: usize,
    pub orphan_staging_removed: usize,
    pub uncommitted_versions_removed: usize,
    pub rebuilt_from_receipts: bool,
    pub fallbacks_activated: Vec<String>,
    pub missing_components: Vec<ComponentMissingAdvice>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct VerifyRepairOutcome {
    pub component: String,
    pub version: Option<String>,
    pub intact: bool,
    pub repaired: bool,
    pub error_code: Option<String>,
    pub message: String,
    pub capability_report: Option<CapabilityReportView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GcOutcome {
    pub component: String,
    pub removed_versions: Vec<String>,
    pub deferred_versions: Vec<String>,
    pub skipped_in_use: Vec<String>,
    pub kept_versions: Vec<String>,
}

#[derive(Error, Debug)]
pub enum InstallError {
    #[error("Component '{component}' is not allowlisted: {reason}")]
    NotAllowlisted { component: String, reason: String },

    #[error("Destination path exceeds Windows safe path limit: {0}")]
    PathTooLong(String),

    #[error("Destination version directory already exists: {0}")]
    VersionDirAlreadyExists(String),

    #[error("Component '{component}' version '{version}' is quarantined: {reason}")]
    Quarantined {
        component: String,
        version: String,
        reason: String,
    },

    #[error("Network or transfer error: {0}")]
    Network(#[from] NetError),

    #[error("Archive verification/extraction error: {0}")]
    Archive(#[from] ArchiveError),

    #[error("Capability verification failed for {component} v{version}: {reason}")]
    CapabilityFailed {
        component: String,
        version: String,
        reason: String,
        rolled_back_to: Option<String>,
        report: Option<Box<CapabilityReport>>,
    },

    #[error("Insufficient disk space during install: available {available} bytes, required {required} bytes")]
    DiskFull { available: u64, required: u64 },

    #[error("Required component '{component}' is missing or corrupted (it may have been removed or quarantined by antivirus software). Use Repair to re-download: {detail}")]
    ComponentMissing { component: String, detail: String },

    #[error("Security boundary error: {0}")]
    Security(#[from] security::SecurityError),

    #[error("Database store error: {0}")]
    Store(#[from] StoreError),

    #[error("IO error during install: {0}")]
    Io(String),
}

impl InstallError {
    pub fn error_code(&self) -> &'static str {
        match self {
            InstallError::Network(ne) => ne.error_code(),
            InstallError::Archive(ae) => ae.error_code(),
            InstallError::DiskFull { .. } => "E_DISK_FULL",
            InstallError::ComponentMissing { .. } => "E_COMPONENT_MISSING",
            InstallError::Security(se) => se.error_code(),
            _ => "E_VERIFY_FAILED",
        }
    }

    pub fn message_key(&self) -> String {
        match self {
            InstallError::Network(ne) => ne.message_key().to_string(),
            _ => format!("err.{}.body", self.error_code()),
        }
    }
}

/// Process-local reference tracker for component versions currently executing in child sidecars.
/// Prevents garbage collection from deleting a directory in use by a live child process.
#[derive(Clone, Default)]
pub struct InUseRegistry {
    counts: Arc<Mutex<HashMap<(String, String), usize>>>,
}

impl InUseRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn acquire(&self, component: &str, version: &str) -> VersionLease {
        if let Ok(mut map) = self.counts.lock() {
            let key = (component.to_string(), version.to_string());
            let entry = map.entry(key).or_insert(0);
            *entry = entry.saturating_add(1);
        }
        VersionLease {
            registry: self.clone(),
            component: component.to_string(),
            version: version.to_string(),
        }
    }

    pub fn is_in_use(&self, component: &str, version: &str) -> bool {
        self.counts
            .lock()
            .map(|m| {
                m.get(&(component.to_string(), version.to_string()))
                    .copied()
                    .unwrap_or(0)
                    > 0
            })
            .unwrap_or(false)
    }

    fn release(&self, component: &str, version: &str) {
        if let Ok(mut map) = self.counts.lock() {
            let key = (component.to_string(), version.to_string());
            if let Some(count) = map.get_mut(&key) {
                if *count <= 1 {
                    map.remove(&key);
                } else {
                    *count -= 1;
                }
            }
        }
    }
}

/// RAII lease held while a sidecar binary from `(component, version)` is running.
pub struct VersionLease {
    registry: InUseRegistry,
    component: String,
    version: String,
}

impl Drop for VersionLease {
    fn drop(&mut self) {
        self.registry.release(&self.component, &self.version);
    }
}

/// Maps a component identifier to its primary executable filename, or `None` if unrecognized.
pub fn primary_executable_for_component(component: &str) -> Option<&'static str> {
    match component {
        "yt-dlp" => Some("yt-dlp.exe"),
        "qjs" | "quickjs" => Some("qjs.exe"),
        "ffmpeg" => Some("ffmpeg.exe"),
        "whisper-cli" | "whisper" => Some("whisper-cli.exe"),
        #[cfg(any(debug_assertions, test))]
        "fake-sidecar" => Some("fake-sidecar.exe"),
        _ => None,
    }
}

/// Distinguishes binary sidecars (`bin`) vs speech models (`model`) cleanly.
pub fn classify_component_kind(component: &str) -> Result<&'static str, InstallError> {
    if security::is_allowed_model(component) {
        return Ok("model");
    }
    if primary_executable_for_component(component).is_some() {
        return Ok("bin");
    }
    Err(InstallError::NotAllowlisted {
        component: component.to_string(),
        reason: "unknown component or model name".to_string(),
    })
}

pub fn model_receipt_path(models_dir: &Path, model_filename: &str) -> PathBuf {
    models_dir.join(format!(".{}.install.json", model_filename))
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn map_io_install_err(e: io::Error, required: u64) -> InstallError {
    if e.kind() == io::ErrorKind::StorageFull || matches!(e.raw_os_error(), Some(39) | Some(112)) {
        InstallError::DiskFull {
            available: 0,
            required,
        }
    } else {
        InstallError::Io(e.to_string())
    }
}

fn check_full_path_limit(path: &Path, limit: usize) -> Result<(), InstallError> {
    let len = path.to_string_lossy().len();
    if len > limit {
        return Err(InstallError::PathTooLong(format!(
            "Path length {} exceeds limit {} ('{}')",
            len,
            limit,
            path.display()
        )));
    }
    Ok(())
}

/// Atomically replaces `dest` with `src` on the same volume.
#[cfg(windows)]
fn atomic_replace_file_same_volume(src: &Path, dest: &Path) -> Result<(), io::Error> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let src_w: Vec<u16> = src.as_os_str().encode_wide().chain(Some(0)).collect();
    let dst_w: Vec<u16> = dest.as_os_str().encode_wide().chain(Some(0)).collect();
    let ok = unsafe {
        MoveFileExW(
            src_w.as_ptr(),
            dst_w.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_replace_file_same_volume(src: &Path, dest: &Path) -> Result<(), io::Error> {
    fs::rename(src, dest)
}

fn write_receipt_atomically(dest: &Path, receipt: &InstallReceipt) -> Result<(), InstallError> {
    let parent = dest
        .parent()
        .ok_or_else(|| InstallError::Io("Missing receipt parent dir".to_string()))?;
    fs::create_dir_all(parent).map_err(|e| map_io_install_err(e, 4096))?;
    let tmp = parent.join(format!(".receipt-{}.tmp", Uuid::new_v4()));
    let json = serde_json::to_string_pretty(receipt)
        .map_err(|e| InstallError::Io(format!("Serialize install.json failed: {}", e)))?;
    fs::write(&tmp, json.as_bytes()).map_err(|e| map_io_install_err(e, json.len() as u64))?;
    atomic_replace_file_same_volume(&tmp, dest).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        map_io_install_err(e, json.len() as u64)
    })?;
    Ok(())
}

/// RAII guard for staging directories created during non-zip or model installation.
struct DirCleanupGuard {
    path: PathBuf,
    armed: bool,
}

impl DirCleanupGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for DirCleanupGuard {
    fn drop(&mut self) {
        if self.armed && self.path.exists() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// Attempts immediate deletion of `path`; if blocked by a Windows file lock, persists into
/// SQLite `deferred_deletes` so the next startup retries removal cleanly.
pub fn remove_dir_with_deferred_fallback(
    path: &Path,
    store: &LocalStore,
) -> Result<bool, InstallError> {
    if !path.exists() {
        return Ok(true);
    }
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(true),
        Err(err) => {
            let path_str = path.to_string_lossy().to_string();
            tracing::warn!(
                "Deferring locked directory cleanup until next startup: {}",
                err
            );
            store.add_deferred_delete(&path_str, &err.to_string())?;
            Ok(false)
        }
    }
}

fn verify_expected_files_on_disk(
    base_dir: &Path,
    expected_files: &[ExtractedFileRecord],
    full_hash: bool,
) -> Result<(), String> {
    if expected_files.is_empty() {
        return Err("install.json contains empty file list".to_string());
    }
    for ef in expected_files {
        security::check_windows_segment_invariants(ef.path.rsplit('/').next().unwrap_or(&ef.path))
            .map_err(|e| e.to_string())?;
        let target = base_dir.join(Path::new(&ef.path.replace('/', "\\")));
        let meta = fs::metadata(&target).map_err(|_| {
            format!(
                "Required file '{}' is missing from '{}'",
                ef.path,
                base_dir.display()
            )
        })?;
        if !meta.is_file() {
            return Err(format!("Expected regular file at '{}'", target.display()));
        }
        if meta.len() != ef.size_bytes {
            return Err(format!(
                "File '{}' size mismatch (expected {}, got {})",
                ef.path,
                ef.size_bytes,
                meta.len()
            ));
        }
        if full_hash {
            let (sha, sz) = compute_file_sha256_and_size(&target)
                .map_err(|e| format!("Failed hashing '{}': {}", ef.path, e))?;
            if sz != ef.size_bytes || !sha.eq_ignore_ascii_case(&ef.sha256) {
                return Err(format!(
                    "File '{}' SHA-256 verification failed (expected {}, got {})",
                    ef.path, ef.sha256, sha
                ));
            }
        }
    }
    Ok(())
}

fn check_manifest_matches_receipt(
    entry: &ComponentManifestEntry,
    receipt: &InstallReceipt,
) -> bool {
    if receipt.component != entry.component || receipt.version != entry.version {
        return false;
    }
    if entry.archive == "zip" {
        if receipt.files.len() != entry.files.len() {
            return false;
        }
        let mut by_path: HashMap<String, (&str, u64)> = HashMap::new();
        for f in &receipt.files {
            by_path.insert(f.path.to_lowercase(), (&f.sha256, f.size_bytes));
        }
        for mf in &entry.files {
            match by_path.get(&mf.path.to_lowercase()) {
                Some((sha, sz)) if *sz == mf.size_bytes && sha.eq_ignore_ascii_case(&mf.sha256) => {
                }
                _ => return false,
            }
        }
        true
    } else {
        receipt.files.len() == 1
            && receipt.files[0].size_bytes == entry.size_bytes
            && receipt.files[0].sha256.eq_ignore_ascii_case(&entry.sha256)
    }
}

pub type DefaultCapCheckFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<CapabilityReport, CapabilityError>> + Send>,
>;
pub type DefaultCapCheckHook = fn(String, PathBuf, String) -> DefaultCapCheckFuture;

fn try_idempotent_bin_install(
    ws_root: &Path,
    store: &LocalStore,
    entry: &ComponentManifestEntry,
) -> Result<Option<InstallOutcome>, InstallError> {
    let version_dir = ws_root
        .join("bin")
        .join(&entry.component)
        .join(&entry.version);
    if !version_dir.is_dir() {
        return Ok(None);
    }

    let receipt = match fs::read_to_string(version_dir.join("install.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<InstallReceipt>(&raw).ok())
    {
        Some(r) => r,
        None => return Ok(None),
    };

    if !check_manifest_matches_receipt(entry, &receipt)
        || verify_expected_files_on_disk(&version_dir, &receipt.files, true).is_err()
    {
        return Ok(None);
    }

    let Some(row) = store.get_component_version(&entry.component, &entry.version)? else {
        return Ok(None);
    };
    if row.status != "installed" {
        return Ok(None);
    }
    if !row.is_current {
        store.switch_active_component_version(&entry.component, &entry.version)?;
    }
    let report = row
        .capability_report_json
        .and_then(|j| serde_json::from_str::<CapabilityReport>(&j).ok());
    Ok(Some(InstallOutcome {
        component: entry.component.clone(),
        version: entry.version.clone(),
        kind: "bin".to_string(),
        install_path: version_dir,
        already_installed: true,
        capability_report: report,
    }))
}

fn try_idempotent_model_install(
    ws_root: &Path,
    store: &LocalStore,
    entry: &ComponentManifestEntry,
) -> Result<Option<InstallOutcome>, InstallError> {
    let target_model = security::resolve_model_path_with_root(ws_root, &entry.component)?;
    let receipt_path = model_receipt_path(&ws_root.join("models"), &entry.component);
    if !target_model.is_file() || !receipt_path.is_file() {
        return Ok(None);
    }

    let Some(receipt) = fs::read_to_string(&receipt_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<InstallReceipt>(&raw).ok())
    else {
        return Ok(None);
    };

    if !check_manifest_matches_receipt(entry, &receipt) {
        return Ok(None);
    }
    let disk_ok = compute_file_sha256_and_size(&target_model)
        .map(|(sha, sz)| sz == entry.size_bytes && sha.eq_ignore_ascii_case(&entry.sha256))
        .unwrap_or(false);
    if !disk_ok {
        return Ok(None);
    }

    if store
        .get_component_version(&entry.component, &entry.version)?
        .is_some_and(|r| r.status == "installed" && r.is_current)
    {
        return Ok(Some(InstallOutcome {
            component: entry.component.clone(),
            version: entry.version.clone(),
            kind: "model".to_string(),
            install_path: target_model,
            already_installed: true,
            capability_report: None,
        }));
    }

    Ok(None)
}

/// Stages and installs a previously verified local file (archive or single binary/model),
/// performing atomic rename, SQLite current promotion, supervised capability verification,
/// automatic quarantine + rollback on failure, and GC (`keep_previous = 2`).
#[allow(clippy::too_many_arguments)]
pub async fn install_verified_artifact_with_options<F, Fut>(
    ws_root: &Path,
    store: &LocalStore,
    in_use: &InUseRegistry,
    entry: &ComponentManifestEntry,
    manifest_sequence: u64,
    verified_payload_path: &Path,
    archive_limits: &ArchiveLimits,
    free_space_override: Option<u64>,
    capability_checker: Option<F>,
) -> Result<InstallOutcome, InstallError>
where
    F: FnOnce(String, PathBuf, String) -> Fut,
    Fut: std::future::Future<Output = Result<CapabilityReport, CapabilityError>>,
{
    let kind = classify_component_kind(&entry.component)?;
    security::validate_path_segment(&entry.component)?;
    security::validate_path_segment(&entry.version)?;

    if kind == "model" {
        return install_verified_model(
            ws_root,
            store,
            entry,
            manifest_sequence,
            verified_payload_path,
            archive_limits.max_full_path_len,
            free_space_override,
        );
    }

    // Refuse reinstalling an already-quarantined version cleanly.
    if let Some(existing) = store.get_component_version(&entry.component, &entry.version)? {
        if existing.status == "quarantined" {
            return Err(InstallError::Quarantined {
                component: entry.component.clone(),
                version: entry.version.clone(),
                reason: existing
                    .quarantine_reason
                    .unwrap_or_else(|| "Previously failed capability verification".to_string()),
            });
        }
    }

    let exe_name = primary_executable_for_component(&entry.component).ok_or_else(|| {
        InstallError::NotAllowlisted {
            component: entry.component.clone(),
            reason: "missing primary executable mapping".to_string(),
        }
    })?;

    let comp_dir = ws_root.join("bin").join(&entry.component);
    let version_dir = comp_dir.join(&entry.version);
    check_full_path_limit(
        &version_dir.join("install.json"),
        archive_limits.max_full_path_len,
    )?;
    check_full_path_limit(
        &version_dir.join(exe_name),
        archive_limits.max_full_path_len,
    )?;

    // Idempotent check: if this version is ALREADY installed intact on disk + matches manifest hashes.
    if version_dir.exists() {
        if let Some(outcome) = try_idempotent_bin_install(ws_root, store, entry)? {
            return Ok(outcome);
        }
        // Directory exists on disk but is corrupt or unmanaged -> fail atomic promote rule cleanly!
        return Err(InstallError::VersionDirAlreadyExists(
            version_dir.display().to_string(),
        ));
    }

    // Extract/stage on the SAME volume (`bin/<component>/.staging-<uuid>/`)
    let (staging_dir, extracted_files) = if entry.archive == "zip" {
        for mf in &entry.files {
            check_full_path_limit(
                &version_dir.join(Path::new(&mf.path.replace('/', "\\"))),
                archive_limits.max_full_path_len,
            )?;
        }
        let staged = extract_verified_archive_with_options(
            verified_payload_path,
            &entry.sha256,
            entry.size_bytes,
            &comp_dir,
            &entry.files,
            archive_limits,
            free_space_override,
        )?;
        (staged.staging_dir, staged.files)
    } else {
        // Single binary: verify archive/file integrity, check disk space, stage into `.staging-<uuid>/`
        let (actual_sha, actual_size) = compute_file_sha256_and_size(verified_payload_path)
            .map_err(|e| InstallError::Io(e.to_string()))?;
        if actual_size != entry.size_bytes {
            return Err(InstallError::Archive(ArchiveError::ArchiveSizeMismatch {
                expected: entry.size_bytes,
                actual: actual_size,
            }));
        }
        if !actual_sha.eq_ignore_ascii_case(&entry.sha256) {
            return Err(InstallError::Archive(
                ArchiveError::ArchiveIntegrityMismatch {
                    expected: entry.sha256.clone(),
                    actual: actual_sha,
                },
            ));
        }
        let avail = match free_space_override {
            Some(a) => a,
            None => security::get_free_disk_space(ws_root)?,
        };
        if avail < entry.size_bytes {
            return Err(InstallError::DiskFull {
                available: avail,
                required: entry.size_bytes,
            });
        }

        fs::create_dir_all(&comp_dir).map_err(|e| map_io_install_err(e, entry.size_bytes))?;
        let sdir = comp_dir.join(format!(".staging-{}", Uuid::new_v4()));
        check_full_path_limit(&sdir.join(exe_name), archive_limits.max_full_path_len)?;
        fs::create_dir_all(&sdir).map_err(|e| map_io_install_err(e, entry.size_bytes))?;
        let mut s_guard = DirCleanupGuard::new(sdir.clone());

        let staged_bin = sdir.join(exe_name);
        // Copy (rather than cross-temp-dir rename) so same-volume atomic rename works reliably even
        // when `verified_payload_path` came from another volume/temp location in tests.
        fs::copy(verified_payload_path, &staged_bin)
            .map_err(|e| map_io_install_err(e, entry.size_bytes))?;
        let (copied_sha, copied_size) = compute_file_sha256_and_size(&staged_bin)
            .map_err(|e| InstallError::Io(e.to_string()))?;
        if copied_size != entry.size_bytes || !copied_sha.eq_ignore_ascii_case(&entry.sha256) {
            return Err(InstallError::Archive(ArchiveError::ExtractedHashMismatch {
                path: exe_name.to_string(),
                expected: entry.sha256.clone(),
                actual: copied_sha,
            }));
        }
        s_guard.disarm();
        (
            sdir,
            vec![ExtractedFileRecord {
                path: exe_name.to_string(),
                sha256: copied_sha,
                size_bytes: copied_size,
            }],
        )
    };

    let mut stage_guard = DirCleanupGuard::new(staging_dir.clone());

    // Verify full extracted tree before promoting
    verify_expected_files_on_disk(&staging_dir, &extracted_files, false)
        .map_err(|reason| InstallError::Io(format!("Staged tree check failed: {}", reason)))?;

    let installed_at = unix_now_secs();
    let receipt = InstallReceipt {
        schema: 1,
        component: entry.component.clone(),
        version: entry.version.clone(),
        kind: "bin".to_string(),
        manifest_sequence,
        files: extracted_files.clone(),
        installed_at,
    };
    write_receipt_atomically(&staging_dir.join("install.json"), &receipt)?;

    // Atomic same-volume rename: `.staging-<uuid>` -> `<version>` (fail if it exists).
    if version_dir.exists() {
        return Err(InstallError::VersionDirAlreadyExists(
            version_dir.display().to_string(),
        ));
    }
    fs::rename(&staging_dir, &version_dir).map_err(|e| map_io_install_err(e, entry.size_bytes))?;
    stage_guard.disarm();

    let rel_path = format!("bin/{}/{}", entry.component, entry.version);
    let files_json = serde_json::to_string(&extracted_files)
        .map_err(|e| InstallError::Io(format!("Serialize files_json failed: {}", e)))?;

    // Set active current pointer in one SQLite transaction.
    store.set_component_installed_current(
        &entry.component,
        &entry.version,
        "bin",
        &rel_path,
        manifest_sequence,
        &files_json,
        None,
        installed_at,
    )?;

    // Run capability verification on the promoted directory; quarantine & roll back on failure!
    let cap_res = match capability_checker {
        Some(f) => {
            f(
                entry.component.clone(),
                version_dir.clone(),
                entry.version.clone(),
            )
            .await
        }
        None => {
            let supervisor = SidecarSupervisor::new();
            verify_installed_component_capabilities(
                &entry.component,
                &version_dir,
                &entry.version,
                VerifyMode::Strict,
                &supervisor,
                ws_root,
            )
            .await
        }
    };

    match cap_res {
        Ok(report) => {
            if let Ok(report_json) = serde_json::to_string(&report) {
                let _ = store.update_component_capability_report(
                    &entry.component,
                    &entry.version,
                    &report_json,
                );
            }
            let _ = gc_component_versions(
                ws_root,
                store,
                &entry.component,
                DEFAULT_KEEP_PREVIOUS_VERSIONS,
                in_use,
            );
            Ok(InstallOutcome {
                component: entry.component.clone(),
                version: entry.version.clone(),
                kind: "bin".to_string(),
                install_path: version_dir,
                already_installed: false,
                capability_report: Some(report),
            })
        }
        Err(cap_err) => {
            let reason = cap_err.to_string();
            let report_opt = cap_err.report().cloned();
            let report_json = report_opt
                .as_ref()
                .and_then(|r| serde_json::to_string(r).ok());

            let rolled_back_to = store.quarantine_and_rollback_component(
                &entry.component,
                &entry.version,
                "bin",
                &rel_path,
                manifest_sequence,
                &files_json,
                installed_at,
                &reason,
                report_json.as_deref(),
            )?;

            tracing::warn!(
                "Component {} v{} failed capability verification ({}); quarantined and rolled back to {:?}",
                entry.component,
                entry.version,
                reason,
                rolled_back_to
            );

            Err(InstallError::CapabilityFailed {
                component: entry.component.clone(),
                version: entry.version.clone(),
                reason,
                rolled_back_to,
                report: report_opt.map(Box::new),
            })
        }
    }
}

fn install_verified_model(
    ws_root: &Path,
    store: &LocalStore,
    entry: &ComponentManifestEntry,
    manifest_sequence: u64,
    verified_payload_path: &Path,
    max_full_path_len: usize,
    free_space_override: Option<u64>,
) -> Result<InstallOutcome, InstallError> {
    let models_dir = ws_root.join("models");
    let target_model = security::resolve_model_path_with_root(ws_root, &entry.component)?;
    let receipt_path = model_receipt_path(&models_dir, &entry.component);
    check_full_path_limit(&target_model, max_full_path_len)?;
    check_full_path_limit(&receipt_path, max_full_path_len)?;

    // Check idempotent reinstall if existing model file + receipt match manifest hash & size.
    if let Some(outcome) = try_idempotent_model_install(ws_root, store, entry)? {
        return Ok(outcome);
    }

    let (actual_sha, actual_size) = compute_file_sha256_and_size(verified_payload_path)
        .map_err(|e| InstallError::Io(e.to_string()))?;
    if actual_size != entry.size_bytes {
        return Err(InstallError::Archive(ArchiveError::ArchiveSizeMismatch {
            expected: entry.size_bytes,
            actual: actual_size,
        }));
    }
    if !actual_sha.eq_ignore_ascii_case(&entry.sha256) {
        return Err(InstallError::Archive(
            ArchiveError::ArchiveIntegrityMismatch {
                expected: entry.sha256.clone(),
                actual: actual_sha,
            },
        ));
    }

    let avail = match free_space_override {
        Some(a) => a,
        None => security::get_free_disk_space(ws_root)?,
    };
    if avail < entry.size_bytes {
        return Err(InstallError::DiskFull {
            available: avail,
            required: entry.size_bytes,
        });
    }

    fs::create_dir_all(&models_dir).map_err(|e| map_io_install_err(e, entry.size_bytes))?;
    let staging_dir = models_dir.join(format!(".staging-{}", Uuid::new_v4()));
    fs::create_dir_all(&staging_dir).map_err(|e| map_io_install_err(e, entry.size_bytes))?;
    let _guard = DirCleanupGuard::new(staging_dir.clone());

    let staged_file = staging_dir.join(&entry.component);
    fs::copy(verified_payload_path, &staged_file)
        .map_err(|e| map_io_install_err(e, entry.size_bytes))?;
    let (staged_sha, staged_sz) =
        compute_file_sha256_and_size(&staged_file).map_err(|e| InstallError::Io(e.to_string()))?;
    if staged_sz != entry.size_bytes || !staged_sha.eq_ignore_ascii_case(&entry.sha256) {
        return Err(InstallError::Archive(ArchiveError::ExtractedHashMismatch {
            path: entry.component.clone(),
            expected: entry.sha256.clone(),
            actual: staged_sha,
        }));
    }

    let installed_at = unix_now_secs();
    let files = vec![ExtractedFileRecord {
        path: entry.component.clone(),
        sha256: staged_sha,
        size_bytes: staged_sz,
    }];
    let receipt = InstallReceipt {
        schema: 1,
        component: entry.component.clone(),
        version: entry.version.clone(),
        kind: "model".to_string(),
        manifest_sequence,
        files: files.clone(),
        installed_at,
    };

    atomic_replace_file_same_volume(&staged_file, &target_model)
        .map_err(|e| map_io_install_err(e, entry.size_bytes))?;
    write_receipt_atomically(&receipt_path, &receipt)?;

    let files_json = serde_json::to_string(&files)
        .map_err(|e| InstallError::Io(format!("Serialize model files failed: {}", e)))?;
    let rel_path = format!("models/{}", entry.component);
    store.set_component_installed_current(
        &entry.component,
        &entry.version,
        "model",
        &rel_path,
        manifest_sequence,
        &files_json,
        None,
        installed_at,
    )?;

    // Because models/<file> holds the single active file on disk, clean superseded model version rows
    for row in store.list_component_versions(&entry.component)? {
        if row.version != entry.version && row.kind == "model" {
            let _ = store.delete_component_version_row(&entry.component, &row.version);
        }
    }

    Ok(InstallOutcome {
        component: entry.component.clone(),
        version: entry.version.clone(),
        kind: "model".to_string(),
        install_path: target_model,
        already_installed: false,
        capability_report: None,
    })
}

/// Full end-to-end production flow: download -> verify -> stage -> atomic rename -> promote ->
/// supervised capability verification (or quarantine + rollback on failure).
pub async fn download_and_install_component<F>(
    ws_root: &Path,
    store: &LocalStore,
    in_use: &InUseRegistry,
    entry: &ComponentManifestEntry,
    manifest_sequence: u64,
    on_progress: F,
    cancel_token: Option<&CancellationToken>,
) -> Result<InstallOutcome, InstallError>
where
    F: Fn(DownloadProgress) + Send + Sync + 'static,
{
    let kind = classify_component_kind(&entry.component)?;
    security::validate_path_segment(&entry.component)?;
    security::validate_path_segment(&entry.version)?;

    // Pre-check idempotency / quarantine before hitting network!
    if kind == "bin" {
        if let Some(existing) = store.get_component_version(&entry.component, &entry.version)? {
            if existing.status == "quarantined" {
                return Err(InstallError::Quarantined {
                    component: entry.component.clone(),
                    version: entry.version.clone(),
                    reason: existing
                        .quarantine_reason
                        .unwrap_or_else(|| "Previously quarantined".to_string()),
                });
            }
        }
        if let Some(outcome) = try_idempotent_bin_install(ws_root, store, entry)? {
            return Ok(outcome);
        }
    } else if kind == "model" {
        if let Some(outcome) = try_idempotent_model_install(ws_root, store, entry)? {
            return Ok(outcome);
        }
    }

    let dl_dir = ws_root.join("cache").join("downloads");
    fs::create_dir_all(&dl_dir).map_err(|e| map_io_install_err(e, entry.size_bytes))?;
    let pkg_name = if entry.archive == "zip" {
        format!("{}-{}.zip", entry.component, entry.version)
    } else {
        format!("{}-{}.pkg", entry.component, entry.version)
    };
    let dl_path = dl_dir.join(pkg_name);

    download_and_verify_component(entry, &dl_path, on_progress, cancel_token).await?;

    let no_override: Option<DefaultCapCheckHook> = None;

    let res = install_verified_artifact_with_options(
        ws_root,
        store,
        in_use,
        entry,
        manifest_sequence,
        &dl_path,
        &ArchiveLimits::default(),
        None,
        no_override,
    )
    .await;

    let _ = fs::remove_file(&dl_path);
    res
}

/// Garbage-collects older `installed` versions beyond `keep_previous`, keeping `current` and any
/// version currently locked in `in_use`. Windows file-lock failures are deferred to next startup.
pub fn gc_component_versions(
    ws_root: &Path,
    store: &LocalStore,
    component: &str,
    keep_previous: usize,
    in_use: &InUseRegistry,
) -> Result<GcOutcome, InstallError> {
    let rows = store.list_component_versions(component)?;
    let mut kept_versions = Vec::new();
    let mut removed_versions = Vec::new();
    let mut deferred_versions = Vec::new();
    let mut skipped_in_use = Vec::new();

    let mut non_current_installed: Vec<ComponentRow> = Vec::new();
    for r in rows {
        if r.kind != "bin" {
            if r.is_current {
                kept_versions.push(r.version);
            }
            continue;
        }
        if r.status == "quarantined" {
            // Keep quarantined rows intact for diagnostics unless explicitly purged.
            kept_versions.push(format!("{} (quarantined)", r.version));
            continue;
        }
        if r.is_current {
            kept_versions.push(r.version);
            continue;
        }
        non_current_installed.push(r);
    }

    // Order non-current installed versions: explicit `is_previous` first, then newest `installed_at`.
    non_current_installed.sort_by(|a, b| {
        b.is_previous
            .cmp(&a.is_previous)
            .then_with(|| b.installed_at.cmp(&a.installed_at))
            .then_with(|| b.manifest_sequence.cmp(&a.manifest_sequence))
    });

    for (idx, row) in non_current_installed.into_iter().enumerate() {
        if idx < keep_previous {
            kept_versions.push(row.version);
            continue;
        }
        if in_use.is_in_use(component, &row.version) {
            skipped_in_use.push(row.version);
            continue;
        }

        let dir_path = ws_root.join(Path::new(&row.install_rel_path.replace('/', "\\")));
        store.delete_component_version_row(component, &row.version)?;
        if remove_dir_with_deferred_fallback(&dir_path, store)? {
            removed_versions.push(row.version);
        } else {
            deferred_versions.push(row.version);
        }
    }

    Ok(GcOutcome {
        component: component.to_string(),
        removed_versions,
        deferred_versions,
        skipped_in_use,
        kept_versions,
    })
}

/// Switches a binary component back to its `previous` installed version (or specific target version).
pub fn rollback_component(
    ws_root: &Path,
    store: &LocalStore,
    component: &str,
    target_version: Option<&str>,
) -> Result<String, InstallError> {
    let target = match target_version {
        Some(v) => v.to_string(),
        None => {
            store
                .get_previous_component(component)?
                .ok_or_else(|| InstallError::ComponentMissing {
                    component: component.to_string(),
                    detail: "No previous installed version available for rollback".to_string(),
                })?
                .version
        }
    };

    let row = store
        .get_component_version(component, &target)?
        .ok_or_else(|| InstallError::ComponentMissing {
            component: component.to_string(),
            detail: format!("Target version '{}' not found in database", target),
        })?;

    if row.status == "quarantined" {
        return Err(InstallError::Quarantined {
            component: component.to_string(),
            version: target,
            reason: row
                .quarantine_reason
                .unwrap_or_else(|| "Quarantined version cannot be activated".to_string()),
        });
    }

    let dir_path = ws_root.join(Path::new(&row.install_rel_path.replace('/', "\\")));
    let receipt_path = dir_path.join("install.json");
    let receipt_raw =
        fs::read_to_string(&receipt_path).map_err(|_| InstallError::ComponentMissing {
            component: component.to_string(),
            detail: format!("Missing install.json in '{}'", dir_path.display()),
        })?;
    let receipt: InstallReceipt = serde_json::from_str(&receipt_raw)
        .map_err(|e| InstallError::Io(format!("Corrupt install.json for {}: {}", component, e)))?;

    verify_expected_files_on_disk(&dir_path, &receipt.files, false).map_err(|detail| {
        InstallError::ComponentMissing {
            component: component.to_string(),
            detail,
        }
    })?;

    store.switch_active_component_version(component, &target)?;
    Ok(target)
}

/// Rebuilds the `components` SQLite table after database corruption by scanning on-disk
/// `bin/<component>/<version>/install.json` and `models/.*.install.json` receipts.
pub fn rebuild_components_from_disk(
    ws_root: &Path,
    store: &LocalStore,
) -> Result<usize, InstallError> {
    let mut receipts: Vec<(InstallReceipt, String)> = Vec::new();
    let bin_root = ws_root.join("bin");
    if bin_root.is_dir() {
        for comp_entry in fs::read_dir(&bin_root).map_err(|e| InstallError::Io(e.to_string()))? {
            let comp_entry = match comp_entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !comp_entry.path().is_dir() {
                continue;
            }
            let comp_name = comp_entry.file_name().to_string_lossy().to_string();
            if primary_executable_for_component(&comp_name).is_none() {
                continue;
            }
            let ver_entries = match fs::read_dir(comp_entry.path()) {
                Ok(r) => r,
                Err(_) => continue,
            };
            for ver_entry in ver_entries.flatten() {
                let ver_dir = ver_entry.path();
                let ver_name = ver_entry.file_name().to_string_lossy().to_string();
                if ver_name.starts_with(".staging-") || !ver_dir.is_dir() {
                    continue;
                }
                let rpath = ver_dir.join("install.json");
                let Ok(raw) = fs::read_to_string(&rpath) else {
                    tracing::warn!(
                        "Skipping version without readable install.json: {}",
                        ver_dir.display()
                    );
                    continue;
                };
                let Ok(receipt) = serde_json::from_str::<InstallReceipt>(&raw) else {
                    tracing::warn!("Skipping corrupted install.json in {}", ver_dir.display());
                    continue;
                };
                if receipt.component != comp_name || receipt.version != ver_name {
                    tracing::warn!("Mismatched component/version in {}", rpath.display());
                    continue;
                }
                if verify_expected_files_on_disk(&ver_dir, &receipt.files, false).is_err() {
                    tracing::warn!(
                        "Incomplete version files in {}, skipping during rebuild",
                        ver_dir.display()
                    );
                    continue;
                }
                receipts.push((receipt, format!("bin/{}/{}", comp_name, ver_name)));
            }
        }
    }

    let models_dir = ws_root.join("models");
    if models_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&models_dir) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                if fname.starts_with('.')
                    && fname.ends_with(".install.json")
                    && entry.path().is_file()
                {
                    if let Ok(raw) = fs::read_to_string(entry.path()) {
                        if let Ok(receipt) = serde_json::from_str::<InstallReceipt>(&raw) {
                            if security::is_allowed_model(&receipt.component)
                                && verify_expected_files_on_disk(&models_dir, &receipt.files, false)
                                    .is_ok()
                            {
                                let rel = format!("models/{}", receipt.component);
                                receipts.push((receipt, rel));
                            }
                        }
                    }
                }
            }
        }
    }

    // Group by component and assign newest intact version as is_current=1, second newest as is_previous=1.
    let mut grouped: HashMap<String, Vec<(InstallReceipt, String)>> = HashMap::new();
    for item in receipts {
        grouped
            .entry(item.0.component.clone())
            .or_default()
            .push(item);
    }

    let now = unix_now_secs();
    let mut rows: Vec<ComponentRow> = Vec::new();
    for (_, mut list) in grouped {
        list.sort_by(|a, b| {
            b.0.installed_at
                .cmp(&a.0.installed_at)
                .then_with(|| b.0.manifest_sequence.cmp(&a.0.manifest_sequence))
        });
        for (idx, (receipt, rel_path)) in list.into_iter().enumerate() {
            let files_json = serde_json::to_string(&receipt.files).unwrap_or_else(|_| "[]".into());
            rows.push(ComponentRow {
                component: receipt.component,
                version: receipt.version,
                kind: receipt.kind,
                install_rel_path: rel_path,
                status: "installed".to_string(),
                is_current: idx == 0,
                is_previous: idx == 1 && receipt_is_bin(&rows, idx), // bin only for previous
                manifest_sequence: receipt.manifest_sequence,
                files_json,
                capability_report_json: None,
                quarantine_reason: None,
                installed_at: receipt.installed_at,
                updated_at: now,
            });
        }
    }

    let count = rows.len();
    store.replace_rebuilt_components(&rows)?;
    let _ = store.delete_setting("needs_component_rebuild");
    Ok(count)
}

fn receipt_is_bin(_rows: &[ComponentRow], idx: usize) -> bool {
    idx == 1
}

/// Startup reconciliation:
/// 1. Retries previously deferred locked deletions.
/// 2. Rebuilds `components` from `install.json` if DB recovered from corruption (`needs_component_rebuild`).
/// 3. Deletes orphan `.staging-*` directories in `bin/*` and `models/`.
/// 4. Cleans uncommitted version directories created between atomic rename and pointer switch.
/// 5. Performs quick check (dir, `install.json`, file existence & sizes) on every `current` component;
///    if broken (e.g. deleted/quarantined by antivirus), falls back to intact `previous` version or
///    emits an actionable `E_COMPONENT_MISSING` advice mentioning antivirus + Repair.
pub fn reconcile_startup(
    ws_root: &Path,
    store: &LocalStore,
) -> Result<ReconciliationReport, InstallError> {
    let mut report = ReconciliationReport {
        deferred_deletes_succeeded: 0,
        deferred_deletes_remaining: 0,
        orphan_staging_removed: 0,
        uncommitted_versions_removed: 0,
        rebuilt_from_receipts: false,
        fallbacks_activated: Vec::new(),
        missing_components: Vec::new(),
    };

    // 1. Retry deferred deletions blocked by earlier Windows file locks.
    for dd in store.list_deferred_deletes()? {
        let p = PathBuf::from(&dd.path);
        if !p.exists() {
            store.remove_deferred_delete(&dd.path)?;
            report.deferred_deletes_succeeded += 1;
            continue;
        }
        match fs::remove_dir_all(&p) {
            Ok(()) => {
                store.remove_deferred_delete(&dd.path)?;
                report.deferred_deletes_succeeded += 1;
            }
            Err(e) => {
                store.add_deferred_delete(&dd.path, &e.to_string())?;
                report.deferred_deletes_remaining += 1;
            }
        }
    }

    // 2. If SQLite recovered from corruption, rebuild components table from install.json receipts.
    if store.get_setting("needs_component_rebuild")?.as_deref() == Some("1") {
        let rebuilt_count = rebuild_components_from_disk(ws_root, store)?;
        if rebuilt_count > 0 {
            report.rebuilt_from_receipts = true;
        }
    }

    // 3. Remove orphan `.staging-*` dirs and uncommitted version folders not tracked in SQLite.
    let known_rows: HashSet<(String, String)> = store
        .list_all_components()?
        .into_iter()
        .map(|r| (r.component, r.version))
        .collect();

    let bin_root = ws_root.join("bin");
    if bin_root.is_dir() {
        for comp_entry in fs::read_dir(&bin_root)
            .map_err(|e| InstallError::Io(e.to_string()))?
            .flatten()
        {
            let comp_path = comp_entry.path();
            if !comp_path.is_dir() {
                continue;
            }
            let comp_name = comp_entry.file_name().to_string_lossy().to_string();
            if let Ok(sub_entries) = fs::read_dir(&comp_path) {
                for sub in sub_entries.flatten() {
                    let sub_name = sub.file_name().to_string_lossy().to_string();
                    let sub_path = sub.path();
                    if sub_name.starts_with(".staging-") {
                        if remove_dir_with_deferred_fallback(&sub_path, store)? {
                            report.orphan_staging_removed += 1;
                        }
                    } else if sub_path.is_dir()
                        && !known_rows.contains(&(comp_name.clone(), sub_name))
                    {
                        // Aborted install left dir after rename before SQLite transaction committed
                        if remove_dir_with_deferred_fallback(&sub_path, store)? {
                            report.uncommitted_versions_removed += 1;
                        }
                    }
                }
            }
        }
    }

    let models_dir = ws_root.join("models");
    if models_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&models_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(".staging-")
                    && entry.path().is_dir()
                    && remove_dir_with_deferred_fallback(&entry.path(), store)?
                {
                    report.orphan_staging_removed += 1;
                }
            }
        }
    }

    // 3b. Clean stale per-component temporary directories under `cache/temp/` from prior runs.
    let cache_temp_dir = ws_root.join("cache").join("temp");
    if cache_temp_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&cache_temp_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    let _ = fs::remove_dir_all(&p);
                } else {
                    let _ = fs::remove_file(&p);
                }
            }
        }
    }

    // 4. Verify every `current` component/model (quick check) & fall back to intact `previous` if broken.
    let all_rows = store.list_all_components()?;
    let mut components_seen: HashSet<String> = HashSet::new();
    for row in &all_rows {
        components_seen.insert(row.component.clone());
    }

    for comp in components_seen {
        let Some(curr) = store.get_current_component(&comp)? else {
            continue;
        };
        let check_err = quick_verify_row(ws_root, &curr);
        if let Err(reason) = check_err {
            tracing::error!(
                "Startup quick check failed for active {} v{}: {}",
                comp,
                curr.version,
                reason
            );
            let files: Vec<ExtractedFileRecord> =
                serde_json::from_str(&curr.files_json).unwrap_or_default();
            let files_json_str = &curr.files_json;
            let restored = store.quarantine_and_rollback_component(
                &curr.component,
                &curr.version,
                &curr.kind,
                &curr.install_rel_path,
                curr.manifest_sequence,
                files_json_str,
                curr.installed_at,
                &format!("Startup check failed: {}", reason),
                curr.capability_report_json.as_deref(),
            )?;

            let mut recovered = false;
            if let Some(prev_ver) = restored {
                if let Some(prev_row) = store.get_component_version(&comp, &prev_ver)? {
                    if quick_verify_row(ws_root, &prev_row).is_ok() {
                        report
                            .fallbacks_activated
                            .push(format!("{}: {} -> {}", comp, curr.version, prev_ver));
                        recovered = true;
                    } else {
                        let _ = store.quarantine_and_rollback_component(
                            &prev_row.component,
                            &prev_row.version,
                            &prev_row.kind,
                            &prev_row.install_rel_path,
                            prev_row.manifest_sequence,
                            &prev_row.files_json,
                            prev_row.installed_at,
                            "Fallback version also missing or broken",
                            prev_row.capability_report_json.as_deref(),
                        );
                    }
                }
            }

            if !recovered {
                let _ = files;
                report.missing_components.push(ComponentMissingAdvice {
                    code: "E_COMPONENT_MISSING".to_string(),
                    component: comp,
                    version: Some(curr.version),
                    message: format!(
                        "Component files failed integrity verification ({}). Antivirus software may have removed or quarantined an executable. Run Repair to re-download.",
                        reason
                    ),
                    mentions_antivirus: true,
                    repair_action: true,
                });
            }
        }
    }

    Ok(report)
}

/// Quick check run on startup and before component resolution: verifies directory/model exists,
/// receipt `install.json` parses cleanly, and all expected files exist with exact expected sizes.
pub fn quick_verify_row(ws_root: &Path, row: &ComponentRow) -> Result<(), String> {
    if row.kind == "model" {
        let model_path = security::resolve_model_path_with_root(ws_root, &row.component)
            .map_err(|e| e.to_string())?;
        let receipt_path = model_receipt_path(&ws_root.join("models"), &row.component);
        let raw = fs::read_to_string(&receipt_path)
            .map_err(|_| format!("Missing model install receipt '{}'", receipt_path.display()))?;
        let receipt: InstallReceipt = serde_json::from_str(&raw)
            .map_err(|e| format!("Corrupt model install receipt: {}", e))?;
        if receipt.component != row.component || receipt.version != row.version {
            return Err("Model receipt metadata disagrees with database".to_string());
        }
        let expected: Vec<ExtractedFileRecord> =
            serde_json::from_str(&row.files_json).unwrap_or(receipt.files);
        verify_expected_files_on_disk(&ws_root.join("models"), &expected, false)?;
        if !model_path.is_file() {
            return Err(format!("Model file vanished: {}", model_path.display()));
        }
        Ok(())
    } else {
        let ver_dir = ws_root.join(Path::new(&row.install_rel_path.replace('/', "\\")));
        if !ver_dir.is_dir() {
            return Err(format!(
                "Component version directory missing: {}",
                ver_dir.display()
            ));
        }
        let receipt_path = ver_dir.join("install.json");
        let raw = fs::read_to_string(&receipt_path)
            .map_err(|_| format!("Missing install.json in '{}'", ver_dir.display()))?;
        let receipt: InstallReceipt =
            serde_json::from_str(&raw).map_err(|e| format!("Corrupt install.json: {}", e))?;
        if receipt.component != row.component || receipt.version != row.version {
            return Err("install.json component/version mismatch".to_string());
        }
        verify_expected_files_on_disk(&ver_dir, &receipt.files, false)
    }
}

/// Runs a **full SHA-256 hash verification** (`Verify / Repair` check) on the active version of a component.
pub async fn verify_active_component_full(
    ws_root: &Path,
    store: &LocalStore,
    component: &str,
) -> VerifyRepairOutcome {
    let curr = match store.get_current_component(component) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return VerifyRepairOutcome {
                component: component.to_string(),
                version: None,
                intact: false,
                repaired: false,
                error_code: Some("E_COMPONENT_MISSING".to_string()),
                message: format!(
                    "No active version installed for '{}'. If files vanished due to antivirus software, click Repair to re-download.",
                    component
                ),
                capability_report: None,
            };
        }
        Err(e) => {
            return VerifyRepairOutcome {
                component: component.to_string(),
                version: None,
                intact: false,
                repaired: false,
                error_code: Some("E_VERIFY_FAILED".to_string()),
                message: e.to_string(),
                capability_report: None,
            };
        }
    };

    let base_dir = if curr.kind == "model" {
        ws_root.join("models")
    } else {
        ws_root.join(Path::new(&curr.install_rel_path.replace('/', "\\")))
    };

    let receipt_res: Result<InstallReceipt, String> = (|| {
        let rpath = if curr.kind == "model" {
            model_receipt_path(&ws_root.join("models"), &curr.component)
        } else {
            base_dir.join("install.json")
        };
        let raw = fs::read_to_string(&rpath)
            .map_err(|_| format!("Missing receipt '{}'", rpath.display()))?;
        serde_json::from_str(&raw).map_err(|e| format!("Corrupt install.json: {}", e))
    })();

    let check_res = receipt_res
        .and_then(|receipt| verify_expected_files_on_disk(&base_dir, &receipt.files, true));

    if let Err(err_msg) = check_res {
        return VerifyRepairOutcome {
            component: component.to_string(),
            version: Some(curr.version),
            intact: false,
            repaired: false,
            error_code: Some("E_COMPONENT_MISSING".to_string()),
            message: format!(
                "{}. Antivirus software may have quarantined or removed component files. Click Repair to re-download.",
                err_msg
            ),
            capability_report: None,
        };
    }

    // Also refresh capability check for binary components when verifying
    if curr.kind == "bin" {
        let supervisor = SidecarSupervisor::new();
        match verify_installed_component_capabilities(
            &curr.component,
            &base_dir,
            &curr.version,
            VerifyMode::Strict,
            &supervisor,
            ws_root,
        )
        .await
        {
            Ok(cap_report) => {
                if let Ok(json) = serde_json::to_string(&cap_report) {
                    let _ = store.update_component_capability_report(
                        &curr.component,
                        &curr.version,
                        &json,
                    );
                }
                VerifyRepairOutcome {
                    component: component.to_string(),
                    version: Some(curr.version),
                    intact: true,
                    repaired: false,
                    error_code: None,
                    message: "All files passed full SHA-256 and capability verification"
                        .to_string(),
                    capability_report: Some(CapabilityReportView::from(&cap_report)),
                }
            }
            Err(cap_err) => VerifyRepairOutcome {
                component: component.to_string(),
                version: Some(curr.version),
                intact: false,
                repaired: false,
                error_code: Some(cap_err.error_code().to_string()),
                message: cap_err.to_string(),
                capability_report: cap_err.report().map(CapabilityReportView::from),
            },
        }
    } else {
        VerifyRepairOutcome {
            component: component.to_string(),
            version: Some(curr.version),
            intact: true,
            repaired: false,
            error_code: None,
            message: "Model passed full SHA-256 verification".to_string(),
            capability_report: None,
        }
    }
}

/// Re-runs strict capability verification on an already-extracted `quarantined` binary version
/// without requiring a full re-download. Promotes to `current` if verification passes.
pub async fn retry_quarantined_component_verification(
    ws_root: &Path,
    store: &LocalStore,
    component: &str,
    version: &str,
) -> Result<VerifyRepairOutcome, InstallError> {
    let row = store
        .get_component_version(component, version)?
        .ok_or_else(|| InstallError::ComponentMissing {
            component: component.to_string(),
            detail: format!("Version '{}' is not present in database", version),
        })?;

    if row.kind != "bin" {
        return Ok(verify_active_component_full(ws_root, store, component).await);
    }

    let ver_dir = ws_root.join(Path::new(&row.install_rel_path.replace('/', "\\")));
    let receipt_res: Result<InstallReceipt, String> =
        fs::read_to_string(ver_dir.join("install.json"))
            .map_err(|_| format!("Missing install.json in '{}'", ver_dir.display()))
            .and_then(|raw| {
                serde_json::from_str(&raw).map_err(|e| format!("Corrupt install.json: {}", e))
            });

    if let Err(err_msg) =
        receipt_res.and_then(|r| verify_expected_files_on_disk(&ver_dir, &r.files, true))
    {
        return Ok(VerifyRepairOutcome {
            component: component.to_string(),
            version: Some(version.to_string()),
            intact: false,
            repaired: false,
            error_code: Some("E_COMPONENT_MISSING".to_string()),
            message: format!(
                "{}. Antivirus software may have quarantined or removed component files. Click Repair to re-download.",
                err_msg
            ),
            capability_report: None,
        });
    }

    let supervisor = SidecarSupervisor::new();
    match verify_installed_component_capabilities(
        &row.component,
        &ver_dir,
        &row.version,
        VerifyMode::Strict,
        &supervisor,
        ws_root,
    )
    .await
    {
        Ok(report) => {
            let report_json = serde_json::to_string(&report).ok();
            store.set_component_installed_current(
                &row.component,
                &row.version,
                "bin",
                &row.install_rel_path,
                row.manifest_sequence,
                &row.files_json,
                report_json.as_deref(),
                row.installed_at,
            )?;
            Ok(VerifyRepairOutcome {
                component: component.to_string(),
                version: Some(version.to_string()),
                intact: true,
                repaired: true,
                error_code: None,
                message: "Quarantined version passed capability verification and is now current"
                    .to_string(),
                capability_report: Some(CapabilityReportView::from(&report)),
            })
        }
        Err(cap_err) => {
            if let Some(rep) = cap_err.report() {
                if let Ok(json) = serde_json::to_string(rep) {
                    let _ = store.update_component_capability_report(
                        &row.component,
                        &row.version,
                        &json,
                    );
                }
            }
            Ok(VerifyRepairOutcome {
                component: component.to_string(),
                version: Some(version.to_string()),
                intact: false,
                repaired: false,
                error_code: Some(cap_err.error_code().to_string()),
                message: format!("Quarantine retry failed: {}", cap_err),
                capability_report: cap_err.report().map(CapabilityReportView::from),
            })
        }
    }
}

/// Repairs a missing, broken, or quarantined component by cleaning stale state and
/// re-downloading + re-verifying from the active manifest.
pub async fn repair_component_from_manifest<F>(
    ws_root: &Path,
    store: &LocalStore,
    in_use: &InUseRegistry,
    entry: &ComponentManifestEntry,
    manifest_sequence: u64,
    on_progress: F,
    cancel_token: Option<&CancellationToken>,
) -> Result<VerifyRepairOutcome, InstallError>
where
    F: Fn(DownloadProgress) + Send + Sync + 'static,
{
    let kind = classify_component_kind(&entry.component)?;
    let existing_quarantined = store
        .get_component_version(&entry.component, &entry.version)?
        .is_some_and(|r| r.status == "quarantined");

    if kind == "bin" {
        let ver_dir = ws_root
            .join("bin")
            .join(&entry.component)
            .join(&entry.version);
        let intact = if ver_dir.exists() {
            fs::read_to_string(ver_dir.join("install.json"))
                .ok()
                .and_then(|s| serde_json::from_str::<InstallReceipt>(&s).ok())
                .map(|r| {
                    check_manifest_matches_receipt(entry, &r)
                        && verify_expected_files_on_disk(&ver_dir, &r.files, true).is_ok()
                })
                .unwrap_or(false)
        } else {
            false
        };

        if existing_quarantined || !intact {
            store.delete_component_version_row(&entry.component, &entry.version)?;
            if ver_dir.exists() {
                let _ = remove_dir_with_deferred_fallback(&ver_dir, store)?;
            }
        }
    } else if kind == "model" {
        let model_path = security::resolve_model_path_with_root(ws_root, &entry.component)?;
        let intact = try_idempotent_model_install(ws_root, store, entry)?.is_some();
        if existing_quarantined || !intact {
            store.delete_component_version_row(&entry.component, &entry.version)?;
            let _ = fs::remove_file(&model_path);
            let _ = fs::remove_file(model_receipt_path(
                &ws_root.join("models"),
                &entry.component,
            ));
        }
    }

    let outcome = download_and_install_component(
        ws_root,
        store,
        in_use,
        entry,
        manifest_sequence,
        on_progress,
        cancel_token,
    )
    .await?;

    Ok(VerifyRepairOutcome {
        component: outcome.component,
        version: Some(outcome.version),
        intact: true,
        repaired: true,
        error_code: None,
        message: if outcome.kind == "model" {
            "Model re-downloaded and hash verified".to_string()
        } else {
            "Component repaired and verified".to_string()
        },
        capability_report: outcome
            .capability_report
            .as_ref()
            .map(CapabilityReportView::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::CapabilityStatus;
    use crate::manifest::ManifestFileEntry;
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    fn sha256_bytes(data: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(data);
        hex::encode(h.finalize())
    }

    fn build_single_zip(name: &str, payload: &[u8]) -> (Vec<u8>, ManifestFileEntry) {
        let mut buf = io::Cursor::new(Vec::new());
        {
            let mut w = ZipWriter::new(&mut buf);
            w.start_file(name, SimpleFileOptions::default()).unwrap();
            w.write_all(payload).unwrap();
            w.finish().unwrap();
        }
        (
            buf.into_inner(),
            ManifestFileEntry {
                path: name.to_string(),
                sha256: sha256_bytes(payload),
                size_bytes: payload.len() as u64,
            },
        )
    }

    fn setup_ws() -> (TempDir, PathBuf, LocalStore, InUseRegistry) {
        let tmp = TempDir::new().unwrap();
        let ws = security::ensure_workspace_with_root(tmp.path()).unwrap();
        let store = LocalStore::open(&ws.join("clipper.db")).unwrap();
        (tmp, ws, store, InUseRegistry::new())
    }

    async fn always_pass_checker(
        comp: String,
        _dir: PathBuf,
        ver: String,
    ) -> Result<CapabilityReport, CapabilityError> {
        Ok(CapabilityReport {
            component: comp,
            version: ver,
            mode: "strict".to_string(),
            status: CapabilityStatus::Pass,
            checked_at: unix_now_secs(),
            reported_version: Some("ok".to_string()),
            reasons: vec![],
            warnings: vec![],
            ffmpeg: None,
        })
    }

    async fn always_fail_gpl_checker(
        comp: String,
        _dir: PathBuf,
        ver: String,
    ) -> Result<CapabilityReport, CapabilityError> {
        let report = CapabilityReport {
            component: comp,
            version: ver,
            mode: "strict".to_string(),
            status: CapabilityStatus::Fail,
            checked_at: unix_now_secs(),
            reported_version: Some("gpl".to_string()),
            reasons: vec!["--enable-gpl detected".to_string()],
            warnings: vec![],
            ffmpeg: None,
        };
        Err(CapabilityError::CapabilityCheckFailed {
            reason: "--enable-gpl detected".to_string(),
            report: Box::new(report),
        })
    }

    fn make_zip_manifest_entry(
        tmp_dir: &Path,
        component: &str,
        version: &str,
        exe_name: &str,
        payload: &[u8],
    ) -> (ComponentManifestEntry, PathBuf) {
        let (zip_bytes, mf) = build_single_zip(exe_name, payload);
        let zip_path = tmp_dir.join(format!("{}-{}.zip", component, version));
        fs::write(&zip_path, &zip_bytes).unwrap();
        let entry = ComponentManifestEntry {
            component: component.to_string(),
            version: version.to_string(),
            platform: "windows-x86_64".to_string(),
            url: "https://cdn.clipperhelper.com/pkg.zip".to_string(),
            mirrors: vec![],
            size_bytes: zip_bytes.len() as u64,
            sha256: sha256_bytes(&zip_bytes),
            archive: "zip".to_string(),
            files: vec![mf],
            min_app_version: "0.1.0".to_string(),
            max_app_version: None,
            rollout_percent: 100,
        };
        (entry, zip_path)
    }

    #[tokio::test]
    async fn test_atomic_install_reinstall_idempotent_and_side_by_side_switch() {
        let (tmp, ws, store, in_use) = setup_ws();
        let (entry1, pkg1) =
            make_zip_manifest_entry(tmp.path(), "ffmpeg", "v1.0.0", "ffmpeg.exe", b"bin-v1");

        let out1 = install_verified_artifact_with_options(
            &ws,
            &store,
            &in_use,
            &entry1,
            1,
            &pkg1,
            &ArchiveLimits::default(),
            None,
            Some(always_pass_checker),
        )
        .await
        .expect("install v1");
        assert!(!out1.already_installed);
        assert!(out1.install_path.join("install.json").is_file());

        // Same-version reinstall is idempotent
        let out1_again = install_verified_artifact_with_options(
            &ws,
            &store,
            &in_use,
            &entry1,
            1,
            &pkg1,
            &ArchiveLimits::default(),
            None,
            Some(always_fail_gpl_checker), // should not even need re-extraction
        )
        .await
        .expect("idempotent reinstall");
        assert!(out1_again.already_installed);

        // Hold an active lease on v1 while installing v2 side by side
        let _lease_v1 = in_use.acquire("ffmpeg", "v1.0.0");
        let (entry2, pkg2) =
            make_zip_manifest_entry(tmp.path(), "ffmpeg", "v2.0.0", "ffmpeg.exe", b"bin-v2");
        let out2 = install_verified_artifact_with_options(
            &ws,
            &store,
            &in_use,
            &entry2,
            2,
            &pkg2,
            &ArchiveLimits::default(),
            None,
            Some(always_pass_checker),
        )
        .await
        .expect("install v2 side by side");
        assert!(!out2.already_installed);
        // Both version dirs coexist cleanly on disk
        assert!(ws.join("bin/ffmpeg/v1.0.0/ffmpeg.exe").is_file());
        assert!(ws.join("bin/ffmpeg/v2.0.0/ffmpeg.exe").is_file());
        assert_eq!(
            store
                .get_current_component("ffmpeg")
                .unwrap()
                .unwrap()
                .version,
            "v2.0.0"
        );
        assert_eq!(
            store
                .get_previous_component("ffmpeg")
                .unwrap()
                .unwrap()
                .version,
            "v1.0.0"
        );

        // Rollback v2 -> v1 and roll forward v1 -> v2
        let rb = rollback_component(&ws, &store, "ffmpeg", None).unwrap();
        assert_eq!(rb, "v1.0.0");
        assert_eq!(
            store
                .get_current_component("ffmpeg")
                .unwrap()
                .unwrap()
                .version,
            "v1.0.0"
        );
        let rf = rollback_component(&ws, &store, "ffmpeg", Some("v2.0.0")).unwrap();
        assert_eq!(rf, "v2.0.0");
    }

    #[tokio::test]
    async fn test_rollback_after_failed_capability_check_and_quarantine() {
        let (tmp, ws, store, in_use) = setup_ws();
        let (entry1, pkg1) =
            make_zip_manifest_entry(tmp.path(), "ffmpeg", "good-v1", "ffmpeg.exe", b"lgpl-bin");
        install_verified_artifact_with_options(
            &ws,
            &store,
            &in_use,
            &entry1,
            1,
            &pkg1,
            &ArchiveLimits::default(),
            None,
            Some(always_pass_checker),
        )
        .await
        .expect("install good v1");

        let (entry_bad, pkg_bad) =
            make_zip_manifest_entry(tmp.path(), "ffmpeg", "gpl-v2", "ffmpeg.exe", b"gpl-bin");
        let err = install_verified_artifact_with_options(
            &ws,
            &store,
            &in_use,
            &entry_bad,
            2,
            &pkg_bad,
            &ArchiveLimits::default(),
            None,
            Some(always_fail_gpl_checker),
        )
        .await
        .unwrap_err();

        match err {
            InstallError::CapabilityFailed {
                rolled_back_to,
                reason,
                ..
            } => {
                assert_eq!(rolled_back_to.as_deref(), Some("good-v1"));
                assert!(reason.contains("--enable-gpl"));
            }
            other => panic!("Expected CapabilityFailed, got {:?}", other),
        }

        // Current must still be good-v1; gpl-v2 must be quarantined
        assert_eq!(
            store
                .get_current_component("ffmpeg")
                .unwrap()
                .unwrap()
                .version,
            "good-v1"
        );
        let bad_row = store
            .get_component_version("ffmpeg", "gpl-v2")
            .unwrap()
            .unwrap();
        assert_eq!(bad_row.status, "quarantined");
        assert!(!bad_row.is_current);
        assert!(bad_row.capability_report_json.is_some());
    }

    #[tokio::test]
    async fn test_gc_keeps_n_versions_and_skips_in_use_and_defers_locked_delete() {
        let (tmp, ws, store, in_use) = setup_ws();
        for (idx, ver) in ["v1", "v2", "v3", "v4"].iter().enumerate() {
            let payload = format!("bin-{}", ver);
            let (e, p) = make_zip_manifest_entry(
                tmp.path(),
                "yt-dlp",
                ver,
                "yt-dlp.exe",
                payload.as_bytes(),
            );
            install_verified_artifact_with_options(
                &ws,
                &store,
                &in_use,
                &e,
                (idx + 1) as u64,
                &p,
                &ArchiveLimits::default(),
                None,
                Some(always_pass_checker),
            )
            .await
            .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        // Installing v1..v4 runs auto-GC keeping current (v4) + N=2 previous (v3, v2) and removing v1.
        assert!(!ws.join("bin/yt-dlp/v1").exists());
        assert!(ws.join("bin/yt-dlp/v2").is_dir());
        assert!(ws.join("bin/yt-dlp/v3").is_dir());
        assert!(ws.join("bin/yt-dlp/v4").is_dir());

        // Hold active lease on v2 while installing v5 -> GC (N=2 keeps v4, v3) MUST skip in-use v2!
        let _lease_v2 = in_use.acquire("yt-dlp", "v2");
        let (e5, p5) = make_zip_manifest_entry(tmp.path(), "yt-dlp", "v5", "yt-dlp.exe", b"bin-v5");
        install_verified_artifact_with_options(
            &ws,
            &store,
            &in_use,
            &e5,
            5,
            &p5,
            &ArchiveLimits::default(),
            None,
            Some(always_pass_checker),
        )
        .await
        .unwrap();

        assert!(ws.join("bin/yt-dlp/v5").is_dir());
        assert!(ws.join("bin/yt-dlp/v4").is_dir());
        assert!(ws.join("bin/yt-dlp/v3").is_dir());
        assert!(
            ws.join("bin/yt-dlp/v2").is_dir(),
            "in-use v2 must be skipped by auto-GC"
        );

        let gc_in_use = gc_component_versions(&ws, &store, "yt-dlp", 2, &in_use).unwrap();
        assert!(gc_in_use.skipped_in_use.contains(&"v2".to_string()));
        assert!(ws.join("bin/yt-dlp/v2").is_dir());
        drop(_lease_v2);

        // Now test Windows locked file -> deferred delete -> startup retry!
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let lock_target = ws.join("bin/yt-dlp/v2/yt-dlp.exe");
            let locked_handle = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(0) // Exclusive lock: blocks deletion on Windows
                .open(&lock_target)
                .expect("open exclusive handle");

            // GC(N=2) keeps v5(current), v4, v3 -> tries deleting v2 -> locked -> deferred!
            let gc_locked = gc_component_versions(&ws, &store, "yt-dlp", 2, &in_use).unwrap();
            assert!(gc_locked.deferred_versions.contains(&"v2".to_string()));
            assert!(!store.list_deferred_deletes().unwrap().is_empty());
            assert!(ws.join("bin/yt-dlp/v2").exists());

            // Release lock, run startup reconciliation -> deferred dir deleted!
            drop(locked_handle);
            let rec = reconcile_startup(&ws, &store).unwrap();
            assert!(rec.deferred_deletes_succeeded >= 1);
            assert_eq!(rec.deferred_deletes_remaining, 0);
            assert!(!ws.join("bin/yt-dlp/v2").exists());
        }
    }

    #[tokio::test]
    async fn test_startup_reconciliation_crash_and_corruption_cases() {
        let (tmp, ws, store, in_use) = setup_ws();

        // Install v1 and v2 cleanly
        let (e1, p1) = make_zip_manifest_entry(tmp.path(), "qjs", "v1", "qjs.exe", b"qjs1");
        let (e2, p2) = make_zip_manifest_entry(tmp.path(), "qjs", "v2", "qjs.exe", b"qjs2");
        install_verified_artifact_with_options(
            &ws,
            &store,
            &in_use,
            &e1,
            1,
            &p1,
            &ArchiveLimits::default(),
            None,
            Some(always_pass_checker),
        )
        .await
        .unwrap();
        install_verified_artifact_with_options(
            &ws,
            &store,
            &in_use,
            &e2,
            2,
            &p2,
            &ArchiveLimits::default(),
            None,
            Some(always_pass_checker),
        )
        .await
        .unwrap();

        // 1. Kill during extraction -> orphan `.staging-*` dir deleted at startup
        let orphan_stage = ws.join("bin/qjs/.staging-deadbeef");
        fs::create_dir_all(&orphan_stage).unwrap();
        fs::write(orphan_stage.join("partial.bin"), b"half").unwrap();

        // 2. Kill between rename and pointer switch -> unrecorded `v3` folder on disk cleaned up while `v2` stays current
        let uncommitted_dir = ws.join("bin/qjs/v3");
        fs::create_dir_all(&uncommitted_dir).unwrap();
        fs::write(uncommitted_dir.join("qjs.exe"), b"uncommitted").unwrap();

        let rec1 = reconcile_startup(&ws, &store).unwrap();
        assert_eq!(rec1.orphan_staging_removed, 1);
        assert_eq!(rec1.uncommitted_versions_removed, 1);
        assert!(!orphan_stage.exists());
        assert!(!uncommitted_dir.exists());
        assert_eq!(
            store.get_current_component("qjs").unwrap().unwrap().version,
            "v2"
        );

        // 3. Corrupted `install.json` on current (v2) -> startup falls back cleanly to previous (v1)
        fs::write(ws.join("bin/qjs/v2/install.json"), b"{corrupt json").unwrap();
        let rec2 = reconcile_startup(&ws, &store).unwrap();
        assert_eq!(rec2.fallbacks_activated.len(), 1);
        assert_eq!(
            store.get_current_component("qjs").unwrap().unwrap().version,
            "v1"
        );

        // 4. Vanished executable while `current` directory exists (e.g. antivirus removed `v1/qjs.exe`)
        fs::remove_file(ws.join("bin/qjs/v1/qjs.exe")).unwrap();
        let rec3 = reconcile_startup(&ws, &store).unwrap();
        assert_eq!(rec3.missing_components.len(), 1);
        assert_eq!(rec3.missing_components[0].code, "E_COMPONENT_MISSING");
        assert!(rec3.missing_components[0].mentions_antivirus);
        assert!(rec3.missing_components[0]
            .message
            .to_lowercase()
            .contains("antivirus"));
        assert!(rec3.missing_components[0].repair_action);

        // 5. Pointer to a missing directory while `previous` is intact -> automatic fallback to `previous`
        let (yt1, yt_p1) =
            make_zip_manifest_entry(tmp.path(), "yt-dlp", "v1", "yt-dlp.exe", b"yt1");
        let (yt2, yt_p2) =
            make_zip_manifest_entry(tmp.path(), "yt-dlp", "v2", "yt-dlp.exe", b"yt2");
        install_verified_artifact_with_options(
            &ws,
            &store,
            &in_use,
            &yt1,
            1,
            &yt_p1,
            &ArchiveLimits::default(),
            None,
            Some(always_pass_checker),
        )
        .await
        .unwrap();
        install_verified_artifact_with_options(
            &ws,
            &store,
            &in_use,
            &yt2,
            2,
            &yt_p2,
            &ArchiveLimits::default(),
            None,
            Some(always_pass_checker),
        )
        .await
        .unwrap();
        fs::remove_dir_all(ws.join("bin/yt-dlp/v2")).unwrap();
        let rec4 = reconcile_startup(&ws, &store).unwrap();
        assert!(rec4
            .fallbacks_activated
            .iter()
            .any(|msg| msg.contains("yt-dlp") && msg.contains("v1")));
        assert_eq!(
            store
                .get_current_component("yt-dlp")
                .unwrap()
                .unwrap()
                .version,
            "v1"
        );

        // 6. Pointer to a completely missing directory with no intact fallback -> E_COMPONENT_MISSING
        fs::remove_dir_all(ws.join("bin/yt-dlp/v1")).unwrap();
        let rec5 = reconcile_startup(&ws, &store).unwrap();
        assert_eq!(rec5.missing_components.len(), 1);
        assert_eq!(rec5.missing_components[0].component, "yt-dlp");
        assert_eq!(rec5.missing_components[0].code, "E_COMPONENT_MISSING");
        assert!(rec5.missing_components[0].mentions_antivirus);
        assert!(rec5.missing_components[0].repair_action);
    }

    #[tokio::test]
    async fn test_db_corruption_rebuild_from_install_json() {
        let (tmp, ws, store, in_use) = setup_ws();
        let (e1, p1) =
            make_zip_manifest_entry(tmp.path(), "yt-dlp", "2026.10.26", "yt-dlp.exe", b"yt1");
        install_verified_artifact_with_options(
            &ws,
            &store,
            &in_use,
            &e1,
            5,
            &p1,
            &ArchiveLimits::default(),
            None,
            Some(always_pass_checker),
        )
        .await
        .unwrap();

        // Also place one corrupted version dir alongside good one
        let corrupt_ver = ws.join("bin/yt-dlp/2026.01.01");
        fs::create_dir_all(&corrupt_ver).unwrap();
        fs::write(corrupt_ver.join("install.json"), b"not-json").unwrap();

        // Corrupt SQLite DB file directly on disk and reopen
        drop(store);
        fs::write(ws.join("clipper.db"), b"corrupt database bytes!").unwrap();
        let rec_err = LocalStore::open(&ws.join("clipper.db")).err().unwrap();
        assert!(matches!(rec_err, StoreError::CorruptRecovered(_)));

        let fresh_store = LocalStore::open(&ws.join("clipper.db")).unwrap();
        let rec = reconcile_startup(&ws, &fresh_store).unwrap();
        assert!(rec.rebuilt_from_receipts);
        let cur = fresh_store
            .get_current_component("yt-dlp")
            .unwrap()
            .expect("rebuilt active row from install.json");
        assert_eq!(cur.version, "2026.10.26");
        assert_eq!(cur.manifest_sequence, 5);
    }

    #[tokio::test]
    async fn test_disk_full_and_long_path_and_model_install() {
        let (tmp, ws, store, in_use) = setup_ws();

        // 1. Disk-full during install -> E_DISK_FULL
        let (e1, p1) = make_zip_manifest_entry(tmp.path(), "qjs", "v1", "qjs.exe", &[0x55u8; 4096]);
        let err_full = install_verified_artifact_with_options(
            &ws,
            &store,
            &in_use,
            &e1,
            1,
            &p1,
            &ArchiveLimits::default(),
            Some(512),
            Some(always_pass_checker),
        )
        .await
        .unwrap_err();
        assert_eq!(err_full.error_code(), "E_DISK_FULL");
        assert!(store.get_current_component("qjs").unwrap().is_none());

        // 2. Long-path version/root rejection before creating target
        let long_limits = ArchiveLimits {
            max_full_path_len: 30,
            ..ArchiveLimits::default()
        };
        let err_long = install_verified_artifact_with_options(
            &ws,
            &store,
            &in_use,
            &e1,
            1,
            &p1,
            &long_limits,
            None,
            Some(always_pass_checker),
        )
        .await
        .unwrap_err();
        assert!(matches!(err_long, InstallError::PathTooLong(_)));

        // 3. Single-file speech model atomic install into `models/` & tracked in `components`
        let model_bytes = vec![0x99u8; 1024];
        let model_pkg = tmp.path().join("ggml-tiny.bin");
        fs::write(&model_pkg, &model_bytes).unwrap();
        let model_entry = ComponentManifestEntry {
            component: "ggml-tiny.bin".into(),
            version: "1.0.0".into(),
            platform: "windows-x86_64".into(),
            url: "https://cdn.clipperhelper.com/ggml-tiny.bin".into(),
            mirrors: vec![],
            size_bytes: model_bytes.len() as u64,
            sha256: sha256_bytes(&model_bytes),
            archive: "none".into(),
            files: vec![],
            min_app_version: "0.1.0".into(),
            max_app_version: None,
            rollout_percent: 100,
        };
        let out_m = install_verified_artifact_with_options(
            &ws,
            &store,
            &in_use,
            &model_entry,
            1,
            &model_pkg,
            &ArchiveLimits::default(),
            None,
            Some(always_pass_checker),
        )
        .await
        .expect("model install");
        assert_eq!(out_m.kind, "model");
        assert!(ws.join("models/ggml-tiny.bin").is_file());
        let row_m = store
            .get_current_component("ggml-tiny.bin")
            .unwrap()
            .expect("tracked in same components table");
        assert_eq!(row_m.kind, "model");

        // Vanished model file detected by verify_active_component_full -> E_COMPONENT_MISSING mentioning antivirus
        fs::remove_file(ws.join("models/ggml-tiny.bin")).unwrap();
        let vr = verify_active_component_full(&ws, &store, "ggml-tiny.bin").await;
        assert!(!vr.intact);
        assert_eq!(vr.error_code.as_deref(), Some("E_COMPONENT_MISSING"));
        assert!(vr.message.to_lowercase().contains("antivirus"));
    }

    #[tokio::test]
    async fn test_quarantine_retry_and_repair_and_temp_cleanup() {
        let (tmp, ws, store, in_use) = setup_ws();

        // 1. Seed stale temp files under cache/temp -> reconcile_startup cleans them cleanly.
        let stale_temp = ws.join("cache").join("temp").join("yt-dlp").join("_MEI123");
        fs::create_dir_all(&stale_temp).unwrap();
        fs::write(stale_temp.join("old.tmp"), b"stale").unwrap();
        reconcile_startup(&ws, &store).unwrap();
        assert!(!stale_temp.exists());

        // 2. Quarantined version: retry fails when binary bad, detects E_COMPONENT_MISSING when deleted
        let (e1, p1) = make_zip_manifest_entry(tmp.path(), "qjs", "v0.18.0", "qjs.exe", b"bad-qjs");
        let _ = install_verified_artifact_with_options(
            &ws,
            &store,
            &in_use,
            &e1,
            1,
            &p1,
            &ArchiveLimits::default(),
            None,
            Some(always_fail_gpl_checker),
        )
        .await
        .unwrap_err();

        let retry_res = retry_quarantined_component_verification(&ws, &store, "qjs", &e1.version)
            .await
            .unwrap();
        assert!(!retry_res.intact);

        // Missing files inside quarantined dir -> retry detects E_COMPONENT_MISSING mentioning antivirus
        fs::remove_file(ws.join("bin/qjs").join(&e1.version).join("qjs.exe")).unwrap();
        let retry_missing =
            retry_quarantined_component_verification(&ws, &store, "qjs", &e1.version)
                .await
                .unwrap();
        assert!(!retry_missing.intact);
        assert_eq!(
            retry_missing.error_code.as_deref(),
            Some("E_COMPONENT_MISSING")
        );
        assert!(retry_missing.message.to_lowercase().contains("antivirus"));
    }
}
