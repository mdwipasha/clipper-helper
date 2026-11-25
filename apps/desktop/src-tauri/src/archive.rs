//! Safe archive verification and extraction into same-volume staging directories
//! (ARCHITECTURE §9, §13; SECURITY §9–§10).
//!
//! Crate & feature justification:
//! - `zip = "=9.8.2"` (MIT), `default-features = false, features = ["deflate-flate2-zlib-rs"]`:
//!   Only Stored and Deflate compression are used by upstream release ZIPs (FFmpeg, whisper.cpp).
//!   Disabling default features strips unused bzip2/lzma/zstd/aes-crypto/time attack surface, and
//!   `deflate-flate2-zlib-rs` uses the pure-Rust `zlib-rs` backend so no native C zlib build is needed.
//! - `uuid = "=1.18.1"` (Apache-2.0/MIT), `features = ["v4"]`:
//!   Generates unpredictable `.staging-<uuid>` folder names inside `bin/<component>/`.
//!
//! Enforces strict ZIP extraction invariants:
//! - Verifies the archive SHA-256 checksum and byte size before opening the ZIP container.
//! - Parses raw ZIP Central Directory headers first to reject exact duplicates and case-insensitive
//!   collisions even when high-level readers deduplicate map keys.
//! - Rejects path traversal (`..`), absolute/UNC paths, drive letters, NTFS ADS (`:`),
//!   backslashes, reserved Windows device names, trailing dots/spaces, 8.3 short names,
//!   over-long paths, encrypted entries, symlinks/junctions (Unix & Windows external
//!   attributes), non-regular file types, and duplicate/case-insensitive colliding names.
//! - Enforces entry count caps, manifest allowlist matching (rejects extras, requires all
//!   listed entries), per-file and total size bounds, zip-bomb compression ratio caps, and
//!   declared-vs-actual size agreement.
//! - Verifies every extracted file's byte length and SHA-256 checksum on disk after extraction.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;
use zip::ZipArchive;

use crate::manifest::ManifestFileEntry;
use crate::security;

/// Upper safety bounds enforced during ZIP extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveLimits {
    pub max_file_count: usize,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
    pub max_compression_ratio: u64,
    pub ratio_grace_bytes: u64,
    pub max_relative_path_len: usize,
    pub max_full_path_len: usize,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_file_count: 128,
            max_file_bytes: 512 * 1024 * 1024,
            max_total_bytes: 1024 * 1024 * 1024,
            max_compression_ratio: 50,
            ratio_grace_bytes: 4096,
            max_relative_path_len: 180,
            max_full_path_len: 240,
        }
    }
}

/// Extracted file verification record produced upon successful staging.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedFileRecord {
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
}

/// Result of extracting a verified archive into a same-volume staging directory.
#[derive(Debug, Clone)]
pub struct StagedArchive {
    pub staging_dir: PathBuf,
    pub files: Vec<ExtractedFileRecord>,
}

/// Errors returned by archive verification and extraction.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum ArchiveError {
    #[error("Archive SHA-256 verification failed: expected {expected}, got {actual}")]
    ArchiveIntegrityMismatch { expected: String, actual: String },

    #[error("Archive size mismatch: expected {expected} bytes, got {actual} bytes")]
    ArchiveSizeMismatch { expected: u64, actual: u64 },

    #[error("Invalid or corrupt ZIP archive: {0}")]
    InvalidZip(String),

    #[error("Manifest file list for archive cannot be empty or invalid: {0}")]
    InvalidManifestFiles(String),

    #[error("Unsafe archive entry path '{path}': {reason}")]
    UnsafePath { path: String, reason: String },

    #[error("Archive entry '{0}' is encrypted and cannot be extracted")]
    EncryptedEntryNotAllowed(String),

    #[error("Archive entry '{0}' is a symlink, junction, or non-regular file")]
    SymlinkOrNonRegularEntry(String),

    #[error(
        "Duplicate or case-colliding archive entry '{path}' conflicts with '{conflicting_with}'"
    )]
    DuplicateOrCollision {
        path: String,
        conflicting_with: String,
    },

    #[error("Archive contains {count} entries, exceeding maximum limit of {limit}")]
    TooManyEntries { count: usize, limit: usize },

    #[error("Archive contains unlisted extra entry '{0}' not declared in manifest files[]")]
    ExtraEntryNotAllowed(String),

    #[error("Archive is missing required manifest entry '{0}'")]
    MissingManifestEntry(String),

    #[error("Archive entry '{path}' exceeds size limit ({size} > {limit} bytes)")]
    SizeLimitExceeded { path: String, size: u64, limit: u64 },

    #[error(
        "Compression ratio limit exceeded for '{path}': {uncompressed} bytes from {compressed} compressed bytes (ratio {ratio}x > {limit}x)"
    )]
    CompressionRatioExceeded {
        path: String,
        uncompressed: u64,
        compressed: u64,
        ratio: u64,
        limit: u64,
    },

    #[error(
        "Declared vs actual size mismatch for '{path}': declared {declared} bytes, actual {actual} bytes"
    )]
    DeclaredVsActualSizeMismatch {
        path: String,
        declared: u64,
        actual: u64,
    },

    #[error("Extracted file '{path}' SHA-256 mismatch: expected {expected}, got {actual}")]
    ExtractedHashMismatch {
        path: String,
        expected: String,
        actual: String,
    },

    #[error("Extracted file '{path}' size mismatch on disk: expected {expected} bytes, got {actual} bytes")]
    ExtractedSizeMismatch {
        path: String,
        expected: u64,
        actual: u64,
    },

    #[error("Insufficient disk space during extraction: available {available} bytes, required {required} bytes")]
    DiskFull { available: u64, required: u64 },

    #[error("IO error during archive extraction: {0}")]
    Io(String),
}

impl ArchiveError {
    pub fn error_code(&self) -> &'static str {
        match self {
            ArchiveError::DiskFull { .. } => "E_DISK_FULL",
            _ => "E_VERIFY_FAILED",
        }
    }
}

/// RAII cleanup guard ensuring partial `.staging-<uuid>` folders are removed on failure.
struct StagingDirGuard {
    path: PathBuf,
    active: bool,
}

impl StagingDirGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, active: true }
    }

    fn commit(mut self) -> PathBuf {
        self.active = false;
        self.path.clone()
    }
}

impl Drop for StagingDirGuard {
    fn drop(&mut self) {
        if self.active && self.path.exists() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn map_write_io_error(err: io::Error, required: u64) -> ArchiveError {
    if err.kind() == io::ErrorKind::StorageFull
        || matches!(err.raw_os_error(), Some(39) | Some(112))
    {
        ArchiveError::DiskFull {
            available: 0,
            required,
        }
    } else {
        ArchiveError::Io(err.to_string())
    }
}

/// Computes exact byte size and lowercase hex SHA-256 of a file on disk.
pub fn compute_file_sha256_and_size(path: &Path) -> Result<(String, u64), io::Error> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    let mut total: u64 = 0;

    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }

    Ok((hex::encode(hasher.finalize()), total))
}

