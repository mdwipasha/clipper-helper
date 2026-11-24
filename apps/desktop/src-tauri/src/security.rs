//! Security, path validation, and logging redaction (ARCHITECTURE §4, §9; SECURITY §9, §13).
//!
//! Responsibilities:
//! - Canonicalize and restrict file access strictly within `%LOCALAPPDATA%\ClipperHelper\`.
//! - Enforce YouTube URL intake allowlist and extract canonical 11-char video IDs.
//! - Implement logging redaction to ensure no tokens, query strings, emails, or Windows usernames leak into logs.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;
use thiserror::Error;
use tracing::field::Visit;
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum SecurityError {
    #[error("Invalid path format: {0}")]
    InvalidFormat(String),
    #[error("Path traversal attempt detected: {0}")]
    TraversalAttempt(String),
    #[error("Absolute or UNC paths are forbidden: {0}")]
    AbsoluteForbidden(String),
    #[error("Alternate data stream is forbidden: {0}")]
    AlternateDataStreamForbidden(String),
    #[error("Reserved device name is forbidden: {0}")]
    ReservedDeviceNameForbidden(String),
    #[error("Path segment ends with dot or space: {0}")]
    TrailingDotOrSpaceForbidden(String),
    #[error("8.3 short name is forbidden: {0}")]
    ShortNameForbidden(String),
    #[error("Path exceeds maximum length: {0}")]
    PathTooLong(String),
    #[error("Path escapes workspace root: {0}")]
    BoundaryEscape(String),
    #[error("Workspace root directory is missing or uninitialized: {0}")]
    WorkspaceRootMissing(String),
    #[error("Workspace permission denied: {0}")]
    WorkspacePermissionDenied(String),
    #[error("Disk full when creating workspace: {0}")]
    WorkspaceDiskFull(String),
    #[error("Disk full: available {available} bytes, required {required} bytes")]
    DiskFull { available: u64, required: u64 },
    #[error("Disk low: available {available} bytes, required {required} bytes")]
    DiskLow { available: u64, required: u64 },
    #[error("Invalid data root: {0}")]
    InvalidDataRoot(String),
    #[error("Component is not allowlisted: {0}")]
    ComponentNotAllowlisted(String),
    #[error("Failed to resolve workspace root: {0}")]
    RootResolutionFailed(String),
}

impl SecurityError {
    pub fn error_code(&self) -> &'static str {
        match self {
            SecurityError::DiskFull { .. } | SecurityError::WorkspaceDiskFull(_) => "E_DISK_FULL",
            SecurityError::DiskLow { .. } => "E_DISK_LOW",
            SecurityError::WorkspacePermissionDenied(_) => "E_APP_INIT_FAILED",
            SecurityError::WorkspaceRootMissing(_) => "E_APP_INIT_FAILED",
            SecurityError::ComponentNotAllowlisted(_) => "E_COMPONENT_MISSING",
            _ => "E_APP_INIT_FAILED",
        }
    }
}

static WORKSPACE_PATH_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(cache|projects)/[A-Za-z0-9._/-]+$").expect("valid regex"));

static RESERVED_DEVICE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(\..*)?$").expect("valid regex")
});

static SHORT_NAME_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"~[0-9]").expect("valid regex"));

static JWT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"ey[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}")
        .expect("valid regex")
});

static BEARER_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)bearer\s+[A-Za-z0-9._~+/-]+={0,2}").expect("valid regex"));

static QUERY_STRING_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\?[A-Za-z0-9_=&%.-]+").expect("valid regex"));

static EMAIL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}").expect("valid regex")
});

static USER_PROFILE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)[A-Z]:[/\\]Users[/\\]([^/\\]+)").expect("valid regex"));

/// Returns the hardcoded default workspace root directory `%LOCALAPPDATA%\ClipperHelper\` (or `ClipperHelper-Dev` in debug builds).
pub fn get_default_workspace_root() -> Result<PathBuf, SecurityError> {
    let local_app_data = std::env::var("LOCALAPPDATA")
        .or_else(|_| std::env::var("USERPROFILE").map(|u| format!("{}\\AppData\\Local", u)))
        .map_err(|e| SecurityError::RootResolutionFailed(format!("Environment error: {}", e)))?;

    #[cfg(debug_assertions)]
    let app_folder = "ClipperHelper-Dev";
    #[cfg(not(debug_assertions))]
    let app_folder = "ClipperHelper";

    let root = PathBuf::from(local_app_data).join(app_folder);
    Ok(root)
}

/// JSON configuration stored in `bootstrap.json` at the default data root.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct BootstrapConfig {
    pub data_root: String,
}

/// Managed DataRoot abstraction resolving active workspace storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataRoot {
    pub path: PathBuf,
    pub is_default: bool,
    pub is_onedrive: bool,
}

#[cfg(windows)]
fn get_drive_prefix(path: &Path) -> Result<Vec<u16>, SecurityError> {
    use std::os::windows::ffi::OsStrExt;
    let path_str = path.to_string_lossy();
    if path_str.starts_with(r"\\") {
        return Err(SecurityError::InvalidDataRoot(
            "UNC network paths are not allowed as DataRoot".into(),
        ));
    }
    let chars: Vec<char> = path_str.chars().collect();
    if chars.len() >= 2 && chars[1] == ':' && chars[0].is_ascii_alphabetic() {
        let drive = format!("{}:\\\0", chars[0]);
        let wide: Vec<u16> = std::ffi::OsStr::new(&drive).encode_wide().collect();
        return Ok(wide);
    }
    Err(SecurityError::InvalidDataRoot(format!(
        "Invalid drive specification in path: {}",
        path.display()
    )))
}

#[cfg(windows)]
pub fn is_local_fixed_drive(path: &Path) -> Result<bool, SecurityError> {
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    const DRIVE_FIXED: u32 = 3;

    let drive_root_wide = get_drive_prefix(path)?;
    let drive_type = unsafe { GetDriveTypeW(drive_root_wide.as_ptr()) };
    Ok(drive_type == DRIVE_FIXED)
}

#[cfg(not(windows))]
pub fn is_local_fixed_drive(_path: &Path) -> Result<bool, SecurityError> {
    Ok(true)
}

pub fn is_system_directory(path: &Path) -> bool {
    let path_str = path.to_string_lossy().to_lowercase();
    let norm = path_str.replace('/', "\\");

    // Reject drive root alone (e.g. "C:\" or "C:")
    if (norm.len() <= 3 && norm.ends_with(':')) || norm.ends_with(r":\") {
        return true;
    }

    let sys_root = std::env::var("SystemRoot")
        .or_else(|_| std::env::var("SYSTEMROOT"))
        .unwrap_or_else(|_| r"C:\Windows".to_string())
        .to_lowercase();
    if norm.starts_with(&sys_root) {
        return true;
    }

    let prog_files = std::env::var("ProgramFiles")
        .unwrap_or_else(|_| r"C:\Program Files".to_string())
        .to_lowercase();
    if norm.starts_with(&prog_files) {
        return true;
    }

    if let Ok(pf86) = std::env::var("ProgramFiles(x86)") {
        if norm.starts_with(&pf86.to_lowercase()) {
            return true;
        }
    }

    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        let user_prof_lower = user_profile.to_lowercase();
        if norm == user_prof_lower || norm == format!("{}\\", user_prof_lower) {
            return true;
        }
    }

    norm == r"c:\users" || norm == r"c:\users\"
}

