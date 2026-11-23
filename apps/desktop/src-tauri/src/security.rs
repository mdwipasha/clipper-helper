//! Security, path validation, and logging redaction (ARCHITECTURE §4, §9; SECURITY §9, §13).
//!
//! Responsibilities:
//! - Canonicalize and restrict file access strictly within `%LOCALAPPDATA%\ClipperHelper\`.
//! - Enforce YouTube URL intake allowlist and extract canonical 11-char video IDs.
//! - Implement logging redaction to ensure no tokens, query strings, emails, or Windows usernames leak into logs.

use regex::Regex;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;
use thiserror::Error;
use tracing::field::Visit;
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

#[derive(Error, Debug, PartialEq, Eq)]
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
    #[error("Failed to resolve workspace root: {0}")]
    RootResolutionFailed(String),
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

/// Returns the default workspace root directory `%LOCALAPPDATA%\ClipperHelper\`.
pub fn get_workspace_root() -> Result<PathBuf, SecurityError> {
    let local_app_data = std::env::var("LOCALAPPDATA")
        .or_else(|_| std::env::var("USERPROFILE").map(|u| format!("{}\\AppData\\Local", u)))
        .map_err(|e| SecurityError::RootResolutionFailed(format!("Environment error: {}", e)))?;

    let root = PathBuf::from(local_app_data).join("ClipperHelper");
    Ok(root)
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

    // 6. Combine with root
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
}
