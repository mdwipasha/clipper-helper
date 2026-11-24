//! Sidecar lifecycle supervisor (ARCHITECTURE §4, SECURITY §9).
//!
//! Responsibilities:
//! - Spawn external binaries (`yt-dlp`, `qjs`, `ffmpeg`, `ffprobe`, `whisper`) with argument arrays.
//! - Enclose every process inside a Windows Job Object with `kill-on-close` flag.
//! - Enforce timeouts, output size limits (per-line 4KB cap, total 10MB cap), and parse real-time progress events.
//! - Handle `\r` and `\n` streaming line delimiters for real-time progress updates.
//! - Maintain a redacted rolling 64KB stderr ring buffer.

use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

#[derive(Error, Debug)]
pub enum SidecarError {
    #[error("Binary not found: {0}")]
    BinaryNotFound(PathBuf),

    #[error("Failed to spawn process: {0}")]
    SpawnFailed(String),

    #[error("Failed to create or configure Windows Job Object: code {0}")]
    JobObjectError(u32),

    #[error("Process timed out after {0:?}")]
    Timeout(Duration),

    #[error("Process execution was cancelled")]
    Cancelled,

    #[error("Process failed with exit code {exit_code:?}: {stderr_tail}")]
    ExecutionFailed {
        exit_code: Option<i32>,
        stderr_tail: String,
    },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Specification for launching a sidecar process.
#[derive(Debug, Clone)]
pub struct SidecarSpec {
    pub binary_path: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub timeout: Option<Duration>,
    pub max_line_bytes: usize,
    pub max_total_bytes: usize,
    pub max_stderr_bytes: usize,
    pub below_normal_priority: bool,
}

impl SidecarSpec {
    pub const DEFAULT_MAX_LINE_BYTES: usize = 4096; // 4 KB
    pub const DEFAULT_MAX_TOTAL_BYTES: usize = 10 * 1024 * 1024; // 10 MB
    pub const DEFAULT_MAX_STDERR_BYTES: usize = 64 * 1024; // 64 KB

    pub fn new(
        binary_path: impl Into<PathBuf>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            binary_path: binary_path.into(),
            args: args.into_iter().map(Into::into).collect(),
            env: Vec::new(),
            timeout: None,
            max_line_bytes: Self::DEFAULT_MAX_LINE_BYTES,
            max_total_bytes: Self::DEFAULT_MAX_TOTAL_BYTES,
            max_stderr_bytes: Self::DEFAULT_MAX_STDERR_BYTES,
            below_normal_priority: false,
        }
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn env(mut self, key: impl Into<String>, val: impl Into<String>) -> Self {
        self.env.push((key.into(), val.into()));
        self
    }

    pub fn max_total_bytes(mut self, limit: usize) -> Self {
        self.max_total_bytes = limit;
        self
    }
}

/// Result of a completed sidecar execution.
#[derive(Debug, Clone)]
pub struct SidecarResult {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stdout_lines: Vec<String>,
    pub stderr_tail: String,
    pub total_stdout_bytes: usize,
    pub total_stderr_bytes: usize,
    pub truncated: bool,
}

/// Line splitter that splits on `\r`, `\n`, and `\r\n`, capping lines at `max_line_bytes`.
pub struct LineSplitter {
    buffer: Vec<u8>,
    last_was_cr: bool,
    max_line_bytes: usize,
}

impl LineSplitter {
    pub fn new(max_line_bytes: usize) -> Self {
        Self {
            buffer: Vec::new(),
            last_was_cr: false,
            max_line_bytes,
        }
    }

    pub fn push_bytes(&mut self, bytes: &[u8], mut on_line: impl FnMut(String)) {
        for &b in bytes {
            if b == b'\r' {
                let line = std::mem::take(&mut self.buffer);
                on_line(String::from_utf8_lossy(&line).into_owned());
                self.last_was_cr = true;
            } else if b == b'\n' {
                if self.last_was_cr {
                    // Part of CRLF — ignore this newline
                    self.last_was_cr = false;
                } else {
                    let line = std::mem::take(&mut self.buffer);
                    on_line(String::from_utf8_lossy(&line).into_owned());
                }
            } else {
                self.last_was_cr = false;
                self.buffer.push(b);
                if self.buffer.len() >= self.max_line_bytes {
                    let line = std::mem::take(&mut self.buffer);
                    on_line(String::from_utf8_lossy(&line).into_owned());
                }
            }
        }
    }

    pub fn finish(mut self, mut on_line: impl FnMut(String)) {
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            on_line(String::from_utf8_lossy(&line).into_owned());
        }
    }
}