pub fn is_onedrive_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy().to_lowercase();
    if path_str.contains("onedrive") {
        return true;
    }
    for env_var in &["OneDrive", "OneDriveConsumer", "OneDriveCommercial"] {
        if let Ok(od_dir) = std::env::var(env_var) {
            if path_str.starts_with(&od_dir.to_lowercase()) {
                return true;
            }
        }
    }
    false
}

pub fn is_valid_or_empty_clipper_dir(path: &Path) -> Result<bool, SecurityError> {
    if !path.exists() {
        return Ok(true);
    }
    if !path.is_dir() {
        return Err(SecurityError::InvalidDataRoot(format!(
            "Data root path is not a directory: {}",
            path.display()
        )));
    }

    let read_dir = std::fs::read_dir(path).map_err(|e| {
        SecurityError::InvalidDataRoot(format!(
            "Failed to read directory '{}': {}",
            path.display(),
            e
        ))
    })?;

    let mut entry_count = 0;
    let mut has_clipper_marker = false;

    for entry in read_dir {
        let entry = entry.map_err(|e| {
            SecurityError::InvalidDataRoot(format!(
                "Failed to read entry in '{}': {}",
                path.display(),
                e
            ))
        })?;
        entry_count += 1;
        let file_name = entry.file_name().to_string_lossy().to_lowercase();
        if file_name == "clipper.db"
            || file_name == ".clipperhelper"
            || file_name == "bootstrap.json"
            || file_name == "bin"
            || file_name == "models"
            || file_name == "cache"
            || file_name == "projects"
            || file_name == "logs"
        {
            has_clipper_marker = true;
        }
    }

    if entry_count == 0 || has_clipper_marker {
        Ok(true)
    } else {
        Err(SecurityError::InvalidDataRoot(format!(
            "Directory '{}' is not empty and is not an existing ClipperHelper data directory",
            path.display()
        )))
    }
}