/// Validates a relative POSIX archive entry path against all traversal, Windows device,
/// ADS, short-name, backslash, trailing-dot/space, and length invariants.
fn validate_entry_path(
    raw_name: &str,
    is_dir: bool,
    staging_dir: &Path,
    limits: &ArchiveLimits,
) -> Result<String, ArchiveError> {
    if raw_name.is_empty() {
        return Err(ArchiveError::UnsafePath {
            path: raw_name.to_string(),
            reason: "empty path".to_string(),
        });
    }

    if raw_name.contains('\0') {
        return Err(ArchiveError::UnsafePath {
            path: raw_name.to_string(),
            reason: "NUL byte forbidden".to_string(),
        });
    }

    // Reject backslashes immediately (blocks UNC \\server\share, C:\..., and ..\ escape).
    if raw_name.contains('\\') {
        return Err(ArchiveError::UnsafePath {
            path: raw_name.to_string(),
            reason: "backslash is forbidden in archive paths".to_string(),
        });
    }

    // Reject leading slashes (absolute paths or //network paths).
    if raw_name.starts_with('/') {
        return Err(ArchiveError::UnsafePath {
            path: raw_name.to_string(),
            reason: "absolute path is forbidden".to_string(),
        });
    }

    // Reject colons everywhere (blocks drive letters like C:foo and NTFS ADS file::$DATA).
    if raw_name.contains(':') {
        return Err(ArchiveError::UnsafePath {
            path: raw_name.to_string(),
            reason: "colon (drive letter or alternate data stream) is forbidden".to_string(),
        });
    }

    let trimmed = if is_dir {
        raw_name.strip_suffix('/').unwrap_or(raw_name)
    } else {
        if raw_name.ends_with('/') {
            return Err(ArchiveError::UnsafePath {
                path: raw_name.to_string(),
                reason: "file entry must not end with '/'".to_string(),
            });
        }
        raw_name
    };

    if trimmed.is_empty() {
        return Err(ArchiveError::UnsafePath {
            path: raw_name.to_string(),
            reason: "empty path after trimming".to_string(),
        });
    }

    if trimmed.contains("..") {
        return Err(ArchiveError::UnsafePath {
            path: raw_name.to_string(),
            reason: "parent directory traversal '..' is forbidden".to_string(),
        });
    }

    if trimmed.len() > limits.max_relative_path_len {
        return Err(ArchiveError::UnsafePath {
            path: raw_name.to_string(),
            reason: format!(
                "relative path length {} exceeds limit {}",
                trimmed.len(),
                limits.max_relative_path_len
            ),
        });
    }

    for seg in trimmed.split('/') {
        if seg.is_empty() {
            return Err(ArchiveError::UnsafePath {
                path: raw_name.to_string(),
                reason: "empty path segment (double slash)".to_string(),
            });
        }
        if let Err(sec_err) = security::check_windows_segment_invariants(seg) {
            return Err(ArchiveError::UnsafePath {
                path: raw_name.to_string(),
                reason: sec_err.to_string(),
            });
        }
    }

    let full_target = staging_dir.join(Path::new(&trimmed.replace('/', "\\")));
    let full_len = full_target.to_string_lossy().len();
    if full_len > limits.max_full_path_len {
        return Err(ArchiveError::UnsafePath {
            path: raw_name.to_string(),
            reason: format!(
                "full destination path length {} exceeds Windows safe limit {}",
                full_len, limits.max_full_path_len
            ),
        });
    }

    Ok(trimmed.to_string())
}

/// Enforces external-attribute, file-type, and encryption checks on a ZIP entry.
fn check_entry_attributes<R: Read>(
    entry: &zip::read::ZipFile<'_, R>,
    raw_name: &str,
    is_dir: bool,
) -> Result<(), ArchiveError> {
    if entry.encrypted() {
        return Err(ArchiveError::EncryptedEntryNotAllowed(raw_name.to_string()));
    }

    if entry.is_symlink() {
        return Err(ArchiveError::SymlinkOrNonRegularEntry(raw_name.to_string()));
    }

    // Check raw external attributes regardless of platform field:
    // Upper 16 bits contain Unix POSIX mode (S_IFMT mask 0o170000).
    let ext = entry.external_attributes();
    let raw_unix = ext >> 16;
    let unix_mode = entry.unix_mode().unwrap_or(raw_unix);
    let unix_ftype = unix_mode & 0o170000;
    if unix_ftype != 0 {
        let expected_type = if is_dir { 0o040000 } else { 0o100000 };
        if unix_ftype != expected_type {
            return Err(ArchiveError::SymlinkOrNonRegularEntry(raw_name.to_string()));
        }
    }

    // Lower 16 bits contain DOS / Windows file attributes.
    let dos_attrs = ext & 0xFFFF;
    const FILE_ATTRIBUTE_VOLUME_ID: u32 = 0x0008;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0010;
    const FILE_ATTRIBUTE_DEVICE: u32 = 0x0040;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;

    if (dos_attrs & FILE_ATTRIBUTE_REPARSE_POINT) != 0
        || (dos_attrs & FILE_ATTRIBUTE_DEVICE) != 0
        || (dos_attrs & FILE_ATTRIBUTE_VOLUME_ID) != 0
    {
        return Err(ArchiveError::SymlinkOrNonRegularEntry(raw_name.to_string()));
    }

    if !is_dir && (dos_attrs & FILE_ATTRIBUTE_DIRECTORY) != 0 {
        return Err(ArchiveError::SymlinkOrNonRegularEntry(raw_name.to_string()));
    }

    Ok(())
}

struct ValidatedManifestFiles<'a> {
    map: HashMap<String, (&'a ManifestFileEntry, String)>,
    allowed_dirs: HashSet<String>,
    total_bytes: u64,
}

