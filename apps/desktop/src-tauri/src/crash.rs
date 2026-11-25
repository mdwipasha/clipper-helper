//! Crash capture: Rust panic hook + unclean-shutdown session marker (M1.1d-lite Part 4b).
//!
//! Writes redacted panic details + backtrace into `<logs_dir>/crash-latest.json` and appends a
//! redacted line to `<logs_dir>/clipper-crash.log`, while managing `<logs_dir>/session.active.json`
//! so unclean exits are surfaced cleanly on next startup.

use std::backtrace::Backtrace;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::security;

pub const SESSION_MARKER_FILE: &str = "session.active.json";
pub const CRASH_RECORD_FILE: &str = "crash-latest.json";
pub const CRASH_LOG_FILE: &str = "clipper-crash.log";
pub const MAX_PANIC_BACKTRACE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveSessionMarker {
    pub started_at: u64,
    pub app_version: String,
    pub pid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct CrashRecord {
    pub timestamp: u64,
    pub app_version: String,
    pub location: Option<String>,
    pub message_redacted: String,
    pub backtrace_redacted: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct CrashRecoveryStatus {
    pub unclean_shutdown_detected: bool,
    pub previous_started_at: Option<u64>,
    pub crash_record: Option<CrashRecord>,
    pub banner_dismissed: bool,
}

static PANIC_CONTEXT: OnceLock<Mutex<(PathBuf, String)>> = OnceLock::new();
static PANIC_HOOK_INSTALLED: OnceLock<()> = OnceLock::new();

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn truncate_utf8(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n...[TRUNCATED]", &s[..end])
}

/// Inspects previous session state BEFORE writing a fresh active session marker.
/// Returns `CrashRecoveryStatus` indicating whether the previous run exited cleanly.
pub fn begin_session(logs_dir: &Path, app_version: &str) -> CrashRecoveryStatus {
    let _ = fs::create_dir_all(logs_dir);
    let marker_path = logs_dir.join(SESSION_MARKER_FILE);
    let crash_path = logs_dir.join(CRASH_RECORD_FILE);

    let previous_marker = fs::read_to_string(&marker_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<ActiveSessionMarker>(&raw).ok());

    let crash_record = fs::read_to_string(&crash_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<CrashRecord>(&raw).ok())
        .map(|mut rec| {
            rec.message_redacted = security::redact_sensitive_text(&rec.message_redacted);
            rec.backtrace_redacted = security::redact_sensitive_text(&rec.backtrace_redacted);
            if let Some(loc) = rec.location.take() {
                rec.location = Some(security::redact_sensitive_text(&loc));
            }
            rec
        });

    let unclean = marker_path.exists() || crash_record.is_some();

    let new_marker = ActiveSessionMarker {
        started_at: unix_now_secs(),
        app_version: app_version.to_string(),
        pid: std::process::id(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&new_marker) {
        let _ = fs::write(&marker_path, json);
    }

    CrashRecoveryStatus {
        unclean_shutdown_detected: unclean,
        previous_started_at: previous_marker.map(|m| m.started_at),
        crash_record,
        banner_dismissed: false,
    }
}

/// Removes the active session marker on clean shutdown.
pub fn mark_clean_shutdown(logs_dir: &Path) {
    let marker_path = logs_dir.join(SESSION_MARKER_FILE);
    let _ = fs::remove_file(marker_path);
}

/// Reads any persisted latest crash record from disk (for inclusion in diagnostics reports).
pub fn read_latest_crash_record(logs_dir: &Path) -> Option<CrashRecord> {
    let crash_path = logs_dir.join(CRASH_RECORD_FILE);
    fs::read_to_string(crash_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<CrashRecord>(&raw).ok())
}

/// Removes `crash-latest.json` once acknowledged/archived so the banner does not loop across future launches.
pub fn clear_crash_record_file(logs_dir: &Path) {
    let crash_path = logs_dir.join(CRASH_RECORD_FILE);
    let _ = fs::remove_file(crash_path);
}

fn write_redacted_crash_files(
    logs_dir: &Path,
    app_version: &str,
    location: Option<&str>,
    raw_message: &str,
    raw_backtrace: &str,
) {
    let _ = fs::create_dir_all(logs_dir);

    let msg_redacted = truncate_utf8(&security::redact_sensitive_text(raw_message), 4096);
    let bt_redacted = truncate_utf8(
        &security::redact_sensitive_text(raw_backtrace),
        MAX_PANIC_BACKTRACE_BYTES,
    );
    let loc_redacted = location.map(security::redact_sensitive_text);

    let record = CrashRecord {
        timestamp: unix_now_secs(),
        app_version: app_version.to_string(),
        location: loc_redacted.clone(),
        message_redacted: msg_redacted.clone(),
        backtrace_redacted: bt_redacted,
    };

    if let Ok(json) = serde_json::to_string_pretty(&record) {
        let _ = fs::write(logs_dir.join(CRASH_RECORD_FILE), json);
    }

    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs_dir.join(CRASH_LOG_FILE))
    {
        let _ = writeln!(
            file,
            "[ts={}] PANIC app={} loc={} msg={}",
            record.timestamp,
            record.app_version,
            loc_redacted.as_deref().unwrap_or("unknown"),
            msg_redacted.replace('\n', " ")
        );
    }
}

/// Installs a global Rust panic hook writing redacted panic details + backtrace into `logs_dir`.
/// Safe to call multiple times (updates target `logs_dir` & installs hook once).
pub fn install_panic_hook(logs_dir: PathBuf, app_version: String) {
    let cell = PANIC_CONTEXT.get_or_init(|| Mutex::new((logs_dir.clone(), app_version.clone())));
    if let Ok(mut guard) = cell.lock() {
        *guard = (logs_dir, app_version);
    }

    PANIC_HOOK_INSTALLED.get_or_init(|| {
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let payload_msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = info.payload().downcast_ref::<String>() {
                s.clone()
            } else {
                "Box<dyn Any> panic payload".to_string()
            };

            let loc = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));

            let bt = Backtrace::force_capture().to_string();

            if let Some(ctx_cell) = PANIC_CONTEXT.get() {
                if let Ok(guard) = ctx_cell.lock() {
                    write_redacted_crash_files(
                        &guard.0,
                        &guard.1,
                        loc.as_deref(),
                        &payload_msg,
                        &bt,
                    );
                }
            }

            prev_hook(info);
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    #[test]
    fn test_unclean_shutdown_marker_lifecycle() {
        let tmp = TempDir::new().unwrap();
        let logs = tmp.path().join("logs");

        // 1. Fresh start -> no unclean shutdown
        let first = begin_session(&logs, "0.1.0");
        assert!(!first.unclean_shutdown_detected);
        assert!(logs.join(SESSION_MARKER_FILE).exists());

        // 2. Clean exit clears marker -> next start is clean
        mark_clean_shutdown(&logs);
        assert!(!logs.join(SESSION_MARKER_FILE).exists());
        let second = begin_session(&logs, "0.1.0");
        assert!(!second.unclean_shutdown_detected);

        // 3. Simulated crash (without calling mark_clean_shutdown) -> next start detects unclean shutdown
        let third = begin_session(&logs, "0.1.0");
        assert!(third.unclean_shutdown_detected);
        assert!(third.previous_started_at.is_some());
    }

    #[test]
    #[ignore]
    fn child_panic_fixture() {
        let logs_dir = PathBuf::from(
            std::env::var("CLIPPER_TEST_PANIC_LOGS").expect("CLIPPER_TEST_PANIC_LOGS required"),
        );
        let _ = begin_session(&logs_dir, "0.1.0-test");
        install_panic_hook(logs_dir, "0.1.0-test".to_string());
        panic!(
            "Simulated panic C:\\Users\\DirtyUserName\\video.mp4 email=leak@example.com Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxIn0.sig1234567890"
        );
    }

    #[test]
    fn test_real_child_process_panic_writes_redacted_crash_record() {
        let tmp = TempDir::new().unwrap();
        let logs_dir = tmp.path().join("logs");
        let exe = std::env::current_exe().expect("current test binary path");

        let output = Command::new(&exe)
            .arg("--exact")
            .arg("crash::tests::child_panic_fixture")
            .arg("--ignored")
            .arg("--nocapture")
            .env("CLIPPER_TEST_PANIC_LOGS", &logs_dir)
            .output()
            .expect("spawn child test process");

        assert!(
            !output.status.success(),
            "Panicking child should exit non-zero"
        );

        let crash_json_path = logs_dir.join(CRASH_RECORD_FILE);
        assert!(
            crash_json_path.exists(),
            "Panic hook must write crash-latest.json"
        );
        let raw_json = fs::read_to_string(&crash_json_path).unwrap();
        let rec: CrashRecord = serde_json::from_str(&raw_json).unwrap();
        assert_eq!(rec.app_version, "0.1.0-test");
        assert!(!rec.message_redacted.contains("DirtyUserName"));
        assert!(!rec.message_redacted.contains("leak@example.com"));
        assert!(!rec.message_redacted.contains("eyJhbGci"));
        assert!(rec.message_redacted.contains("%USERPROFILE%"));
        assert!(rec.message_redacted.contains("[REDACTED_EMAIL]"));
        assert!(!rec.backtrace_redacted.is_empty());

        // Next startup sees unclean_shutdown_detected = true AND crash_record
        let status = begin_session(&logs_dir, "0.1.0");
        assert!(status.unclean_shutdown_detected);
        assert!(status.crash_record.is_some());
    }
}