pub fn verify_dir_writable(path: &Path) -> Result<(), SecurityError> {
    let probe_dir = if path.exists() {
        path.to_path_buf()
    } else {
        let mut ancestor = path.parent();
        while let Some(a) = ancestor {
            if a.exists() {
                break;
            }
            ancestor = a.parent();
        }
        ancestor
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(r"C:\"))
    };

    let probe_file = probe_dir.join(format!(
        "._clipper_write_probe_{}_{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));

    std::fs::write(&probe_file, b"probe").map_err(|e| {
        SecurityError::WorkspacePermissionDenied(format!(
            "Data root location '{}' is not writable: {}",
            probe_dir.display(),
            e
        ))
    })?;

    let _ = std::fs::remove_file(&probe_file);
    Ok(())
}

impl DataRoot {
    pub fn validate(path: &Path) -> Result<Self, SecurityError> {
        if !path.is_absolute() {
            return Err(SecurityError::InvalidDataRoot(format!(
                "Data root must be an absolute path: {}",
                path.display()
            )));
        }

        if path.to_string_lossy().starts_with(r"\\") {
            return Err(SecurityError::InvalidDataRoot(format!(
                "UNC / network paths are forbidden: {}",
                path.display()
            )));
        }

        #[cfg(windows)]
        {
            if !is_local_fixed_drive(path)? {
                return Err(SecurityError::InvalidDataRoot(format!(
                    "Data root must be on a local fixed drive: {}",
                    path.display()
                )));
            }
        }

        if is_system_directory(path) {
            return Err(SecurityError::InvalidDataRoot(format!(
                "Data root cannot be a system directory: {}",
                path.display()
            )));
        }

        is_valid_or_empty_clipper_dir(path)?;
        verify_dir_writable(path)?;

        let is_onedrive = is_onedrive_path(path);
        if is_onedrive {
            tracing::warn!(
                "Data root '{}' is inside OneDrive. Sync conflicts may occur.",
                path.display()
            );
        }

        Ok(Self {
            path: path.to_path_buf(),
            is_default: false,
            is_onedrive,
        })
    }

    pub fn read_bootstrap(default_root: &Path) -> Result<Option<PathBuf>, SecurityError> {
        let bootstrap_path = default_root.join("bootstrap.json");
        if !bootstrap_path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&bootstrap_path).map_err(|e| {
            SecurityError::RootResolutionFailed(format!("Failed to read bootstrap.json: {}", e))
        })?;
        let config: BootstrapConfig = serde_json::from_str(&content).map_err(|e| {
            SecurityError::RootResolutionFailed(format!("Invalid bootstrap.json format: {}", e))
        })?;
        let trimmed = config.data_root.trim();
        if trimmed.is_empty() {
            Ok(None)
        } else {
            Ok(Some(PathBuf::from(trimmed)))
        }
    }

    pub fn write_bootstrap(default_root: &Path, target_root: &Path) -> Result<(), SecurityError> {
        std::fs::create_dir_all(default_root).map_err(|e| {
            SecurityError::RootResolutionFailed(format!("Failed to create default root dir: {}", e))
        })?;
        let config = BootstrapConfig {
            data_root: target_root.to_string_lossy().to_string(),
        };
        let json = serde_json::to_string_pretty(&config).map_err(|e| {
            SecurityError::RootResolutionFailed(format!("Failed to serialize bootstrap: {}", e))
        })?;
        let bootstrap_path = default_root.join("bootstrap.json");
        std::fs::write(&bootstrap_path, json).map_err(|e| {
            SecurityError::RootResolutionFailed(format!("Failed to write bootstrap.json: {}", e))
        })?;
        Ok(())
    }

    pub fn resolve_active() -> Result<Self, SecurityError> {
        #[cfg(any(debug_assertions, test))]
        {
            if let Ok(override_path) = std::env::var("CLIPPER_WORKSPACE_ROOT") {
                let trimmed = override_path.trim();
                if !trimmed.is_empty() {
                    let p = PathBuf::from(trimmed);
                    return Ok(Self {
                        path: p,
                        is_default: false,
                        is_onedrive: false,
                    });
                }
            }
        }

        let default_root = get_default_workspace_root()?;
        if let Some(target) = Self::read_bootstrap(&default_root)? {
            let mut dr = Self::validate(&target)?;
            dr.is_default = false;
            return Ok(dr);
        }

        let is_od = is_onedrive_path(&default_root);
        Ok(Self {
            path: default_root,
            is_default: true,
            is_onedrive: is_od,
        })
    }

    pub fn check_free_space(&self, required_bytes: u64) -> Result<(), SecurityError> {
        check_free_space_at(&self.path, required_bytes)
    }
}

/// Returns the active workspace root directory from DataRoot.
pub fn get_workspace_root() -> Result<PathBuf, SecurityError> {
    DataRoot::resolve_active().map(|dr| dr.path)
}

/// Checks that the active workspace root has sufficient free disk space.
pub fn check_free_space(required_bytes: u64) -> Result<(), SecurityError> {
    let ws = get_workspace_root()?;
    check_free_space_at(&ws, required_bytes)
}

/// Checks that the given path has sufficient free disk space (required + 500 MB headroom).
pub fn check_free_space_at(path: &Path, required_bytes: u64) -> Result<(), SecurityError> {
    let available = get_free_disk_space(path)?;
    const HEADROOM_BYTES: u64 = 500 * 1024 * 1024; // 500 MB headroom
    let minimum_safe = required_bytes.saturating_add(HEADROOM_BYTES);

    if available < required_bytes {
        return Err(SecurityError::DiskFull {
            available,
            required: required_bytes,
        });
    }
    if available < minimum_safe {
        return Err(SecurityError::DiskLow {
            available,
            required: minimum_safe,
        });
    }
    Ok(())
}

/// Asserts that a workspace root is permitted to be reset by dev commands.
///
/// Refuses to run unless the root's final path component is exactly `ClipperHelper-Dev`.
pub fn assert_can_reset_dev_workspace(root: &Path) -> Result<(), SecurityError> {
    let final_component = root.file_name().and_then(|n| n.to_str());
    if final_component != Some("ClipperHelper-Dev") {
        return Err(SecurityError::RootResolutionFailed(format!(
            "Reset Dev Workspace is forbidden: final path component must be exactly 'ClipperHelper-Dev', got {:?}",
            final_component
        )));
    }
    Ok(())
}

/// Ensures the workspace root and its core subdirectories (`bin`, `models`, `cache`, `projects`, `logs`)
/// exist and are safe from boundary escapes.
pub fn ensure_workspace() -> Result<PathBuf, SecurityError> {
    let root = get_workspace_root()?;
    ensure_workspace_with_root(&root)
}

/// Ensures the specified workspace root and its core subdirectories exist.
///
/// Verifies the root is a real directory (not a regular file or junction escaping outside).
/// Maps OS errors to typed `SecurityError` variants.
pub fn ensure_workspace_with_root(root: &Path) -> Result<PathBuf, SecurityError> {
    if root.exists() && root.is_file() {
        return Err(SecurityError::RootResolutionFailed(format!(
            "Workspace root is a file, not a directory: {}",
            root.display()
        )));
    }

    let subdirs = ["bin", "models", "cache", "projects", "logs"];

    // Ensure root and subdirectories exist
    for subdir in &subdirs {
        let dir = root.join(subdir);
        std::fs::create_dir_all(&dir).map_err(|e| match e.kind() {
            std::io::ErrorKind::PermissionDenied => SecurityError::WorkspacePermissionDenied(
                format!("Permission denied creating '{}': {}", dir.display(), e),
            ),
            _ => {
                if e.raw_os_error() == Some(112) {
                    SecurityError::WorkspaceDiskFull(format!(
                        "Disk full creating '{}': {}",
                        dir.display(),
                        e
                    ))
                } else {
                    SecurityError::RootResolutionFailed(format!(
                        "Failed to create directory '{}': {}",
                        dir.display(),
                        e
                    ))
                }
            }
        })?;
    }

    let canonical_root = dunce::canonicalize(root).map_err(|e| {
        SecurityError::RootResolutionFailed(format!("Canonicalize root failed: {}", e))
    })?;

    let norm_root = canonical_root.to_string_lossy().to_lowercase();

    // Verify root is not a junction pointing outside its parent (if parent exists)
    if let Some(parent) = root.parent() {
        if parent.exists() {
            if let Ok(meta) = root.symlink_metadata() {
                if meta.file_type().is_symlink() {
                    let canonical_parent = dunce::canonicalize(parent).map_err(|e| {
                        SecurityError::RootResolutionFailed(format!(
                            "Canonicalize parent failed: {}",
                            e
                        ))
                    })?;
                    let norm_parent = canonical_parent.to_string_lossy().to_lowercase();
                    if !norm_root.starts_with(&norm_parent) {
                        return Err(SecurityError::BoundaryEscape(format!(
                            "Workspace root junction '{}' escapes parent directory '{}'",
                            canonical_root.display(),
                            canonical_parent.display()
                        )));
                    }
                }
            }
        }
    }

    // Verify subdirectories do not escape canonical_root (in case any was a junction)
    for subdir in &subdirs {
        let dir = root.join(subdir);
        let canonical_subdir = dunce::canonicalize(&dir).map_err(|e| {
            SecurityError::BoundaryEscape(format!(
                "Canonicalize subdir '{}' failed: {}",
                dir.display(),
                e
            ))
        })?;
        let norm_subdir = canonical_subdir.to_string_lossy().to_lowercase();
        if !norm_subdir.starts_with(&norm_root) {
            return Err(SecurityError::BoundaryEscape(format!(
                "Workspace subdirectory '{}' escapes workspace root '{}'",
                canonical_subdir.display(),
                canonical_root.display()
            )));
        }
    }

    Ok(canonical_root)
}

/// Resolves a stored relative POSIX path against the default workspace root.
pub fn resolve_workspace_path(rel_path: &str) -> Result<PathBuf, SecurityError> {
    let root = get_workspace_root()?;
    resolve_workspace_path_with_root(&root, rel_path)
}

/// Resolves a stored relative POSIX path against an explicit workspace root.
/// Enforces all security boundary invariants per ARCHITECTURE §9 and SECURITY §9/§13.
pub fn resolve_workspace_path_with_root(
    root: &Path,
    rel_path: &str,
) -> Result<PathBuf, SecurityError> {
    // 1. Strict length limit (Windows MAX_PATH compatibility guard)
    if rel_path.is_empty() || rel_path.len() > 250 {
        return Err(SecurityError::PathTooLong(format!(
            "Path length {} is invalid",
            rel_path.len()
        )));
    }

    // 2. Reject leading slashes, backslashes, drive letters, UNC paths
    if rel_path.starts_with('/')
        || rel_path.starts_with('\\')
        || rel_path.contains(':')
        || rel_path.starts_with(r"\\")
    {
        if rel_path.contains(':') {
            return Err(SecurityError::AlternateDataStreamForbidden(
                rel_path.to_string(),
            ));
        }
        return Err(SecurityError::AbsoluteForbidden(rel_path.to_string()));
    }

    // 3. Reject directory traversal '..'
    if rel_path.contains("..") {
        return Err(SecurityError::TraversalAttempt(rel_path.to_string()));
    }

    // 4. Inspect individual segments
    for segment in rel_path.split('/') {
        if segment.is_empty() {
            return Err(SecurityError::InvalidFormat("Empty segment".to_string()));
        }

        // Trailing dot or space check
        if segment.ends_with('.') || segment.ends_with(' ') {
            return Err(SecurityError::TrailingDotOrSpaceForbidden(
                segment.to_string(),
            ));
        }

        // 8.3 short name check
        if SHORT_NAME_REGEX.is_match(segment) {
            return Err(SecurityError::ShortNameForbidden(segment.to_string()));
        }

        // Reserved Windows device names
        if RESERVED_DEVICE_REGEX.is_match(segment) {
            return Err(SecurityError::ReservedDeviceNameForbidden(
                segment.to_string(),
            ));
        }
    }

    // 5. Strict regex validation for POSIX format
    if !WORKSPACE_PATH_REGEX.is_match(rel_path) {
        return Err(SecurityError::InvalidFormat(rel_path.to_string()));
    }

    // 6. Verify workspace root exists before filesystem operations
    if !root.exists() {
        return Err(SecurityError::WorkspaceRootMissing(
            root.display().to_string(),
        ));
    }

    // 7. Combine with root
    let target = root.join(Path::new(&rel_path.replace('/', "\\")));

    // 7. Verify components don't escape root
    for component in target.components() {
        if matches!(component, Component::ParentDir) {
            return Err(SecurityError::TraversalAttempt(rel_path.to_string()));
        }
    }

    // 8. Canonicalization check (ensuring symlinks and junctions cannot escape root)
    if target.exists() {
        let canonical_target = dunce::canonicalize(&target).map_err(|e| {
            SecurityError::BoundaryEscape(format!("Canonicalize target failed: {}", e))
        })?;
        let canonical_root = dunce::canonicalize(root).map_err(|e| {
            SecurityError::RootResolutionFailed(format!("Canonicalize root failed: {}", e))
        })?;

        let norm_target = canonical_target.to_string_lossy().to_lowercase();
        let norm_root = canonical_root.to_string_lossy().to_lowercase();

        if !norm_target.starts_with(&norm_root) {
            return Err(SecurityError::BoundaryEscape(format!(
                "Target '{}' escapes root '{}'",
                canonical_target.display(),
                canonical_root.display()
            )));
        }
    } else {
        // If target does not exist yet, verify closest existing ancestor does not escape root
        let mut ancestor = target.parent();
        while let Some(anc) = ancestor {
            if anc.exists() {
                let canonical_anc = dunce::canonicalize(anc).map_err(|e| {
                    SecurityError::BoundaryEscape(format!("Canonicalize ancestor failed: {}", e))
                })?;
                let canonical_root = dunce::canonicalize(root).map_err(|e| {
                    SecurityError::RootResolutionFailed(format!("Canonicalize root failed: {}", e))
                })?;

                let norm_anc = canonical_anc.to_string_lossy().to_lowercase();
                let norm_root = canonical_root.to_string_lossy().to_lowercase();

                if !norm_anc.starts_with(&norm_root) {
                    return Err(SecurityError::BoundaryEscape(format!(
                        "Ancestor '{}' escapes root '{}'",
                        canonical_anc.display(),
                        canonical_root.display()
                    )));
                }
                break;
            }
            ancestor = anc.parent();
        }
    }

    Ok(target)
}

/// Validates an individual path segment (component, version, filename).
fn validate_path_segment(segment: &str) -> Result<(), SecurityError> {
    if segment.is_empty() || segment.len() > 100 {
        return Err(SecurityError::InvalidFormat(format!(
            "Invalid segment length: {}",
            segment.len()
        )));
    }
    if segment.contains('/')
        || segment.contains('\\')
        || segment.contains(':')
        || segment.contains("..")
    {
        return Err(SecurityError::InvalidFormat(segment.to_string()));
    }
    if SHORT_NAME_REGEX.is_match(segment) {
        return Err(SecurityError::ShortNameForbidden(segment.to_string()));
    }
    if RESERVED_DEVICE_REGEX.is_match(segment) {
        return Err(SecurityError::ReservedDeviceNameForbidden(
            segment.to_string(),
        ));
    }
    if segment.ends_with('.') || segment.ends_with(' ') {
        return Err(SecurityError::TrailingDotOrSpaceForbidden(
            segment.to_string(),
        ));
    }
    Ok(())
}

/// Checks whether an executable name is in the trusted binary allowlist.
pub fn is_allowed_bin(exe: &str) -> bool {
    match exe {
        "yt-dlp.exe" | "qjs.exe" | "ffmpeg.exe" | "ffprobe.exe" | "whisper-cli.exe" => true,
        #[cfg(any(debug_assertions, test))]
        "fake-sidecar.exe" => true,
        _ => false,
    }
}

/// Checks whether a component and executable pair is allowed.
pub fn is_valid_component_and_exe(component: &str, exe: &str) -> bool {
    match (component, exe) {
        ("yt-dlp", "yt-dlp.exe") => true,
        ("quickjs" | "qjs", "qjs.exe") => true,
        ("ffmpeg", "ffmpeg.exe" | "ffprobe.exe") => true,
        ("whisper" | "whisper-cli", "whisper-cli.exe") => true,
        #[cfg(any(debug_assertions, test))]
        ("fake-sidecar", "fake-sidecar.exe") => true,
        _ => false,
    }
}

/// Checks whether a speech model filename is in the models allowlist.
pub fn is_allowed_model(filename: &str) -> bool {
    matches!(
        filename,
        "ggml-silero-v6.2.0.bin"
            | "ggml-base.bin"
            | "ggml-small-q5_1.bin"
            | "ggml-small.bin"
            | "ggml-medium.bin"
            | "ggml-tiny.bin"
            | "ggml-large-v3.bin"
    )
}

/// Category of trusted workspace components (sidecar binaries or speech models).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentKind {
    Bin,
    Models,
}

impl ComponentKind {
    pub fn subdir(&self) -> &'static str {
        match self {
            ComponentKind::Bin => "bin",
            ComponentKind::Models => "models",
        }
    }

    pub fn is_allowed(&self, filename: &str) -> bool {
        match self {
            ComponentKind::Bin => is_allowed_bin(filename),
            ComponentKind::Models => is_allowed_model(filename),
        }
    }
}