/// Rolling ring buffer keeping the last `capacity` bytes of stderr.
pub struct StderrRingBuffer {
    buffer: Vec<u8>,
    capacity: usize,
}

impl StderrRingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(capacity.min(4096)),
            capacity,
        }
    }

    pub fn push_bytes(&mut self, bytes: &[u8]) {
        if bytes.len() >= self.capacity {
            self.buffer.clear();
            self.buffer
                .extend_from_slice(&bytes[bytes.len() - self.capacity..]);
        } else {
            self.buffer.extend_from_slice(bytes);
            if self.buffer.len() > self.capacity {
                let excess = self.buffer.len() - self.capacity;
                self.buffer.drain(..excess);
            }
        }
    }

    pub fn to_redacted_string(&self) -> String {
        let raw = String::from_utf8_lossy(&self.buffer);
        crate::security::redact_sensitive_text(&raw)
    }
}

// ── Windows Job Object ──────────────────────────────────────────────────

#[cfg(windows)]
pub struct JobObject {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
unsafe impl Send for JobObject {}
#[cfg(windows)]
unsafe impl Sync for JobObject {}

#[cfg(windows)]
impl JobObject {
    pub fn create() -> Result<Self, u32> {
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(GetLastError());
            }

            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

            let res = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );

            if res == 0 {
                let err = GetLastError();
                CloseHandle(handle);
                return Err(err);
            }

            Ok(Self { handle })
        }
    }

    pub fn assign_process(
        &self,
        process_handle: std::os::windows::io::RawHandle,
    ) -> Result<(), u32> {
        use windows_sys::Win32::Foundation::GetLastError;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;

        unsafe {
            let res = AssignProcessToJobObject(self.handle, process_handle as _);
            if res == 0 {
                return Err(GetLastError());
            }
            Ok(())
        }
    }

    pub fn terminate(&self, exit_code: u32) {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        unsafe {
            TerminateJobObject(self.handle, exit_code);
        }
    }
}

#[cfg(windows)]
impl Drop for JobObject {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        unsafe {
            let _ = TerminateJobObject(self.handle, 1);
            let _ = CloseHandle(self.handle);
        }
    }
}

#[cfg(not(windows))]
pub struct JobObject;

#[cfg(not(windows))]
impl JobObject {
    pub fn create() -> Result<Self, u32> {
        Ok(Self)
    }
    pub fn assign_process(
        &self,
        _process_handle: std::os::windows::io::RawHandle,
    ) -> Result<(), u32> {
        Ok(())
    }
    pub fn terminate(&self, _exit_code: u32) {}
}

#[cfg(windows)]
fn resume_process_threads(pid: u32) -> Result<(), u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(GetLastError());
        }

        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;

        if Thread32First(snapshot, &mut entry) != 0 {
            loop {
                if entry.th32OwnerProcessID == pid {
                    let h_thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !h_thread.is_null() {
                        ResumeThread(h_thread);
                        CloseHandle(h_thread);
                    }
                }
                if Thread32Next(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }

        CloseHandle(snapshot);
        Ok(())
    }
}

// ── Supervisor ──────────────────────────────────────────────────────────

/// Supervisor for spawning and monitoring sidecar child processes.
#[derive(Default)]
pub struct SidecarSupervisor;

impl SidecarSupervisor {
    pub fn new() -> Self {
        Self
    }