/// Scans the ZIP End-of-Central-Directory (EOCD) and walks every raw Central Directory header
/// so exact duplicates and case-insensitive name collisions are caught deterministically before
/// `ZipArchive` builds its internal filename lookup map.
fn check_raw_zip_central_directory(
    file: &mut File,
    limits: &ArchiveLimits,
) -> Result<(), ArchiveError> {
    let file_len = file
        .seek(SeekFrom::End(0))
        .map_err(|e| ArchiveError::Io(e.to_string()))?;
    if file_len < 22 {
        return Err(ArchiveError::InvalidZip(
            "File too small to be a valid ZIP archive".to_string(),
        ));
    }

    // Search tail window (max EOCD 22 + 65535 comment bytes) for EOCD signature PK\x05\x06.
    let search_len = file_len.min(22 + 65535) as usize;
    let search_start = file_len - search_len as u64;
    file.seek(SeekFrom::Start(search_start))
        .map_err(|e| ArchiveError::Io(e.to_string()))?;
    let mut tail = vec![0u8; search_len];
    file.read_exact(&mut tail)
        .map_err(|e| ArchiveError::Io(e.to_string()))?;

    let mut eocd_rel: Option<usize> = None;
    for i in (0..=tail.len().saturating_sub(22)).rev() {
        if tail[i..i + 4] == [0x50, 0x4b, 0x05, 0x06] {
            let comment_len = u16::from_le_bytes([tail[i + 20], tail[i + 21]]) as usize;
            if i + 22 + comment_len == tail.len() {
                eocd_rel = Some(i);
                break;
            }
        }
    }

    let eocd_off = eocd_rel.ok_or_else(|| {
        ArchiveError::InvalidZip("End of Central Directory record not found".to_string())
    })?;
    let eocd = &tail[eocd_off..];
    let eocd_entries = u16::from_le_bytes([eocd[10], eocd[11]]) as u64;
    let cd_size_32 = u32::from_le_bytes([eocd[12], eocd[13], eocd[14], eocd[15]]) as u64;
    let cd_offset_32 = u32::from_le_bytes([eocd[16], eocd[17], eocd[18], eocd[19]]) as u64;

    let (total_entries, cd_size, cd_offset) =
        if eocd_entries == 0xFFFF || cd_size_32 == 0xFFFFFFFF || cd_offset_32 == 0xFFFFFFFF {
            // Check ZIP64 End of Central Directory Locator (20 bytes immediately before EOCD)
            let eocd_abs = search_start + eocd_off as u64;
            if eocd_abs < 20 {
                return Err(ArchiveError::InvalidZip(
                    "Invalid ZIP64 EOCD locator position".to_string(),
                ));
            }
            file.seek(SeekFrom::Start(eocd_abs - 20))
                .map_err(|e| ArchiveError::Io(e.to_string()))?;
            let mut loc = [0u8; 20];
            file.read_exact(&mut loc)
                .map_err(|e| ArchiveError::Io(e.to_string()))?;
            if loc[0..4] != [0x50, 0x4b, 0x06, 0x07] {
                return Err(ArchiveError::InvalidZip(
                    "ZIP64 sentinel in EOCD without ZIP64 locator".to_string(),
                ));
            }
            let z64_off = u64::from_le_bytes(loc[8..16].try_into().unwrap_or([0; 8]));
            file.seek(SeekFrom::Start(z64_off))
                .map_err(|e| ArchiveError::Io(e.to_string()))?;
            let mut z64 = [0u8; 56];
            file.read_exact(&mut z64)
                .map_err(|e| ArchiveError::Io(e.to_string()))?;
            if z64[0..4] != [0x50, 0x4b, 0x06, 0x06] {
                return Err(ArchiveError::InvalidZip(
                    "Invalid ZIP64 EOCD signature".to_string(),
                ));
            }
            let entries = u64::from_le_bytes(z64[32..40].try_into().unwrap_or([0; 8]));
            let size = u64::from_le_bytes(z64[40..48].try_into().unwrap_or([0; 8]));
            let offset = u64::from_le_bytes(z64[48..56].try_into().unwrap_or([0; 8]));
            (entries, size, offset)
        } else {
            (eocd_entries, cd_size_32, cd_offset_32)
        };

    if total_entries > limits.max_file_count as u64 {
        return Err(ArchiveError::TooManyEntries {
            count: total_entries as usize,
            limit: limits.max_file_count,
        });
    }
    if cd_offset.saturating_add(cd_size) > file_len {
        return Err(ArchiveError::InvalidZip(
            "Central Directory extends past end of file".to_string(),
        ));
    }

    file.seek(SeekFrom::Start(cd_offset))
        .map_err(|e| ArchiveError::Io(e.to_string()))?;
    let mut seen_raw: HashMap<String, String> = HashMap::with_capacity(total_entries as usize);

    for _ in 0..total_entries {
        let mut hdr = [0u8; 46];
        file.read_exact(&mut hdr)
            .map_err(|e| ArchiveError::InvalidZip(format!("Truncated CD header: {}", e)))?;
        if hdr[0..4] != [0x50, 0x4b, 0x01, 0x02] {
            return Err(ArchiveError::InvalidZip(
                "Invalid Central Directory header signature".to_string(),
            ));
        }

        let name_len = u16::from_le_bytes([hdr[28], hdr[29]]) as usize;
        let extra_len = u16::from_le_bytes([hdr[30], hdr[31]]) as usize;
        let comment_len = u16::from_le_bytes([hdr[32], hdr[33]]) as usize;

        if name_len > 4096 {
            return Err(ArchiveError::UnsafePath {
                path: "<overlong raw entry>".to_string(),
                reason: format!("raw ZIP filename byte length {} exceeds cap", name_len),
            });
        }

        let mut name_buf = vec![0u8; name_len];
        file.read_exact(&mut name_buf)
            .map_err(|e| ArchiveError::InvalidZip(format!("Truncated CD filename: {}", e)))?;

        if name_buf.contains(&0) {
            return Err(ArchiveError::UnsafePath {
                path: String::from_utf8_lossy(&name_buf).to_string(),
                reason: "raw entry name contains NUL byte".to_string(),
            });
        }

        let raw_name = String::from_utf8(name_buf)
            .map_err(|e| ArchiveError::InvalidZip(format!("Non-UTF8 ZIP filename: {}", e)))?;
        let key = raw_name.trim_end_matches('/').to_lowercase();
        if let Some(prev) = seen_raw.get(&key) {
            return Err(ArchiveError::DuplicateOrCollision {
                path: raw_name,
                conflicting_with: prev.clone(),
            });
        }
        seen_raw.insert(key, raw_name);

        let skip = (extra_len + comment_len) as i64;
        if skip > 0 {
            file.seek(SeekFrom::Current(skip))
                .map_err(|e| ArchiveError::InvalidZip(format!("Seek past CD extra: {}", e)))?;
        }
    }

    file.seek(SeekFrom::Start(0))
        .map_err(|e| ArchiveError::Io(e.to_string()))?;
    Ok(())
}

/// Validates and indexes the manifest `files[]` list before opening the archive.
fn validate_manifest_file_list<'a>(
    manifest_files: &'a [ManifestFileEntry],
    staging_dir: &Path,
    limits: &ArchiveLimits,
) -> Result<ValidatedManifestFiles<'a>, ArchiveError> {
    if manifest_files.is_empty() {
        return Err(ArchiveError::InvalidManifestFiles(
            "files[] must not be empty for zip archive components".to_string(),
        ));
    }
    if manifest_files.len() > limits.max_file_count {
        return Err(ArchiveError::TooManyEntries {
            count: manifest_files.len(),
            limit: limits.max_file_count,
        });
    }

    let mut map: HashMap<String, (&ManifestFileEntry, String)> = HashMap::new();
    let mut allowed_dirs: HashSet<String> = HashSet::new();
    let mut total_bytes: u64 = 0;

    for mf in manifest_files {
        let normalized = validate_entry_path(&mf.path, false, staging_dir, limits)?;
        if mf.path != normalized {
            return Err(ArchiveError::UnsafePath {
                path: mf.path.clone(),
                reason: "manifest path must be canonical relative POSIX path".to_string(),
            });
        }
        if mf.size_bytes > limits.max_file_bytes {
            return Err(ArchiveError::SizeLimitExceeded {
                path: mf.path.clone(),
                size: mf.size_bytes,
                limit: limits.max_file_bytes,
            });
        }
        total_bytes = total_bytes.checked_add(mf.size_bytes).ok_or_else(|| {
            ArchiveError::SizeLimitExceeded {
                path: mf.path.clone(),
                size: u64::MAX,
                limit: limits.max_total_bytes,
            }
        })?;
        if total_bytes > limits.max_total_bytes {
            return Err(ArchiveError::SizeLimitExceeded {
                path: mf.path.clone(),
                size: total_bytes,
                limit: limits.max_total_bytes,
            });
        }

        let lower = normalized.to_lowercase();
        if let Some((_, existing_orig)) = map.get(&lower) {
            return Err(ArchiveError::DuplicateOrCollision {
                path: normalized,
                conflicting_with: existing_orig.clone(),
            });
        }
        map.insert(lower, (mf, normalized.clone()));

        // Record allowed ancestor directory prefixes for entries inside folders.
        let mut prefix = normalized.as_str();
        while let Some(idx) = prefix.rfind('/') {
            prefix = &prefix[..idx];
            allowed_dirs.insert(prefix.to_lowercase());
        }
    }

    Ok(ValidatedManifestFiles {
        map,
        allowed_dirs,
        total_bytes,
    })
}