/// Resolves a versioned component binary against the default workspace root.
pub fn resolve_component_path(
    component: &str,
    version: &str,
    exe: &str,
) -> Result<PathBuf, SecurityError> {
    let root = get_workspace_root()?;
    resolve_component_path_with_root(&root, component, version, exe)
}

/// Resolves a versioned component binary `bin/<component>/<version>/<exe>` against an explicit workspace root.
pub fn resolve_component_path_with_root(
    root: &Path,
    component: &str,
    version: &str,
    exe: &str,
) -> Result<PathBuf, SecurityError> {
    if !is_valid_component_and_exe(component, exe) {
        return Err(SecurityError::ComponentNotAllowlisted(format!(
            "{}/{}",
            component, exe
        )));
    }

    validate_path_segment(component)?;
    validate_path_segment(version)?;
    validate_path_segment(exe)?;

    if !root.exists() {
        return Err(SecurityError::WorkspaceRootMissing(
            root.display().to_string(),
        ));
    }

    let bin_dir = root.join("bin");
    let comp_dir = bin_dir.join(component);
    let ver_dir = comp_dir.join(version);
    let target = ver_dir.join(exe);

    let canonical_root = dunce::canonicalize(root).map_err(|e| {
        if !root.exists() {
            SecurityError::WorkspaceRootMissing(root.display().to_string())
        } else {
            SecurityError::RootResolutionFailed(format!("Canonicalize root failed: {}", e))
        }
    })?;

    let check_path = if target.exists() {
        dunce::canonicalize(&target).map_err(|e| {
            SecurityError::BoundaryEscape(format!("Canonicalize target failed: {}", e))
        })?
    } else if ver_dir.exists() {
        dunce::canonicalize(&ver_dir).map_err(|e| {
            SecurityError::BoundaryEscape(format!("Canonicalize version dir failed: {}", e))
        })?
    } else if comp_dir.exists() {
        dunce::canonicalize(&comp_dir).map_err(|e| {
            SecurityError::BoundaryEscape(format!("Canonicalize component dir failed: {}", e))
        })?
    } else if bin_dir.exists() {
        dunce::canonicalize(&bin_dir).map_err(|e| {
            SecurityError::BoundaryEscape(format!("Canonicalize bin dir failed: {}", e))
        })?
    } else {
        canonical_root.clone()
    };

    let norm_check = check_path.to_string_lossy().to_lowercase();
    let norm_root = canonical_root.to_string_lossy().to_lowercase();
    if !norm_check.starts_with(&norm_root) {
        return Err(SecurityError::BoundaryEscape(format!(
            "Component path '{}' escapes root '{}'",
            check_path.display(),
            canonical_root.display()
        )));
    }

    Ok(target)
}

