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
    #[serde(default)]
    pub graceful: bool,
    #[serde(default)]
    pub is_debug_build: bool,
    #[serde(default)]
    pub ended_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
pub struct CrashRecord {
    pub timestamp: f64,
    pub app_version: String,
    pub location: Option<String>,
    pub message_redacted: String,
    pub backtrace_redacted: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
pub struct CrashRecoveryStatus {
    pub unclean_shutdown_detected: bool,
    pub previous_started_at: Option<f64>,
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
/// In debug builds (`!enforce_marker_without_panic`), dev-server restarts without a real
/// panic record are ignored so the crash banner only appears on genuine panics or release crashes.
pub fn begin_session_with_mode(
    logs_dir: &Path,
    app_version: &str,
    enforce_marker_without_panic: bool,
) -> CrashRecoveryStatus {
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

    let ungraceful_marker = previous_marker.as_ref().is_some_and(|m| !m.graceful);
    let unclean = crash_record.is_some() || (enforce_marker_without_panic && ungraceful_marker);

    let new_marker = ActiveSessionMarker {
        started_at: unix_now_secs(),
        app_version: app_version.to_string(),
        pid: std::process::id(),
        graceful: false,
        is_debug_build: cfg!(debug_assertions),
        ended_at: None,
    };
    if let Ok(json) = serde_json::to_string_pretty(&new_marker) {
        let _ = fs::write(&marker_path, json);
    }

    CrashRecoveryStatus {
        unclean_shutdown_detected: unclean,
        previous_started_at: previous_marker.map(|m| m.started_at as f64),
        crash_record,
        banner_dismissed: false,
    }
}

/// Production entrypoint: enforces ungraceful marker detection in release builds (`!cfg!(debug_assertions)`);
/// in debug builds, ignores dev-server kills unless an actual `crash-latest.json` panic record exists.
pub fn begin_session(logs_dir: &Path, app_version: &str) -> CrashRecoveryStatus {
    begin_session_with_mode(logs_dir, app_version, !cfg!(debug_assertions))
}

/// Marks the active session marker as ended cleanly (`graceful = true`).
pub fn mark_clean_shutdown(logs_dir: &Path) {
    let marker_path = logs_dir.join(SESSION_MARKER_FILE);
    let mut marker = fs::read_to_string(&marker_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<ActiveSessionMarker>(&raw).ok())
        .unwrap_or_else(|| ActiveSessionMarker {
            started_at: unix_now_secs(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            pid: std::process::id(),
            graceful: true,
            is_debug_build: cfg!(debug_assertions),
            ended_at: None,
        });
    marker.graceful = true;
    marker.ended_at = Some(unix_now_secs());
    if let Ok(json) = serde_json::to_string_pretty(&marker) {
        let _ = fs::write(&marker_path, json);
    }
}

/// Marks the active session recorded in `PANIC_CONTEXT` as ended cleanly and flushes file logs.
/// Safe to invoke from Windows `WM_QUERYENDSESSION`/`WM_ENDSESSION` and console control handlers.
pub fn handle_end_session_signal() {
    if let Some(ctx_cell) = PANIC_CONTEXT.get() {
        if let Ok(guard) = ctx_cell.lock() {
            mark_clean_shutdown(&guard.0);
        }
    }
    crate::logging::flush_file_logging();
}

static SHUTDOWN_HANDLERS_INSTALLED: OnceLock<()> = OnceLock::new();

#[cfg(windows)]
unsafe extern "system" fn console_ctrl_handler(ctrl_type: u32) -> i32 {
    const CTRL_C_EVENT: u32 = 0;
    const CTRL_BREAK_EVENT: u32 = 1;
    const CTRL_CLOSE_EVENT: u32 = 2;
    const CTRL_LOGOFF_EVENT: u32 = 5;
    const CTRL_SHUTDOWN_EVENT: u32 = 6;
    if matches!(
        ctrl_type,
        CTRL_C_EVENT
            | CTRL_BREAK_EVENT
            | CTRL_CLOSE_EVENT
            | CTRL_LOGOFF_EVENT
            | CTRL_SHUTDOWN_EVENT
    ) {
        handle_end_session_signal();
    }
    0
}

#[cfg(windows)]
unsafe extern "system" fn shutdown_wnd_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, PostQuitMessage, WM_DESTROY, WM_ENDSESSION, WM_QUERYENDSESSION,
    };
    match msg {
        WM_QUERYENDSESSION => {
            handle_end_session_signal();
            1
        }
        WM_ENDSESSION => {
            if wparam != 0 {
                handle_end_session_signal();
            }
            1
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Registers OS shutdown/logoff handlers (`WM_QUERYENDSESSION` / `WM_ENDSESSION` +
/// `SetConsoleCtrlHandler`) so clean system shutdown/logoff never triggers a false crash banner.
pub fn install_shutdown_handlers(logs_dir: PathBuf, app_version: String) {
    let cell = PANIC_CONTEXT.get_or_init(|| Mutex::new((logs_dir.clone(), app_version.clone())));
    if let Ok(mut guard) = cell.lock() {
        *guard = (logs_dir, app_version);
    }

    #[cfg(windows)]
    SHUTDOWN_HANDLERS_INSTALLED.get_or_init(|| {
        use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
        unsafe {
            let _ = SetConsoleCtrlHandler(Some(console_ctrl_handler), 1);
        }
        std::thread::Builder::new()
            .name("clipper-shutdown-listener".into())
            .spawn(|| unsafe {
                use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    CreateWindowExW, DispatchMessageW, GetMessageW, RegisterClassW,
                    TranslateMessage, MSG, WNDCLASSW, WS_OVERLAPPEDWINDOW,
                };
                let class_name: Vec<u16> = "ClipperShutdownWatcher\0".encode_utf16().collect();
                let hinstance = GetModuleHandleW(std::ptr::null());
                let wc = WNDCLASSW {
                    style: 0,
                    lpfnWndProc: Some(shutdown_wnd_proc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: hinstance,
                    hIcon: std::ptr::null_mut(),
                    hCursor: std::ptr::null_mut(),
                    hbrBackground: std::ptr::null_mut(),
                    lpszMenuName: std::ptr::null(),
                    lpszClassName: class_name.as_ptr(),
                };
                if RegisterClassW(&wc) != 0 {
                    let hwnd = CreateWindowExW(
                        0,
                        class_name.as_ptr(),
                        class_name.as_ptr(),
                        WS_OVERLAPPEDWINDOW,
                        0,
                        0,
                        0,
                        0,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        hinstance,
                        std::ptr::null(),
                    );
                    if !hwnd.is_null() {
                        let mut msg: MSG = std::mem::zeroed();
                        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                            TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                    }
                }
            })
            .ok();
    });
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

    let ts_now = unix_now_secs();
    let record = CrashRecord {
        timestamp: ts_now as f64,
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
            ts_now,
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
        let first = begin_session_with_mode(&logs, "0.1.0", true);
        assert!(!first.unclean_shutdown_detected);
        assert!(logs.join(SESSION_MARKER_FILE).exists());

        // 2. Clean exit sets graceful=true -> next release start is clean
        mark_clean_shutdown(&logs);
        let raw_clean: ActiveSessionMarker =
            serde_json::from_str(&fs::read_to_string(logs.join(SESSION_MARKER_FILE)).unwrap())
                .unwrap();
        assert!(raw_clean.graceful);
        assert!(raw_clean.ended_at.is_some());

        let second = begin_session_with_mode(&logs, "0.1.0", true);
        assert!(!second.unclean_shutdown_detected);

        // 3. Simulated crash in release mode (graceful still false) -> detected
        let third = begin_session_with_mode(&logs, "0.1.0", true);
        assert!(third.unclean_shutdown_detected);
        assert!(third.previous_started_at.is_some());

        // 4. In debug mode without a panic record, abrupt dev-server restarts are suppressed
        let debug_suppressed = begin_session_with_mode(&logs, "0.1.0", false);
        assert!(!debug_suppressed.unclean_shutdown_detected);
    }

    #[test]
    fn test_end_session_signal_marks_graceful() {
        let tmp = TempDir::new().unwrap();
        let logs = tmp.path().join("logs");
        let _ = begin_session_with_mode(&logs, "0.1.0", true);
        install_panic_hook(logs.clone(), "0.1.0".to_string());
        handle_end_session_signal();

        let after: ActiveSessionMarker =
            serde_json::from_str(&fs::read_to_string(logs.join(SESSION_MARKER_FILE)).unwrap())
                .unwrap();
        assert!(after.graceful);

        let next = begin_session_with_mode(&logs, "0.1.0", true);
        assert!(!next.unclean_shutdown_detected);
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