    /// Run a sidecar process to completion, collecting stdout and stderr.
    pub async fn run_to_completion(
        &self,
        spec: &SidecarSpec,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<SidecarResult, SidecarError> {
        self.spawn_streaming(spec, |_| {}, cancel_token).await
    }

    /// Spawn a sidecar process and stream stdout lines in real time via `on_stdout_line`.
    pub async fn spawn_streaming<F>(
        &self,
        spec: &SidecarSpec,
        mut on_stdout_line: F,
        cancel_token: Option<&CancellationToken>,
    ) -> Result<SidecarResult, SidecarError>
    where
        F: FnMut(String) + Send + 'static,
    {
        if !spec.binary_path.exists() {
            return Err(SidecarError::BinaryNotFound(spec.binary_path.clone()));
        }

        let job = JobObject::create().map_err(SidecarError::JobObjectError)?;

        let mut cmd = tokio::process::Command::new(&spec.binary_path);
        cmd.args(&spec.args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Sanitized environment: clear and only inherit critical system variables
        cmd.env_clear();
        const ALLOWED_SYSTEM_ENV: &[&str] = &[
            "SYSTEMROOT",
            "SYSTEMDRIVE",
            "PATH",
            "TEMP",
            "TMP",
            "LOCALAPPDATA",
            "COMSPEC",
            "PATHEXT",
        ];
        for &var in ALLOWED_SYSTEM_ENV {
            if let Ok(val) = std::env::var(var) {
                cmd.env(var, val);
            }
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }

        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Threading::{
                BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW, CREATE_SUSPENDED,
            };

            let mut flags = CREATE_NO_WINDOW | CREATE_SUSPENDED;
            if spec.below_normal_priority {
                flags |= BELOW_NORMAL_PRIORITY_CLASS;
            }
            cmd.creation_flags(flags);
        }

        // Spawn child in suspended state (no instructions executed yet)
        let mut child = cmd
            .spawn()
            .map_err(|e| SidecarError::SpawnFailed(e.to_string()))?;

        #[cfg(windows)]
        {
            // Assign to Job Object before resuming
            if let Some(raw) = child.raw_handle() {
                job.assign_process(raw)
                    .map_err(SidecarError::JobObjectError)?;
            }
            // Resume suspended process threads now that it is inside the Job Object
            if let Some(pid) = child.id() {
                resume_process_threads(pid).map_err(SidecarError::JobObjectError)?;
            }
        }

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| SidecarError::SpawnFailed("Failed to capture stdout".into()))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| SidecarError::SpawnFailed("Failed to capture stderr".into()))?;

        let mut captured_stdout = Vec::new();
        let mut captured_lines = Vec::new();
        let mut total_stdout_bytes = 0usize;
        let mut total_stderr_bytes = 0usize;
        let mut truncated = false;

        let mut splitter = LineSplitter::new(spec.max_line_bytes);
        let mut stderr_ring = StderrRingBuffer::new(spec.max_stderr_bytes);

        let mut stdout_buf = vec![0u8; 4096];
        let mut stderr_buf = vec![0u8; 4096];

        let mut stdout_done = false;
        let mut stderr_done = false;
        let mut exit_status: Option<std::process::ExitStatus> = None;

        let timeout_duration = spec.timeout.unwrap_or(Duration::from_secs(365 * 24 * 3600));

        // Sleep future for timeout
        tokio::pin! {
            let timeout_sleep = tokio::time::sleep(timeout_duration);
        }

        let mut post_exit_drain = false;
        let mut drain_deadline = tokio::time::Instant::now() + Duration::from_secs(365 * 24 * 3600);

        while !stdout_done || !stderr_done || exit_status.is_none() {
            tokio::select! {
                _ = &mut timeout_sleep, if spec.timeout.is_some() => {
                    job.terminate(1);
                    let _ = child.kill().await;
                    return Err(SidecarError::Timeout(spec.timeout.unwrap()));
                }
                _ = tokio::time::sleep_until(drain_deadline), if post_exit_drain => {
                    // Child already exited; drain window expired (likely inherited handles by grandchild)
                    stdout_done = true;
                    stderr_done = true;
                }
                _ = async {
                    if let Some(token) = cancel_token {
                        token.cancelled().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    job.terminate(1);
                    let _ = child.kill().await;
                    return Err(SidecarError::Cancelled);
                }
                res = stdout.read(&mut stdout_buf), if !stdout_done => {
                    match res {
                        Ok(0) => {
                            stdout_done = true;
                        }
                        Ok(n) => {
                            total_stdout_bytes += n;
                            if total_stdout_bytes <= spec.max_total_bytes {
                                captured_stdout.extend_from_slice(&stdout_buf[..n]);
                                splitter.push_bytes(&stdout_buf[..n], |line| {
                                    captured_lines.push(line.clone());
                                    on_stdout_line(line);
                                });
                            } else if !truncated {
                                truncated = true;
                                job.terminate(1);
                                let _ = child.kill().await;
                                stdout_done = true;
                            }
                        }
                        Err(e) => {
                            tracing::warn!("stdout read error: {}", e);
                            stdout_done = true;
                        }
                    }
                }
                res = stderr.read(&mut stderr_buf), if !stderr_done => {
                    match res {
                        Ok(0) => {
                            stderr_done = true;
                        }
                        Ok(n) => {
                            total_stderr_bytes += n;
                            stderr_ring.push_bytes(&stderr_buf[..n]);
                        }
                        Err(e) => {
                            tracing::warn!("stderr read error: {}", e);
                            stderr_done = true;
                        }
                    }
                }
                res = child.wait(), if exit_status.is_none() => {
                    match res {
                        Ok(status) => {
                            exit_status = Some(status);
                            post_exit_drain = true;
                            drain_deadline = tokio::time::Instant::now() + Duration::from_millis(150);
                        }
                        Err(e) => {
                            return Err(SidecarError::Io(e));
                        }
                    }
                }
            }
        }

        splitter.finish(|line| {
            captured_lines.push(line.clone());
            on_stdout_line(line);
        });

        let stderr_tail = stderr_ring.to_redacted_string();
        let code = exit_status.and_then(|s| s.code()).unwrap_or(-1);

        if code != 0 && !truncated {
            return Err(SidecarError::ExecutionFailed {
                exit_code: Some(code),
                stderr_tail,
            });
        }

        Ok(SidecarResult {
            exit_code: code,
            stdout: captured_stdout,
            stdout_lines: captured_lines,
            stderr_tail,
            total_stdout_bytes,
            total_stderr_bytes,
            truncated,
        })
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Locate the fake-sidecar executable.
    fn fake_sidecar_path() -> PathBuf {
        let current_exe = std::env::current_exe().expect("get current exe");
        // current_exe is target\debug\deps\clipper_helper_desktop_lib-xxxx.exe
        let target_dir = current_exe
            .parent()
            .and_then(Path::parent)
            .expect("target debug dir");
        let bin_path = target_dir.join("fake-sidecar.exe");
        if !bin_path.exists() {
            // Build it if not already present
            let status = std::process::Command::new("cargo")
                .args(["build", "-p", "fake-sidecar"])
                .status()
                .expect("build fake-sidecar");
            assert!(status.success(), "cargo build -p fake-sidecar failed");
        }
        bin_path
    }

    #[cfg(windows)]
    fn is_pid_alive(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut exit_code: u32 = 0;
            let res = GetExitCodeProcess(handle, &mut exit_code);
            CloseHandle(handle);
            res != 0 && exit_code == STILL_ACTIVE as u32
        }
    }