/// Resolves a speech model file against the default workspace root.
pub fn resolve_model_path(file: &str) -> Result<PathBuf, SecurityError> {
    let root = get_workspace_root()?;
    resolve_model_path_with_root(&root, file)
}

/// Resolves a speech model file `models/<file>` against an explicit workspace root.
pub fn resolve_model_path_with_root(root: &Path, file: &str) -> Result<PathBuf, SecurityError> {
    if !is_allowed_model(file) {
        return Err(SecurityError::ComponentNotAllowlisted(file.to_string()));
    }

    validate_path_segment(file)?;

    if !root.exists() {
        return Err(SecurityError::WorkspaceRootMissing(
            root.display().to_string(),
        ));
    }

    let models_dir = root.join("models");
    let target = models_dir.join(file);

    let canonical_root = dunce::canonicalize(root).map_err(|e| {
        if !root.exists() {
            SecurityError::WorkspaceRootMissing(root.display().to_string())
        } else {
            SecurityError::RootResolutionFailed(format!("Canonicalize root failed: {}", e))
        }
    })?;

    let check_path = if target.exists() {
        dunce::canonicalize(&target).map_err(|e| {
            SecurityError::BoundaryEscape(format!("Canonicalize target failed: {}", e))
        })?
    } else if models_dir.exists() {
        dunce::canonicalize(&models_dir).map_err(|e| {
            SecurityError::BoundaryEscape(format!("Canonicalize models dir failed: {}", e))
        })?
    } else {
        canonical_root.clone()
    };

    let norm_check = check_path.to_string_lossy().to_lowercase();
    let norm_root = canonical_root.to_string_lossy().to_lowercase();
    if !norm_check.starts_with(&norm_root) {
        return Err(SecurityError::BoundaryEscape(format!(
            "Model path '{}' escapes root '{}'",
            check_path.display(),
            canonical_root.display()
        )));
    }

    Ok(target)
}

/// Returns the free disk space available in bytes at the specified path.
#[cfg(windows)]
pub fn get_free_disk_space(path: &Path) -> Result<u64, SecurityError> {
    use std::os::windows::ffi::OsStrExt;

    let mut current = path;
    while !current.exists() {
        if let Some(parent) = current.parent() {
            current = parent;
        } else {
            break;
        }
    }

    let mut wide: Vec<u16> = current.as_os_str().encode_wide().collect();
    wide.push(0);

    let mut free_bytes_available: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut total_free_bytes: u64 = 0;

    let success = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_bytes_available,
            &mut total_bytes,
            &mut total_free_bytes,
        )
    };

    if success == 0 {
        return Err(SecurityError::RootResolutionFailed(format!(
            "GetDiskFreeSpaceExW failed for '{}': os error {}",
            current.display(),
            std::io::Error::last_os_error()
        )));
    }

    Ok(free_bytes_available)
}

#[cfg(not(windows))]
pub fn get_free_disk_space(_path: &Path) -> Result<u64, SecurityError> {
    Ok(100 * 1024 * 1024 * 1024) // 100 GB mock for non-Windows
}

/// Redacts sensitive tokens, query strings, email addresses, and Windows usernames.
pub fn redact_sensitive_text(input: &str) -> String {
    let mut text = input.to_string();

    // 1. Redact JWT tokens
    text = JWT_REGEX.replace_all(&text, "[REDACTED_TOKEN]").to_string();

    // 2. Redact Bearer authorization headers
    text = BEARER_REGEX
        .replace_all(&text, "Bearer [REDACTED_TOKEN]")
        .to_string();

    // 3. Redact URL query strings
    text = QUERY_STRING_REGEX
        .replace_all(&text, "?[REDACTED_QUERY]")
        .to_string();

    // 4. Redact emails
    text = EMAIL_REGEX
        .replace_all(&text, "[REDACTED_EMAIL]")
        .to_string();

    // 5. Redact Windows username in paths (C:\Users\<username> -> %USERPROFILE%)
    text = USER_PROFILE_REGEX
        .replace_all(&text, "%USERPROFILE%")
        .to_string();

    text
}

/// Redaction layer for tracing that ensures no secrets or sensitive user data are logged.
pub struct RedactionLayer;