/// Verifies an archive's SHA-256 and extracts ONLY entries declared in `manifest_files`
/// into a same-volume staging directory `bin/<component>/.staging-<uuid>/`.
///
/// Uses default `ArchiveLimits` and queries OS free disk space on the staging parent volume.
pub fn extract_verified_archive(
    archive_path: &Path,
    expected_archive_sha256: &str,
    expected_archive_size: u64,
    component_dir: &Path,
    manifest_files: &[ManifestFileEntry],
) -> Result<StagedArchive, ArchiveError> {
    extract_verified_archive_with_options(
        archive_path,
        expected_archive_sha256,
        expected_archive_size,
        component_dir,
        manifest_files,
        &ArchiveLimits::default(),
        None,
    )
}

/// Configurable extraction entrypoint supporting custom `ArchiveLimits` and deterministic
/// disk-space override in tests.
pub fn extract_verified_archive_with_options(
    archive_path: &Path,
    expected_archive_sha256: &str,
    expected_archive_size: u64,
    component_dir: &Path,
    manifest_files: &[ManifestFileEntry],
    limits: &ArchiveLimits,
    free_space_override: Option<u64>,
) -> Result<StagedArchive, ArchiveError> {
    // 1. Verify archive byte size & SHA-256 checksum strictly before opening ZIP parser.
    let (actual_sha, actual_size) =
        compute_file_sha256_and_size(archive_path).map_err(|e| ArchiveError::Io(e.to_string()))?;

    if actual_size != expected_archive_size {
        return Err(ArchiveError::ArchiveSizeMismatch {
            expected: expected_archive_size,
            actual: actual_size,
        });
    }
    if !actual_sha.eq_ignore_ascii_case(expected_archive_sha256) {
        return Err(ArchiveError::ArchiveIntegrityMismatch {
            expected: expected_archive_sha256.to_string(),
            actual: actual_sha,
        });
    }

    fs::create_dir_all(component_dir).map_err(|e| map_write_io_error(e, 0))?;

    // 2. Reserve staging directory path on the same volume and validate manifest file table.
    let staging_dir = component_dir.join(format!(".staging-{}", Uuid::new_v4()));
    let ValidatedManifestFiles {
        map: manifest_map,
        allowed_dirs,
        total_bytes: total_declared_bytes,
    } = validate_manifest_file_list(manifest_files, &staging_dir, limits)?;

    // 3. Preflight free disk space for uncompressed extraction output.
    let available = match free_space_override {
        Some(avail) => avail,
        None => security::get_free_disk_space(component_dir)
            .map_err(|e| ArchiveError::Io(e.to_string()))?,
    };
    if available < total_declared_bytes {
        return Err(ArchiveError::DiskFull {
            available,
            required: total_declared_bytes,
        });
    }

    fs::create_dir_all(&staging_dir).map_err(|e| map_write_io_error(e, total_declared_bytes))?;
    let guard = StagingDirGuard::new(staging_dir.clone());

    // 4. Open verified archive, audit raw Central Directory headers, and extract entries.
    let mut archive_file = File::open(archive_path).map_err(|e| ArchiveError::Io(e.to_string()))?;
    check_raw_zip_central_directory(&mut archive_file, limits)?;
    let mut zip =
        ZipArchive::new(archive_file).map_err(|e| ArchiveError::InvalidZip(e.to_string()))?;

    let entry_count = zip.len();
    if entry_count > limits.max_file_count {
        return Err(ArchiveError::TooManyEntries {
            count: entry_count,
            limit: limits.max_file_count,
        });
    }

    let mut seen_paths: HashMap<String, String> = HashMap::new();
    let mut extracted_lower: HashSet<String> = HashSet::new();
    let mut extracted_records: Vec<ExtractedFileRecord> = Vec::with_capacity(manifest_map.len());
    let mut total_written: u64 = 0;

    for idx in 0..entry_count {
        let mut entry = zip
            .by_index(idx)
            .map_err(|e| ArchiveError::InvalidZip(e.to_string()))?;

        if entry.name_raw().contains(&0) {
            let lossy = String::from_utf8_lossy(entry.name_raw()).to_string();
            return Err(ArchiveError::UnsafePath {
                path: lossy,
                reason: "raw entry name contains NUL byte".to_string(),
            });
        }

        let raw_name = entry
            .name()
            .map_err(|e| ArchiveError::InvalidZip(e.to_string()))?
            .to_string();
        let is_dir = entry.is_dir();

        check_entry_attributes(&entry, &raw_name, is_dir)?;
        let normalized = validate_entry_path(&raw_name, is_dir, &staging_dir, limits)?;
        let lower = normalized.to_lowercase();

        if let Some(prev) = seen_paths.get(&lower) {
            return Err(ArchiveError::DuplicateOrCollision {
                path: normalized,
                conflicting_with: prev.clone(),
            });
        }
        seen_paths.insert(lower.clone(), normalized.clone());

        if is_dir {
            if entry.size() != 0 {
                return Err(ArchiveError::DeclaredVsActualSizeMismatch {
                    path: normalized,
                    declared: entry.size(),
                    actual: 0,
                });
            }
            if !allowed_dirs.contains(&lower) {
                return Err(ArchiveError::ExtraEntryNotAllowed(normalized));
            }
            let dir_target = staging_dir.join(Path::new(&normalized.replace('/', "\\")));
            fs::create_dir_all(&dir_target)
                .map_err(|e| map_write_io_error(e, total_declared_bytes))?;
            continue;
        }

        // File must be listed in manifest files[].
        let (expected_entry, _) = manifest_map
            .get(&lower)
            .ok_or_else(|| ArchiveError::ExtraEntryNotAllowed(normalized.clone()))?;

        // Check declared size in ZIP header against manifest size.
        let zip_declared_size = entry.size();
        if zip_declared_size != expected_entry.size_bytes {
            return Err(ArchiveError::DeclaredVsActualSizeMismatch {
                path: normalized,
                declared: expected_entry.size_bytes,
                actual: zip_declared_size,
            });
        }
        if zip_declared_size > limits.max_file_bytes {
            return Err(ArchiveError::SizeLimitExceeded {
                path: normalized,
                size: zip_declared_size,
                limit: limits.max_file_bytes,
            });
        }

        // Check compression ratio before decompressing.
        let compressed_size = entry.compressed_size().max(1);
        if zip_declared_size > limits.ratio_grace_bytes {
            let max_allowed_for_compressed =
                compressed_size.saturating_mul(limits.max_compression_ratio);
            if zip_declared_size > max_allowed_for_compressed {
                return Err(ArchiveError::CompressionRatioExceeded {
                    path: normalized,
                    uncompressed: zip_declared_size,
                    compressed: entry.compressed_size(),
                    ratio: zip_declared_size / compressed_size,
                    limit: limits.max_compression_ratio,
                });
            }
        }

        let target_path = staging_dir.join(Path::new(&normalized.replace('/', "\\")));
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| map_write_io_error(e, expected_entry.size_bytes))?;
        }

        let mut out_file = File::create(&target_path)
            .map_err(|e| map_write_io_error(e, expected_entry.size_bytes))?;

        // Stream with strict byte counter + ratio cap even if ZIP header lied about uncompressed size.
        let mut buf = [0u8; 65536];
        let mut written_this_file: u64 = 0;

        loop {
            let n = entry.read(&mut buf).map_err(|e| {
                let msg = e.to_string();
                if msg.contains("declared uncompressed size") {
                    ArchiveError::DeclaredVsActualSizeMismatch {
                        path: normalized.clone(),
                        declared: expected_entry.size_bytes,
                        actual: written_this_file.saturating_add(1),
                    }
                } else {
                    ArchiveError::InvalidZip(format!("{}: {}", normalized, msg))
                }
            })?;
            if n == 0 {
                break;
            }
            written_this_file = written_this_file.saturating_add(n as u64);

            if written_this_file > expected_entry.size_bytes {
                return Err(ArchiveError::DeclaredVsActualSizeMismatch {
                    path: normalized,
                    declared: expected_entry.size_bytes,
                    actual: written_this_file,
                });
            }
            if written_this_file > limits.max_file_bytes {
                return Err(ArchiveError::SizeLimitExceeded {
                    path: normalized,
                    size: written_this_file,
                    limit: limits.max_file_bytes,
                });
            }
            if written_this_file > limits.ratio_grace_bytes
                && written_this_file > compressed_size.saturating_mul(limits.max_compression_ratio)
            {
                return Err(ArchiveError::CompressionRatioExceeded {
                    path: normalized,
                    uncompressed: written_this_file,
                    compressed: entry.compressed_size(),
                    ratio: written_this_file / compressed_size,
                    limit: limits.max_compression_ratio,
                });
            }
            let next_total = total_written.saturating_add(n as u64);
            if next_total > limits.max_total_bytes {
                return Err(ArchiveError::SizeLimitExceeded {
                    path: normalized,
                    size: next_total,
                    limit: limits.max_total_bytes,
                });
            }

            out_file
                .write_all(&buf[..n])
                .map_err(|e| map_write_io_error(e, expected_entry.size_bytes))?;
            total_written = next_total;
        }

        out_file
            .flush()
            .map_err(|e| map_write_io_error(e, expected_entry.size_bytes))?;
        drop(out_file);

        if written_this_file != expected_entry.size_bytes {
            return Err(ArchiveError::DeclaredVsActualSizeMismatch {
                path: normalized,
                declared: expected_entry.size_bytes,
                actual: written_this_file,
            });
        }

        // Verify per-file size and SHA-256 directly on disk after extraction.
        let (disk_sha, disk_size) = compute_file_sha256_and_size(&target_path)
            .map_err(|e| ArchiveError::Io(e.to_string()))?;
        if disk_size != expected_entry.size_bytes {
            return Err(ArchiveError::ExtractedSizeMismatch {
                path: normalized,
                expected: expected_entry.size_bytes,
                actual: disk_size,
            });
        }
        if !disk_sha.eq_ignore_ascii_case(&expected_entry.sha256) {
            return Err(ArchiveError::ExtractedHashMismatch {
                path: normalized,
                expected: expected_entry.sha256.clone(),
                actual: disk_sha,
            });
        }

        extracted_lower.insert(lower);
        extracted_records.push(ExtractedFileRecord {
            path: expected_entry.path.clone(),
            sha256: disk_sha,
            size_bytes: disk_size,
        });
    }

    // 5. Ensure ALL manifest files[] entries were extracted.
    for (lower_key, (mf, _)) in &manifest_map {
        if !extracted_lower.contains(lower_key) {
            return Err(ArchiveError::MissingManifestEntry(mf.path.clone()));
        }
    }

    let committed_dir = guard.commit();
    Ok(StagedArchive {
        staging_dir: committed_dir,
        files: extracted_records,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, System, ZipWriter};

    fn sha256_bytes(data: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(data);
        hex::encode(h.finalize())
    }

    fn build_zip_bytes(entries: Vec<(&str, &[u8], SimpleFileOptions)>) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut w = ZipWriter::new(&mut buf);
            for (name, data, opts) in entries {
                w.start_file(name, opts).expect("start_file");
                w.write_all(data).expect("write_all");
            }
            w.finish().expect("finish zip");
        }
        buf.into_inner()
    }

    fn write_test_zip(dir: &Path, zip_bytes: &[u8]) -> (PathBuf, String, u64) {
        let zip_path = dir.join(format!("test_archive_{}.zip", Uuid::new_v4()));
        fs::write(&zip_path, zip_bytes).expect("write zip");
        (zip_path, sha256_bytes(zip_bytes), zip_bytes.len() as u64)
    }

    #[test]
    fn test_extract_verified_archive_happy_path_and_subdirectory() {
        let tmp = TempDir::new().unwrap();
        let comp_dir = tmp.path().join("bin").join("ffmpeg");

        let bin_bytes = b"MZ_fake_ffmpeg_binary_content_123";
        let dll_bytes = b"MZ_fake_avcodec_dll_content_456";
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        let zip_bytes = build_zip_bytes(vec![
            ("ffmpeg.exe", bin_bytes.as_slice(), opts),
            ("sub/avcodec-63.dll", dll_bytes.as_slice(), opts),
        ]);
        let (zip_path, zip_sha, zip_size) = write_test_zip(tmp.path(), &zip_bytes);

        let files = vec![
            ManifestFileEntry {
                path: "ffmpeg.exe".into(),
                sha256: sha256_bytes(bin_bytes),
                size_bytes: bin_bytes.len() as u64,
            },
            ManifestFileEntry {
                path: "sub/avcodec-63.dll".into(),
                sha256: sha256_bytes(dll_bytes),
                size_bytes: dll_bytes.len() as u64,
            },
        ];

        let staged = extract_verified_archive(&zip_path, &zip_sha, zip_size, &comp_dir, &files)
            .expect("valid archive extraction should succeed");

        assert!(staged.staging_dir.exists());
        assert!(staged
            .staging_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(".staging-"));
        assert_eq!(staged.files.len(), 2);
        assert_eq!(
            fs::read(staged.staging_dir.join("ffmpeg.exe")).unwrap(),
            bin_bytes
        );
        assert_eq!(
            fs::read(staged.staging_dir.join("sub").join("avcodec-63.dll")).unwrap(),
            dll_bytes
        );
    }

    #[test]
    fn test_rejects_unverified_archive_hash_or_size() {
        let tmp = TempDir::new().unwrap();
        let comp_dir = tmp.path().join("bin").join("ffmpeg");
        let payload = b"hello";
        let zip_bytes =
            build_zip_bytes(vec![("ffmpeg.exe", payload, SimpleFileOptions::default())]);
        let (zip_path, zip_sha, zip_size) = write_test_zip(tmp.path(), &zip_bytes);

        let files = vec![ManifestFileEntry {
            path: "ffmpeg.exe".into(),
            sha256: sha256_bytes(payload),
            size_bytes: payload.len() as u64,
        }];

        // Wrong archive SHA
        let err_sha = extract_verified_archive(
            &zip_path,
            "0000000000000000000000000000000000000000000000000000000000000000",
            zip_size,
            &comp_dir,
            &files,
        )
        .unwrap_err();
        assert!(matches!(
            err_sha,
            ArchiveError::ArchiveIntegrityMismatch { .. }
        ));
        assert_eq!(err_sha.error_code(), "E_VERIFY_FAILED");

        // Wrong archive size
        let err_size =
            extract_verified_archive(&zip_path, &zip_sha, zip_size + 1, &comp_dir, &files)
                .unwrap_err();
        assert!(matches!(err_size, ArchiveError::ArchiveSizeMismatch { .. }));
    }

    #[test]
    fn test_rejects_extra_and_missing_manifest_entries() {
        let tmp = TempDir::new().unwrap();
        let comp_dir = tmp.path().join("bin").join("ffmpeg");
        let opts = SimpleFileOptions::default();

        // Case A: Archive has an extra unlisted file (e.g. ffplay.exe)
        let zip_bytes = build_zip_bytes(vec![
            ("ffmpeg.exe", b"main", opts),
            ("ffplay.exe", b"extra", opts),
        ]);
        let (zip_path, zip_sha, zip_size) = write_test_zip(tmp.path(), &zip_bytes);
        let files = vec![ManifestFileEntry {
            path: "ffmpeg.exe".into(),
            sha256: sha256_bytes(b"main"),
            size_bytes: 4,
        }];
        let err_extra =
            extract_verified_archive(&zip_path, &zip_sha, zip_size, &comp_dir, &files).unwrap_err();
        assert!(
            matches!(err_extra, ArchiveError::ExtraEntryNotAllowed(ref p) if p == "ffplay.exe")
        );

        // Case B: Archive is missing a listed file
        let zip_missing = build_zip_bytes(vec![("ffmpeg.exe", b"main", opts)]);
        let (zip_m_path, zip_m_sha, zip_m_size) = write_test_zip(tmp.path(), &zip_missing);
        let files_two = vec![
            ManifestFileEntry {
                path: "ffmpeg.exe".into(),
                sha256: sha256_bytes(b"main"),
                size_bytes: 4,
            },
            ManifestFileEntry {
                path: "ffprobe.exe".into(),
                sha256: sha256_bytes(b"probe"),
                size_bytes: 5,
            },
        ];
        let err_missing =
            extract_verified_archive(&zip_m_path, &zip_m_sha, zip_m_size, &comp_dir, &files_two)
                .unwrap_err();
        assert!(matches!(
            err_missing,
            ArchiveError::MissingManifestEntry(ref p) if p == "ffprobe.exe"
        ));
    }

    #[test]
    fn test_rejects_all_unsafe_entry_paths() {
        let tmp = TempDir::new().unwrap();
        let comp_dir = tmp.path().join("bin").join("comp");
        let opts = SimpleFileOptions::default();

        let malicious_paths = [
            "/abs/evil.exe",
            "../escape.exe",
            "sub/../../escape.exe",
            r"C:\Windows\evil.exe",
            "D:evil.exe",
            r"\\server\share\evil.exe",
            r"sub\backslash.exe",
            "ffmpeg.exe::$DATA",
            "ffmpeg:hidden",
            "CON",
            "PRN.dll",
            "sub/AUX.exe",
            "NUL",
            "COM1",
            "LPT9.txt",
            "trailing.",
            "trailing ",
            "dir./file.exe",
            "dir /file.exe",
            "PROGRA~1/file.exe",
            "SHORT~2.EXE",
        ];

        for bad_path in malicious_paths {
            let mut buf = Cursor::new(Vec::new());
            {
                let mut w = ZipWriter::new(&mut buf);
                w.start_file(bad_path, opts).unwrap();
                w.write_all(b"evil").unwrap();
                w.finish().unwrap();
            }
            let zbytes = buf.into_inner();
            let (zpath, zsha, zsize) = write_test_zip(tmp.path(), &zbytes);

            let files = vec![ManifestFileEntry {
                path: bad_path.to_string(),
                sha256: sha256_bytes(b"evil"),
                size_bytes: 4,
            }];

            let err =
                extract_verified_archive(&zpath, &zsha, zsize, &comp_dir, &files).unwrap_err();
            assert!(
                matches!(err, ArchiveError::UnsafePath { .. }),
                "expected UnsafePath for '{}', got {:?}",
                bad_path,
                err
            );
        }

        // Verify staging dirs were cleaned up (no leftover .staging-*)
        if comp_dir.exists() {
            let leftovers: Vec<_> = fs::read_dir(&comp_dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .collect();
            assert!(
                leftovers.is_empty(),
                "leftover staging dirs: {:?}",
                leftovers
            );
        }
    }

    #[test]
    fn test_rejects_over_long_paths() {
        let tmp = TempDir::new().unwrap();
        let comp_dir = tmp.path().join("bin").join("comp");
        let opts = SimpleFileOptions::default();

        let long_rel = format!("a/{}/file.exe", "x".repeat(200));
        let mut buf = Cursor::new(Vec::new());
        {
            let mut w = ZipWriter::new(&mut buf);
            w.start_file(&long_rel, opts).unwrap();
            w.write_all(b"data").unwrap();
            w.finish().unwrap();
        }
        let zbytes = buf.into_inner();
        let (zpath, zsha, zsize) = write_test_zip(tmp.path(), &zbytes);

        let files = vec![ManifestFileEntry {
            path: long_rel,
            sha256: sha256_bytes(b"data"),
            size_bytes: 4,
        }];

        let err = extract_verified_archive(&zpath, &zsha, zsize, &comp_dir, &files).unwrap_err();
        assert!(matches!(err, ArchiveError::UnsafePath { .. }));
    }

    #[test]
    fn test_rejects_symlinks_junctions_and_non_regular_entries() {
        let tmp = TempDir::new().unwrap();
        let comp_dir = tmp.path().join("bin").join("comp");

        // 1. Unix S_IFLNK symlink via external attributes
        let unix_symlink_opts = SimpleFileOptions::default()
            .system(System::Unix)
            .external_attributes((0o120777 << 16) as u32);
        let zbytes_sym = build_zip_bytes(vec![("ffmpeg.exe", b"/etc/passwd", unix_symlink_opts)]);
        let (p_sym, s_sym, l_sym) = write_test_zip(tmp.path(), &zbytes_sym);
        let err_sym = extract_verified_archive(
            &p_sym,
            &s_sym,
            l_sym,
            &comp_dir,
            &[ManifestFileEntry {
                path: "ffmpeg.exe".into(),
                sha256: sha256_bytes(b"/etc/passwd"),
                size_bytes: 11,
            }],
        )
        .unwrap_err();
        assert!(matches!(err_sym, ArchiveError::SymlinkOrNonRegularEntry(_)));

        // 2. Unix FIFO non-regular entry (S_IFIFO = 0o010000)
        let fifo_opts = SimpleFileOptions::default()
            .system(System::Unix)
            .external_attributes((0o010644 << 16) as u32);
        let zbytes_fifo = build_zip_bytes(vec![("ffmpeg.exe", b"fifo", fifo_opts)]);
        let (p_fifo, s_fifo, l_fifo) = write_test_zip(tmp.path(), &zbytes_fifo);
        let err_fifo = extract_verified_archive(
            &p_fifo,
            &s_fifo,
            l_fifo,
            &comp_dir,
            &[ManifestFileEntry {
                path: "ffmpeg.exe".into(),
                sha256: sha256_bytes(b"fifo"),
                size_bytes: 4,
            }],
        )
        .unwrap_err();
        assert!(matches!(
            err_fifo,
            ArchiveError::SymlinkOrNonRegularEntry(_)
        ));

        // 3. Windows NTFS FILE_ATTRIBUTE_REPARSE_POINT (0x0400) junction attribute
        let reparse_opts = SimpleFileOptions::default()
            .system(System::Dos)
            .external_attributes(0x0400 | 0x0020);
        let zbytes_rep = build_zip_bytes(vec![("ffmpeg.exe", b"junction", reparse_opts)]);
        let (p_rep, s_rep, l_rep) = write_test_zip(tmp.path(), &zbytes_rep);
        let err_rep = extract_verified_archive(
            &p_rep,
            &s_rep,
            l_rep,
            &comp_dir,
            &[ManifestFileEntry {
                path: "ffmpeg.exe".into(),
                sha256: sha256_bytes(b"junction"),
                size_bytes: 8,
            }],
        )
        .unwrap_err();
        assert!(matches!(err_rep, ArchiveError::SymlinkOrNonRegularEntry(_)));
    }

    #[test]
    fn test_rejects_duplicate_names_and_case_insensitive_collisions() {
        let tmp = TempDir::new().unwrap();
        let comp_dir = tmp.path().join("bin").join("comp");
        let opts = SimpleFileOptions::default();

        // 1. Byte-patched raw duplicate file entry inside zip ("ffmpeg.ex2" -> "ffmpeg.exe")
        let mut zbytes_dup = build_zip_bytes(vec![
            ("ffmpeg.exe", b"one", opts),
            ("ffmpeg.ex2", b"two", opts),
        ]);
        let needle = b"ffmpeg.ex2";
        let replacement = b"ffmpeg.exe";
        let mut repl_count = 0;
        for i in 0..=zbytes_dup.len() - needle.len() {
            if &zbytes_dup[i..i + needle.len()] == needle {
                zbytes_dup[i..i + needle.len()].copy_from_slice(replacement);
                repl_count += 1;
            }
        }
        assert_eq!(repl_count, 2, "must patch local & central directory names");
        let (p_dup, s_dup, l_dup) = write_test_zip(tmp.path(), &zbytes_dup);
        let err_dup = extract_verified_archive(
            &p_dup,
            &s_dup,
            l_dup,
            &comp_dir,
            &[ManifestFileEntry {
                path: "ffmpeg.exe".into(),
                sha256: sha256_bytes(b"one"),
                size_bytes: 3,
            }],
        )
        .unwrap_err();
        assert!(
            matches!(
                err_dup,
                ArchiveError::DuplicateOrCollision { .. } | ArchiveError::InvalidZip(_)
            ),
            "expected DuplicateOrCollision or InvalidZip, got {:?}",
            err_dup
        );

        // 2. Case-insensitive collision inside zip (FFmpeg.exe vs ffmpeg.exe)
        let zbytes_case = build_zip_bytes(vec![
            ("ffmpeg.exe", b"one", opts),
            ("FFmpeg.exe", b"two", opts),
        ]);
        let (p_case, s_case, l_case) = write_test_zip(tmp.path(), &zbytes_case);
        let err_case = extract_verified_archive(
            &p_case,
            &s_case,
            l_case,
            &comp_dir,
            &[ManifestFileEntry {
                path: "ffmpeg.exe".into(),
                sha256: sha256_bytes(b"one"),
                size_bytes: 3,
            }],
        )
        .unwrap_err();
        assert!(matches!(
            err_case,
            ArchiveError::DuplicateOrCollision { .. }
        ));
    }

    #[test]
    fn test_limits_file_count_total_size_and_zip_bomb_ratio() {
        let tmp = TempDir::new().unwrap();
        let comp_dir = tmp.path().join("bin").join("comp");
        let opts_deflate =
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        // 1. Zip bomb (256 KB of zeroes compresses to < 300 bytes => > 800:1 ratio)
        let bomb_data = vec![0u8; 256 * 1024];
        let zbytes_bomb = build_zip_bytes(vec![("bomb.dll", &bomb_data, opts_deflate)]);
        let (p_bomb, s_bomb, l_bomb) = write_test_zip(tmp.path(), &zbytes_bomb);
        let err_bomb = extract_verified_archive(
            &p_bomb,
            &s_bomb,
            l_bomb,
            &comp_dir,
            &[ManifestFileEntry {
                path: "bomb.dll".into(),
                sha256: sha256_bytes(&bomb_data),
                size_bytes: bomb_data.len() as u64,
            }],
        )
        .unwrap_err();
        assert!(
            matches!(err_bomb, ArchiveError::CompressionRatioExceeded { .. }),
            "expected CompressionRatioExceeded, got {:?}",
            err_bomb
        );

        // 2. Entry count limit
        let custom_limits = ArchiveLimits {
            max_file_count: 1,
            max_file_bytes: 100,
            max_total_bytes: 150,
            ..ArchiveLimits::default()
        };
        let zbytes_cnt = build_zip_bytes(vec![
            ("a.dll", b"11", opts_deflate),
            ("b.dll", b"22", opts_deflate),
        ]);
        let (p_cnt, s_cnt, l_cnt) = write_test_zip(tmp.path(), &zbytes_cnt);
        let err_cnt = extract_verified_archive_with_options(
            &p_cnt,
            &s_cnt,
            l_cnt,
            &comp_dir,
            &[
                ManifestFileEntry {
                    path: "a.dll".into(),
                    sha256: sha256_bytes(b"11"),
                    size_bytes: 2,
                },
                ManifestFileEntry {
                    path: "b.dll".into(),
                    sha256: sha256_bytes(b"22"),
                    size_bytes: 2,
                },
            ],
            &custom_limits,
            None,
        )
        .unwrap_err();
        assert!(matches!(err_cnt, ArchiveError::TooManyEntries { .. }));

        // 3. Per-file size limit from limits
        let big_payload = vec![0xAB; 101];
        let zbytes_big = build_zip_bytes(vec![("big.dll", &big_payload, opts_deflate)]);
        let (p_big, s_big, l_big) = write_test_zip(tmp.path(), &zbytes_big);
        let err_big = extract_verified_archive_with_options(
            &p_big,
            &s_big,
            l_big,
            &comp_dir,
            &[ManifestFileEntry {
                path: "big.dll".into(),
                sha256: sha256_bytes(&big_payload),
                size_bytes: 101,
            }],
            &custom_limits,
            None,
        )
        .unwrap_err();
        assert!(matches!(err_big, ArchiveError::SizeLimitExceeded { .. }));

        // 4. Total uncompressed size limit across multiple otherwise-valid files (80 + 80 = 160 > 150)
        let total_limits = ArchiveLimits {
            max_file_count: 10,
            max_file_bytes: 100,
            max_total_bytes: 150,
            ..ArchiveLimits::default()
        };
        let p80_a = vec![0x11u8; 80];
        let p80_b = vec![0x22u8; 80];
        let zbytes_tot = build_zip_bytes(vec![
            ("part1.dll", &p80_a, opts_deflate),
            ("part2.dll", &p80_b, opts_deflate),
        ]);
        let (p_tot, s_tot, l_tot) = write_test_zip(tmp.path(), &zbytes_tot);
        let err_tot = extract_verified_archive_with_options(
            &p_tot,
            &s_tot,
            l_tot,
            &comp_dir,
            &[
                ManifestFileEntry {
                    path: "part1.dll".into(),
                    sha256: sha256_bytes(&p80_a),
                    size_bytes: 80,
                },
                ManifestFileEntry {
                    path: "part2.dll".into(),
                    sha256: sha256_bytes(&p80_b),
                    size_bytes: 80,
                },
            ],
            &total_limits,
            None,
        )
        .unwrap_err();
        assert!(matches!(
            err_tot,
            ArchiveError::SizeLimitExceeded {
                size: 160,
                limit: 150,
                ..
            }
        ));
    }

    #[test]
    fn test_rejects_declared_vs_actual_size_lies_and_per_file_hash_mismatch() {
        let tmp = TempDir::new().unwrap();
        let comp_dir = tmp.path().join("bin").join("comp");
        let opts_stored =
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

        // 1. Manifest declares wrong size compared to ZIP entry size
        let payload = b"actual-12-bb";
        let zbytes = build_zip_bytes(vec![("ffmpeg.exe", payload, opts_stored)]);
        let (zpath, zsha, zsize) = write_test_zip(tmp.path(), &zbytes);

        let err_decl = extract_verified_archive(
            &zpath,
            &zsha,
            zsize,
            &comp_dir,
            &[ManifestFileEntry {
                path: "ffmpeg.exe".into(),
                sha256: sha256_bytes(payload),
                size_bytes: 999,
            }],
        )
        .unwrap_err();
        assert!(matches!(
            err_decl,
            ArchiveError::DeclaredVsActualSizeMismatch { .. }
        ));

        // 2. Patch uncompressed_size in ZIP headers directly to lie about actual decompressed stream length
        //    (e.g., ZIP header and manifest BOTH say 100 bytes, while stream is 12 bytes, OR vice versa).
        let actual_deflate_payload = b"1234567890ABCDEFGHIJ1234567890ABCDEFGHIJ";
        let mut patched = build_zip_bytes(vec![(
            "lied.exe",
            actual_deflate_payload,
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )]);
        // Patch the 4-byte LE uncompressed_size (40 = 0x28) in both Local File Header and Central Directory
        let orig_len_le = (actual_deflate_payload.len() as u32).to_le_bytes();
        let fake_len_le = (10u32).to_le_bytes();
        let mut patched_count = 0;
        for i in 0..patched.len().saturating_sub(4) {
            // Only replace exact occurrences preceded by matching crc/compressed bytes isn't ambiguous here since payload is unique,
            // verify local header signature PK\x03\x04 at 0 (uncompressed size is at offset 22..26) and central header PK\x01\x02 (uncompressed size at +24..28)
            if patched[i..i + 4] == [0x50, 0x4b, 0x03, 0x04]
                && patched[i + 22..i + 26] == orig_len_le
            {
                patched[i + 22..i + 26].copy_from_slice(&fake_len_le);
                patched_count += 1;
            }
            if patched[i..i + 4] == [0x50, 0x4b, 0x01, 0x02]
                && patched[i + 24..i + 28] == orig_len_le
            {
                patched[i + 24..i + 28].copy_from_slice(&fake_len_le);
                patched_count += 1;
            }
        }
        assert_eq!(
            patched_count, 2,
            "must patch local + central header uncompressed_size"
        );

        let (p_lied, s_lied, l_lied) = write_test_zip(tmp.path(), &patched);
        let err_lied = extract_verified_archive(
            &p_lied,
            &s_lied,
            l_lied,
            &comp_dir,
            &[ManifestFileEntry {
                path: "lied.exe".into(),
                sha256: sha256_bytes(actual_deflate_payload),
                size_bytes: 10,
            }],
        )
        .unwrap_err();
        assert!(
            matches!(err_lied, ArchiveError::DeclaredVsActualSizeMismatch { .. }),
            "lied stream size exceeding header must be rejected, got {:?}",
            err_lied
        );

        // 2b. Patch uncompressed_size larger than actual stream (header & manifest say 100, stream yields 40 bytes)
        let mut patched_larger = build_zip_bytes(vec![(
            "short.exe",
            actual_deflate_payload,
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )]);
        let inflated_len_le = (100u32).to_le_bytes();
        let mut patched_larger_cnt = 0;
        for i in 0..patched_larger.len().saturating_sub(4) {
            if patched_larger[i..i + 4] == [0x50, 0x4b, 0x03, 0x04]
                && patched_larger[i + 22..i + 26] == orig_len_le
            {
                patched_larger[i + 22..i + 26].copy_from_slice(&inflated_len_le);
                patched_larger_cnt += 1;
            }
            if patched_larger[i..i + 4] == [0x50, 0x4b, 0x01, 0x02]
                && patched_larger[i + 24..i + 28] == orig_len_le
            {
                patched_larger[i + 24..i + 28].copy_from_slice(&inflated_len_le);
                patched_larger_cnt += 1;
            }
        }
        assert_eq!(patched_larger_cnt, 2);
        let (p_short, s_short, l_short) = write_test_zip(tmp.path(), &patched_larger);
        let err_short = extract_verified_archive(
            &p_short,
            &s_short,
            l_short,
            &comp_dir,
            &[ManifestFileEntry {
                path: "short.exe".into(),
                sha256: sha256_bytes(actual_deflate_payload),
                size_bytes: 100,
            }],
        )
        .unwrap_err();
        assert!(matches!(
            err_short,
            ArchiveError::DeclaredVsActualSizeMismatch {
                declared: 100,
                actual: 40,
                ..
            }
        ));

        // 3. Valid size, wrong inner SHA-256
        let err_inner_hash = extract_verified_archive(
            &zpath,
            &zsha,
            zsize,
            &comp_dir,
            &[ManifestFileEntry {
                path: "ffmpeg.exe".into(),
                sha256: "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".into(),
                size_bytes: payload.len() as u64,
            }],
        )
        .unwrap_err();
        assert!(matches!(
            err_inner_hash,
            ArchiveError::ExtractedHashMismatch { .. }
        ));
    }

    #[test]
    fn test_disk_full_during_extraction_returns_e_disk_full() {
        let tmp = TempDir::new().unwrap();
        let comp_dir = tmp.path().join("bin").join("ffmpeg");
        let payload = vec![0x42u8; 4096];
        let zbytes = build_zip_bytes(vec![("ffmpeg.exe", &payload, SimpleFileOptions::default())]);
        let (zpath, zsha, zsize) = write_test_zip(tmp.path(), &zbytes);

        let files = vec![ManifestFileEntry {
            path: "ffmpeg.exe".into(),
            sha256: sha256_bytes(&payload),
            size_bytes: payload.len() as u64,
        }];

        let err = extract_verified_archive_with_options(
            &zpath,
            &zsha,
            zsize,
            &comp_dir,
            &files,
            &ArchiveLimits::default(),
            Some(1024), // Only 1024 bytes available < 4096 required
        )
        .unwrap_err();

        assert!(matches!(err, ArchiveError::DiskFull { .. }));
        assert_eq!(err.error_code(), "E_DISK_FULL");

        // Also verify OS ERROR_DISK_FULL (Win32 code 112) write mapping
        let win_full = io::Error::from_raw_os_error(112);
        let mapped = map_write_io_error(win_full, 4096);
        assert!(matches!(
            mapped,
            ArchiveError::DiskFull { required: 4096, .. }
        ));
        assert_eq!(mapped.error_code(), "E_DISK_FULL");
    }
}