    #[tokio::test]
    async fn test_normal_exit() {
        let bin = fake_sidecar_path();
        let spec = SidecarSpec::new(&bin, ["--mode", "normal"]);
        let supervisor = SidecarSupervisor::new();
        let res = supervisor
            .run_to_completion(&spec, None)
            .await
            .expect("run normal");
        assert_eq!(res.exit_code, 0);
        assert!(
            String::from_utf8_lossy(&res.stdout).contains("hello"),
            "got stdout: {:?}",
            String::from_utf8_lossy(&res.stdout)
        );
    }

    #[tokio::test]
    async fn test_nonzero_exit_code() {
        let bin = fake_sidecar_path();
        let spec = SidecarSpec::new(&bin, ["--mode", "exit-code", "42"]);
        let supervisor = SidecarSupervisor::new();
        let err = supervisor
            .run_to_completion(&spec, None)
            .await
            .expect_err("should fail");
        match err {
            SidecarError::ExecutionFailed { exit_code, .. } => {
                assert_eq!(exit_code, Some(42));
            }
            other => panic!("expected ExecutionFailed, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_progress_streaming_with_carriage_return() {
        let bin = fake_sidecar_path();
        let spec = SidecarSpec::new(&bin, ["--mode", "progress"]);
        let supervisor = SidecarSupervisor::new();

        let line_count = Arc::new(AtomicUsize::new(0));
        let count_clone = Arc::clone(&line_count);

        let res = supervisor
            .spawn_streaming(
                &spec,
                move |_line| {
                    count_clone.fetch_add(1, Ordering::SeqCst);
                },
                None,
            )
            .await
            .expect("stream progress");

        assert_eq!(res.exit_code, 0);
        // fake-sidecar emits 11 progress lines (0% to 100%) + "done"
        assert!(
            line_count.load(Ordering::SeqCst) >= 11,
            "expected >= 11 lines streamed, got: {}",
            line_count.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn test_timeout_kills_process() {
        let bin = fake_sidecar_path();
        let spec = SidecarSpec::new(&bin, ["--mode", "sleep", "10000"])
            .timeout(Duration::from_millis(300));
        let supervisor = SidecarSupervisor::new();

        let start = std::time::Instant::now();
        let err = supervisor
            .run_to_completion(&spec, None)
            .await
            .expect_err("should time out");

        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(3000),
            "timeout should trigger quickly, elapsed: {:?}",
            elapsed
        );
        match err {
            SidecarError::Timeout(d) => {
                assert_eq!(d, Duration::from_millis(300));
            }
            other => panic!("expected Timeout, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_cancellation_kills_process() {
        let bin = fake_sidecar_path();
        let spec = SidecarSpec::new(&bin, ["--mode", "sleep", "10000"]);
        let supervisor = SidecarSupervisor::new();

        let token = CancellationToken::new();
        let token_clone = token.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            token_clone.cancel();
        });

        let err = supervisor
            .run_to_completion(&spec, Some(&token))
            .await
            .expect_err("should be cancelled");

        match err {
            SidecarError::Cancelled => {}
            other => panic!("expected Cancelled, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_flood_output_is_capped() {
        let bin = fake_sidecar_path();
        // Set limit to 64 KB for fast test
        let spec = SidecarSpec::new(&bin, ["--mode", "flood"]).max_total_bytes(64 * 1024);
        let supervisor = SidecarSupervisor::new();

        let res = supervisor
            .run_to_completion(&spec, None)
            .await
            .expect("should terminate on cap");

        assert!(res.truncated, "result should be marked truncated");
        assert!(
            res.total_stdout_bytes >= 64 * 1024,
            "total stdout bytes: {}",
            res.total_stdout_bytes
        );
    }

    #[tokio::test]
    async fn test_invalid_utf8_handled_lossily() {
        let bin = fake_sidecar_path();
        let spec = SidecarSpec::new(&bin, ["--mode", "invalid-utf8"]);
        let supervisor = SidecarSupervisor::new();

        let res = supervisor
            .run_to_completion(&spec, None)
            .await
            .expect("should handle invalid utf8 without error");

        assert_eq!(res.exit_code, 0);
        // Lines should contain replacement characters without panic
        assert!(!res.stdout_lines.is_empty());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn test_grandchild_killed_via_job_object() {
        let bin = fake_sidecar_path();
        let spec = SidecarSpec::new(&bin, ["--mode", "grandchild"]);
        let supervisor = SidecarSupervisor::new();

        let res = supervisor
            .run_to_completion(&spec, None)
            .await
            .expect("run grandchild mode");

        // stdout has "grandchild pid: 12345"
        let stdout_str = String::from_utf8_lossy(&res.stdout);
        let pid_line = stdout_str
            .lines()
            .find(|l| l.contains("grandchild pid:"))
            .expect("must output grandchild pid");

        let pid: u32 = pid_line
            .split(':')
            .nth(1)
            .expect("pid value")
            .trim()
            .parse()
            .expect("valid pid");

        assert!(pid > 0, "valid process ID");

        // The supervisor has finished, which dropped the JobObject with KILL_ON_JOB_CLOSE.
        // Wait up to 2 seconds for OS process cleanup of the grandchild.
        let mut dead = false;
        for _ in 0..20 {
            if !is_pid_alive(pid) {
                dead = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        assert!(
            dead,
            "grandchild process with PID {} should be dead after JobObject was dropped",
            pid
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn test_abrupt_app_kill_terminates_job_tree() {
        use std::io::BufRead;

        let bin = fake_sidecar_path();
        let mut supervisor = std::process::Command::new(&bin)
            .args(["--mode", "supervisor-sim"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn supervisor-sim");

        let out = supervisor.stdout.take().expect("take supervisor stdout");
        let mut reader = std::io::BufReader::new(out);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .expect("read supervisor ready line");

        let parts: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(
            parts.first(),
            Some(&"SUPERVISOR_READY"),
            "line was: {}",
            line
        );

        let child_pid: u32 = parts[1]
            .split('=')
            .nth(1)
            .expect("child pid")
            .parse()
            .expect("parse child pid");
        let grandchild_pid: u32 = parts[2]
            .split('=')
            .nth(1)
            .expect("grandchild pid")
            .parse()
            .expect("parse grandchild pid");

        assert!(child_pid > 0);
        assert!(grandchild_pid > 0);

        let status = supervisor.wait().expect("wait supervisor exit");
        assert_eq!(status.code(), Some(42));

        let mut child_dead = false;
        let mut grandchild_dead = false;
        for _ in 0..30 {
            if !child_dead && !is_pid_alive(child_pid) {
                child_dead = true;
            }
            if !grandchild_dead && !is_pid_alive(grandchild_pid) {
                grandchild_dead = true;
            }
            if child_dead && grandchild_dead {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        assert!(
            child_dead,
            "child process with PID {} must be dead after supervisor abrupt exit",
            child_pid
        );
        assert!(
            grandchild_dead,
            "grandchild process with PID {} must be dead after supervisor abrupt exit",
            grandchild_pid
        );
    }
}