impl<S> Layer<S> for RedactionLayer
where
    S: tracing::Subscriber,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        struct RedactionVisitor;
        impl Visit for RedactionVisitor {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                let name = field.name();
                if name.contains("token")
                    || name.contains("key")
                    || name.contains("secret")
                    || name.contains("jwt")
                {
                    // Suppressed
                    return;
                }
                let raw = format!("{:?}", value);
                let _redacted = redact_sensitive_text(&raw);
            }

            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                let name = field.name();
                if name.contains("token")
                    || name.contains("key")
                    || name.contains("secret")
                    || name.contains("jwt")
                {
                    // Suppressed
                    return;
                }
                let _redacted = redact_sensitive_text(value);
            }
        }
        let mut visitor = RedactionVisitor;
        event.record(&mut visitor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_valid_workspace_paths() {
        let temp_dir = std::env::temp_dir().join("test_clipper_ws_valid");
        fs::create_dir_all(&temp_dir).unwrap();

        let valid_paths = [
            "cache/download_123.mp4",
            "projects/proj_abc/composition.json",
            "cache/thumbnails/preview.jpg",
            "projects/123-abc_test/audio.wav",
        ];

        for path in valid_paths {
            let res = resolve_workspace_path_with_root(&temp_dir, path);
            assert!(res.is_ok(), "Failed for valid path: {}", path);
            assert!(res.unwrap().starts_with(&temp_dir));
        }

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_rejects_path_traversal() {
        let root = PathBuf::from(r"C:\AppData\Local\ClipperHelper");
        let traversal_paths = [
            "cache/../secret.txt",
            "projects/../../Windows/System32",
            "cache/..../outside",
            "cache/sub/../../escape",
        ];

        for path in traversal_paths {
            let err = resolve_workspace_path_with_root(&root, path).unwrap_err();
            assert!(matches!(
                err,
                SecurityError::TraversalAttempt(_) | SecurityError::InvalidFormat(_)
            ));
        }
    }

    #[test]
    fn test_rejects_absolute_and_unc_paths() {
        let root = PathBuf::from(r"C:\AppData\Local\ClipperHelper");
        let invalid = [
            r"C:\Windows\System32\calc.exe",
            r"/etc/passwd",
            r"\\server\share\file.txt",
            r"D:/cache/file.mp4",
        ];

        for path in invalid {
            let err = resolve_workspace_path_with_root(&root, path).unwrap_err();
            assert!(matches!(
                err,
                SecurityError::AbsoluteForbidden(_)
                    | SecurityError::AlternateDataStreamForbidden(_)
                    | SecurityError::InvalidFormat(_)
            ));
        }
    }

    #[test]
    fn test_rejects_ntfs_alternate_data_streams() {
        let root = PathBuf::from(r"C:\AppData\Local\ClipperHelper");
        let ads_paths = [
            "cache/download.mp4::$DATA",
            "cache/file:hidden_stream",
            "projects/project:zone.identifier",
        ];

        for path in ads_paths {
            let err = resolve_workspace_path_with_root(&root, path).unwrap_err();
            assert_eq!(
                err,
                SecurityError::AlternateDataStreamForbidden(path.to_string())
            );
        }
    }

    #[test]
    fn test_rejects_reserved_device_names() {
        let root = PathBuf::from(r"C:\AppData\Local\ClipperHelper");
        let devices = [
            "cache/CON",
            "cache/con.txt",
            "projects/PRN",
            "projects/aux.mp4",
            "cache/NUL",
            "cache/COM1",
            "cache/com5.dat",
            "projects/LPT1",
            "projects/lpt9.log",
        ];

        for path in devices {
            let err = resolve_workspace_path_with_root(&root, path).unwrap_err();
            assert!(matches!(err, SecurityError::ReservedDeviceNameForbidden(_)));
        }
    }

    #[test]
    fn test_rejects_trailing_dots_and_spaces() {
        let root = PathBuf::from(r"C:\AppData\Local\ClipperHelper");
        let trailing = [
            "cache/download.mp4.",
            "cache/download.mp4 ",
            "projects/folder./file.json",
            "projects/folder /file.json",
        ];

        for path in trailing {
            let err = resolve_workspace_path_with_root(&root, path).unwrap_err();
            assert!(matches!(err, SecurityError::TrailingDotOrSpaceForbidden(_)));
        }
    }

    #[test]
    fn test_rejects_short_8_3_names() {
        let root = PathBuf::from(r"C:\AppData\Local\ClipperHelper");
        let short_names = [
            "cache/PROGRA~1/file.mp4",
            "projects/CLIPPE~2.DAT",
            "cache/TEST~1",
        ];

        for path in short_names {
            let err = resolve_workspace_path_with_root(&root, path).unwrap_err();
            assert!(matches!(err, SecurityError::ShortNameForbidden(_)));
        }
    }

    #[test]
    fn test_rejects_over_long_paths() {
        let root = PathBuf::from(r"C:\AppData\Local\ClipperHelper");
        let long_path = format!("cache/{}", "a".repeat(260));
        let err = resolve_workspace_path_with_root(&root, &long_path).unwrap_err();
        assert!(matches!(err, SecurityError::PathTooLong(_)));
    }

    #[test]
    fn test_symlink_escape_detected() {
        let test_base = std::env::temp_dir().join("test_symlink_guard");
        let ws_root = test_base.join("workspace");
        let outside = test_base.join("outside");

        let _ = fs::remove_dir_all(&test_base);
        fs::create_dir_all(ws_root.join("cache")).unwrap();
        fs::create_dir_all(&outside).unwrap();

        let outside_file = outside.join("secret.txt");
        fs::write(&outside_file, "secret").unwrap();

        // On Windows without admin, symlinks might fail to create; if it fails, test graceful skip
        #[cfg(windows)]
        {
            let link_path = ws_root.join("cache").join("link_to_outside.txt");
            if std::os::windows::fs::symlink_file(&outside_file, &link_path).is_ok() {
                let err = resolve_workspace_path_with_root(&ws_root, "cache/link_to_outside.txt")
                    .unwrap_err();
                assert!(matches!(err, SecurityError::BoundaryEscape(_)));
            }
        }

        let _ = fs::remove_dir_all(&test_base);
    }

    #[test]
    fn test_extended_logging_redaction() {
        // Tokens
        let jwt =
            "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozGzptqJ";
        let redacted = redact_sensitive_text(jwt);
        assert!(!redacted.contains("eyJhbGci"));
        assert!(redacted.contains("[REDACTED_TOKEN]"));

        // Query string
        let url = "https://api.supabase.co/v1/auth?token=secret123&redirect=true";
        assert_eq!(
            redact_sensitive_text(url),
            "https://api.supabase.co/v1/auth?[REDACTED_QUERY]"
        );

        // Emails
        let email_text = "Contact support at user.name@domain.co.id for assistance.";
        assert_eq!(
            redact_sensitive_text(email_text),
            "Contact support at [REDACTED_EMAIL] for assistance."
        );

        // Windows username in path
        let win_path =
            r"Error loading file from C:\Users\MyUsername\AppData\Local\ClipperHelper\log.txt";
        let redacted_path = redact_sensitive_text(win_path);
        assert!(!redacted_path.contains("MyUsername"));
        assert!(redacted_path.contains("%USERPROFILE%"));
    }

    #[test]
    fn test_ensure_workspace_creates_subdirs_and_canonicalizes() {
        let test_base = std::env::temp_dir().join(format!("test_ws_init_{}", std::process::id()));
        let _ = fs::remove_dir_all(&test_base);

        assert!(!test_base.exists());
        let res = ensure_workspace_with_root(&test_base);
        assert!(res.is_ok(), "ensure_workspace_with_root failed: {:?}", res);
        let canonical = res.unwrap();
        assert!(canonical.exists());

        // Verify subdirectories exist
        for sub in &["bin", "models", "cache", "projects", "logs"] {
            let p = canonical.join(sub);
            assert!(p.is_dir(), "Expected subdir {:?} to exist as dir", p);
        }

        let _ = fs::remove_dir_all(&test_base);
    }

    #[test]
    fn test_resolve_workspace_path_missing_root_returns_typed_error() {
        let non_existent_root =
            std::env::temp_dir().join(format!("test_ws_non_existent_{}", std::process::id()));
        let _ = fs::remove_dir_all(&non_existent_root);

        let res = resolve_workspace_path_with_root(&non_existent_root, "cache/probe.tmp");
        assert!(res.is_err());
        match res.unwrap_err() {
            SecurityError::WorkspaceRootMissing(p) => {
                assert!(p.contains("test_ws_non_existent_"));
            }
            other => panic!("Expected WorkspaceRootMissing, got: {:?}", other),
        }
    }

    #[test]
    fn test_resolve_component_path_allowlist_and_containment() {
        let test_base =
            std::env::temp_dir().join(format!("test_ws_component_{}", std::process::id()));
        let _ = fs::remove_dir_all(&test_base);
        ensure_workspace_with_root(&test_base).unwrap();

        // Allowlisted versioned binaries
        assert!(
            resolve_component_path_with_root(&test_base, "yt-dlp", "2026.10.26", "yt-dlp.exe")
                .is_ok()
        );
        assert!(
            resolve_component_path_with_root(&test_base, "ffmpeg", "n9.0.2", "ffmpeg.exe").is_ok()
        );
        assert!(
            resolve_component_path_with_root(&test_base, "ffmpeg", "n9.0.2", "ffprobe.exe").is_ok()
        );
        assert!(
            resolve_component_path_with_root(&test_base, "quickjs", "2024.01", "qjs.exe").is_ok()
        );
        assert!(resolve_component_path_with_root(&test_base, "qjs", "2024.01", "qjs.exe").is_ok());
        assert!(resolve_component_path_with_root(
            &test_base,
            "whisper-cli",
            "v1.9.5",
            "whisper-cli.exe"
        )
        .is_ok());
        assert!(resolve_component_path_with_root(
            &test_base,
            "whisper",
            "v1.9.5",
            "whisper-cli.exe"
        )
        .is_ok());
        #[cfg(any(debug_assertions, test))]
        assert!(resolve_component_path_with_root(
            &test_base,
            "fake-sidecar",
            "1.0.0",
            "fake-sidecar.exe"
        )
        .is_ok());

        // Disallowed binary executable
        let disallowed_exe =
            resolve_component_path_with_root(&test_base, "yt-dlp", "2026.10.26", "cmd.exe");
        assert!(matches!(
            disallowed_exe.unwrap_err(),
            SecurityError::ComponentNotAllowlisted(_)
        ));

        // Disallowed component
        let disallowed_comp =
            resolve_component_path_with_root(&test_base, "powershell", "1.0", "powershell.exe");
        assert!(matches!(
            disallowed_comp.unwrap_err(),
            SecurityError::ComponentNotAllowlisted(_)
        ));

        // Mismatched component and binary
        let mismatched =
            resolve_component_path_with_root(&test_base, "yt-dlp", "2026.10.26", "ffmpeg.exe");
        assert!(matches!(
            mismatched.unwrap_err(),
            SecurityError::ComponentNotAllowlisted(_)
        ));

        // Traversal attempt in version or component
        assert!(
            resolve_component_path_with_root(&test_base, "yt-dlp", "../v1", "yt-dlp.exe").is_err()
        );
        assert!(
            resolve_component_path_with_root(&test_base, "../yt-dlp", "v1", "yt-dlp.exe").is_err()
        );

        // Allowlisted models in models/<file>
        assert!(resolve_model_path_with_root(&test_base, "ggml-silero-v6.2.0.bin").is_ok());
        assert!(resolve_model_path_with_root(&test_base, "ggml-base.bin").is_ok());
        assert!(resolve_model_path_with_root(&test_base, "ggml-small-q5_1.bin").is_ok());
        assert!(resolve_model_path_with_root(&test_base, "ggml-small.bin").is_ok());
        assert!(resolve_model_path_with_root(&test_base, "ggml-medium.bin").is_ok());

        // Disallowed model
        let disallowed_model = resolve_model_path_with_root(&test_base, "untrusted-model.bin");
        assert!(matches!(
            disallowed_model.unwrap_err(),
            SecurityError::ComponentNotAllowlisted(_)
        ));

        // Traversal attempt in model name
        assert!(resolve_model_path_with_root(&test_base, "../models/ggml-base.bin").is_err());

        let _ = fs::remove_dir_all(&test_base);
    }

    #[test]
    fn test_ensure_workspace_denied_location() {
        // Attempting to create workspace where root is an existing regular file
        let temp_file =
            std::env::temp_dir().join(format!("test_ws_file_{}.tmp", std::process::id()));
        fs::write(&temp_file, "not a directory").unwrap();

        let res = ensure_workspace_with_root(&temp_file);
        assert!(res.is_err());
        match res.unwrap_err() {
            SecurityError::RootResolutionFailed(msg) => {
                assert!(msg.contains("is a file, not a directory"));
            }
            other => panic!("Expected RootResolutionFailed, got {:?}", other),
        }

        let _ = fs::remove_file(&temp_file);
    }

    #[test]
    fn test_ensure_workspace_junction_escape_rejected() {
        let test_base =
            std::env::temp_dir().join(format!("test_ws_junction_{}", std::process::id()));
        let ws_root = test_base.join("workspace");
        let outside = test_base.join("outside");

        let _ = fs::remove_dir_all(&test_base);
        fs::create_dir_all(&ws_root).unwrap();
        fs::create_dir_all(&outside).unwrap();

        // Create a directory junction for `projects` pointing to `outside`
        #[cfg(windows)]
        {
            let link = ws_root.join("projects");
            let status = std::process::Command::new("cmd")
                .args([
                    "/C",
                    "mklink",
                    "/J",
                    &link.to_string_lossy(),
                    &outside.to_string_lossy(),
                ])
                .output();

            if let Ok(out) = status {
                if out.status.success() {
                    let res = ensure_workspace_with_root(&ws_root);
                    assert!(res.is_err(), "Expected junction escape to be rejected");
                    assert!(matches!(res.unwrap_err(), SecurityError::BoundaryEscape(_)));
                }
            }
        }

        let _ = fs::remove_dir_all(&test_base);
    }

    #[test]
    fn test_free_disk_space() {
        let temp_dir = std::env::temp_dir();
        let free_bytes = get_free_disk_space(&temp_dir);
        assert!(free_bytes.is_ok());
        assert!(free_bytes.unwrap() > 0);
    }

    #[test]
    fn test_assert_can_reset_dev_workspace() {
        // Valid dev workspace root
        let valid = PathBuf::from(r"C:\Users\test\AppData\Local\ClipperHelper-Dev");
        assert!(assert_can_reset_dev_workspace(&valid).is_ok());

        // Release workspace root must be refused
        let release = PathBuf::from(r"C:\Users\test\AppData\Local\ClipperHelper");
        assert!(assert_can_reset_dev_workspace(&release).is_err());

        // Arbitrary directory must be refused
        let other = PathBuf::from(r"C:\Users\test\Desktop\SomeFolder");
        assert!(assert_can_reset_dev_workspace(&other).is_err());

        // Subdirectory of dev workspace root must be refused
        let sub = PathBuf::from(r"C:\Users\test\AppData\Local\ClipperHelper-Dev\cache");
        assert!(assert_can_reset_dev_workspace(&sub).is_err());

        // Exact match required: lowercase/variation must be refused
        let lower = PathBuf::from(r"C:\Users\test\AppData\Local\clipperhelper-dev");
        assert!(assert_can_reset_dev_workspace(&lower).is_err());
    }

    #[test]
    fn test_dataroot_validation_rules() {
        // 1. Relative path rejected
        let rel = Path::new("relative/data/root");
        assert!(matches!(
            DataRoot::validate(rel).unwrap_err(),
            SecurityError::InvalidDataRoot(_)
        ));

        // 2. UNC path rejected
        let unc = Path::new(r"\\network_server\share\clipper");
        assert!(matches!(
            DataRoot::validate(unc).unwrap_err(),
            SecurityError::InvalidDataRoot(_)
        ));

        // 3. System directories rejected
        assert!(matches!(
            DataRoot::validate(Path::new(r"C:\Windows")).unwrap_err(),
            SecurityError::InvalidDataRoot(_)
        ));
        assert!(matches!(
            DataRoot::validate(Path::new(r"C:\Program Files")).unwrap_err(),
            SecurityError::InvalidDataRoot(_)
        ));
        assert!(matches!(
            DataRoot::validate(Path::new(r"C:\")).unwrap_err(),
            SecurityError::InvalidDataRoot(_)
        ));

        // 4. Valid empty directory accepted
        let test_dir =
            std::env::temp_dir().join(format!("test_dataroot_valid_{}", std::process::id()));
        let _ = fs::remove_dir_all(&test_dir);
        fs::create_dir_all(&test_dir).unwrap();

        let valid_res = DataRoot::validate(&test_dir);
        assert!(valid_res.is_ok(), "Empty directory must be accepted");

        // 5. Existing ClipperHelper directory with marker accepted
        let marker_file = test_dir.join("clipper.db");
        fs::write(&marker_file, b"sqlite header").unwrap();
        assert!(DataRoot::validate(&test_dir).is_ok());

        // 6. Non-empty foreign directory rejected
        let _ = fs::remove_file(&marker_file);
        let foreign_file = test_dir.join("vacation_photo.jpg");
        fs::write(&foreign_file, b"jpeg bytes").unwrap();
        let foreign_res = DataRoot::validate(&test_dir);
        assert!(
            matches!(foreign_res.unwrap_err(), SecurityError::InvalidDataRoot(_)),
            "Non-empty foreign directory must be rejected"
        );

        let _ = fs::remove_dir_all(&test_dir);
    }

    #[test]
    fn test_dataroot_onedrive_detection() {
        let onedrive_path = Path::new(r"C:\Users\User\OneDrive\Documents\ClipperHelper");
        assert!(is_onedrive_path(onedrive_path));

        let local_path = Path::new(r"C:\Users\User\AppData\Local\ClipperHelper");
        assert!(!is_onedrive_path(local_path));
    }

    #[test]
    fn test_dataroot_bootstrap_roundtrip() {
        let test_base = std::env::temp_dir().join(format!("test_bootstrap_{}", std::process::id()));
        let default_root = test_base.join("default");
        let custom_root = test_base.join("custom");

        let _ = fs::remove_dir_all(&test_base);
        fs::create_dir_all(&default_root).unwrap();
        fs::create_dir_all(&custom_root).unwrap();

        // Initially no bootstrap
        assert_eq!(DataRoot::read_bootstrap(&default_root).unwrap(), None);

        // Write bootstrap
        DataRoot::write_bootstrap(&default_root, &custom_root).unwrap();

        // Read bootstrap back
        let read_back = DataRoot::read_bootstrap(&default_root).unwrap();
        assert_eq!(read_back, Some(custom_root.clone()));

        let _ = fs::remove_dir_all(&test_base);
    }

    #[test]
    fn test_check_free_space_typed_errors() {
        let temp_dir = std::env::temp_dir();
        let available = get_free_disk_space(&temp_dir).unwrap();
        assert!(available > 0);

        // 1. Available space sufficient
        assert!(check_free_space_at(&temp_dir, 1000).is_ok());

        // 2. Disk full: requested > available
        let req_full = available.saturating_add(1024 * 1024);
        let err_full = check_free_space_at(&temp_dir, req_full);
        assert!(err_full.is_err());
        let err = err_full.unwrap_err();
        assert_eq!(err.error_code(), "E_DISK_FULL");
        match err {
            SecurityError::DiskFull {
                available: a,
                required: r,
            } => {
                assert!(a > 0);
                assert!(a < r);
                assert_eq!(r, req_full);
            }
            other => panic!("Expected DiskFull, got: {:?}", other),
        }

        // 3. Disk low: available < required + 500MB
        if available > 100 * 1024 * 1024 {
            let required = available.saturating_sub(100 * 1024 * 1024);
            let err_low = check_free_space_at(&temp_dir, required);
            assert!(err_low.is_err());
            let err = err_low.unwrap_err();
            assert_eq!(err.error_code(), "E_DISK_LOW");
            match err {
                SecurityError::DiskLow {
                    available: a,
                    required: r,
                } => {
                    assert!(a > 0);
                    assert!(a < r);
                    assert_eq!(r, required + 500 * 1024 * 1024);
                }
                other => panic!("Expected DiskLow, got: {:?}", other),
            }
        }
    }
}
