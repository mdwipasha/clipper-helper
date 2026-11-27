//! Persistent job runner, monotonic progress/ETA event coalescer, cancellation registry,
//! and startup crash reconciliation (ARCHITECTURE §4, §5.1; DATA_MODEL §9; PROGRESS_AND_ETA §2–§3).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use specta::Type;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::security;
use crate::store::{JobRecord, JobRepo, LocalStore, StoreError};

/// Minimum interval between non-transition progress events (`<= 4 events/second` per PROGRESS_AND_ETA §3).
pub const PROGRESS_COALESCE_INTERVAL: Duration = Duration::from_millis(250);
/// Heartbeat cadence while a job stage is executing (`PROGRESS_AND_ETA §3`).
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);

pub const JOB_KIND_GENERATE: &str = "generate";

/// Generate pipeline stage IDs per `PROGRESS_AND_ETA §2.1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum GenerateStageId {
    Probe,
    Download,
    PreparePreview,
    Build,
}

impl GenerateStageId {
    pub fn as_str(self) -> &'static str {
        match self {
            GenerateStageId::Probe => "probe",
            GenerateStageId::Download => "download",
            GenerateStageId::PreparePreview => "prepare_preview",
            GenerateStageId::Build => "build",
        }
    }

    pub fn label_key(self) -> &'static str {
        match self {
            GenerateStageId::Probe => "job.stage.probe",
            GenerateStageId::Download => "job.stage.download",
            GenerateStageId::PreparePreview => "job.stage.preparePreview",
            GenerateStageId::Build => "job.stage.build",
        }
    }

    pub fn unit(self) -> &'static str {
        match self {
            GenerateStageId::Probe => "steps",
            GenerateStageId::Download => "media_seconds",
            GenerateStageId::PreparePreview => "media_seconds",
            GenerateStageId::Build => "steps",
        }
    }
}

#[derive(Error, Debug)]
pub enum JobError {
    #[error("Job not found: {0}")]
    NotFound(String),
    #[error("Another generate job is already running ({active_job_id})")]
    Busy { active_job_id: String },
    #[error("Job is not in a retryable state: {0}")]
    NotRetryable(String),
    #[error("Job execution cancelled: {0}")]
    Cancelled(String),
    #[error("Store error: {0}")]
    Store(#[from] StoreError),
}

impl JobError {
    pub fn error_code(&self) -> &'static str {
        match self {
            JobError::NotFound(_) => "E_JOB_NOT_FOUND",
            JobError::Busy { .. } => "E_JOB_BUSY",
            JobError::NotRetryable(_) => "E_JOB_INVALID_STATE",
            JobError::Cancelled(_) => "E_JOB_CANCELLED",
            JobError::Store(_) => "E_UNKNOWN",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobEventStage {
    pub id: String,
    pub index: u32,
    pub count: u32,
    pub label_key: String,
    pub label_args: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobEventStageProgress {
    pub done: f64,
    pub total: Option<f64>,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobEventOverall {
    pub percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobEventRate {
    pub value: f64,
    pub unit: String,
    pub smoothed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobEventEta {
    pub seconds: f64,
    pub low_seconds: f64,
    pub high_seconds: f64,
    pub confidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobEventError {
    pub code: String,
    pub message_key: String,
    pub retryable: bool,
    pub action_keys: Vec<String>,
}

/// Typed `JobEvent` payload matching `PROGRESS_AND_ETA §3` emitted to the React UI via `tauri-specta`.
#[derive(Debug, Clone, Serialize, Deserialize, Type, tauri_specta::Event, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobEventPayload {
    pub job_id: String,
    pub project_id: String,
    pub kind: String,
    pub state: String,
    pub stage: JobEventStage,
    pub stage_progress: JobEventStageProgress,
    pub overall: JobEventOverall,
    pub rate: Option<JobEventRate>,
    pub eta: Option<JobEventEta>,
    pub detail: Option<HashMap<String, String>>,
    pub error: Option<JobEventError>,
    pub ts: f64,
}

/// Compute stable generate idempotency key covering project, range, format selector, and handles.
pub fn compute_generate_idempotency_key(
    project_id: &str,
    provider: &str,
    video_id: &str,
    range_start_ms: u32,
    range_end_ms: u32,
    format_selector: &str,
    handle_seconds: u32,
) -> String {
    let mut hasher = Sha256::new();
    let input = format!(
        "{}|{}|{}|{}|{}|{}|{}",
        project_id,
        provider,
        video_id,
        range_start_ms,
        range_end_ms,
        format_selector,
        handle_seconds
    );
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

/// Computes stage weight & overall percent monotonically across 3-stage (no proxy) or 4-stage (proxy) generate runs.
pub fn compute_generate_overall_percent(
    stage: GenerateStageId,
    stage_fraction: f64,
    needs_proxy: bool,
) -> f64 {
    let f = stage_fraction.clamp(0.0, 1.0);
    let pct = if needs_proxy {
        // probe: 5%, download: 85%, prepare_preview: 8%, build: 2%
        match stage {
            GenerateStageId::Probe => 5.0 * f,
            GenerateStageId::Download => 5.0 + 85.0 * f,
            GenerateStageId::PreparePreview => 90.0 + 8.0 * f,
            GenerateStageId::Build => 98.0 + 2.0 * f,
        }
    } else {
        // probe: 5%, download: 92%, build: 3% (so 5 + 92 = 97 -> build completes to 100 without backwards step)
        match stage {
            GenerateStageId::Probe => 5.0 * f,
            GenerateStageId::Download => 5.0 + 92.0 * f,
            GenerateStageId::PreparePreview => 97.0,
            GenerateStageId::Build => 97.0 + 3.0 * f,
        }
    };
    pct.clamp(0.0, 100.0)
}

pub fn stage_index_and_count(stage: GenerateStageId, needs_proxy: bool) -> (u32, u32) {
    if needs_proxy {
        match stage {
            GenerateStageId::Probe => (0, 4),
            GenerateStageId::Download => (1, 4),
            GenerateStageId::PreparePreview => (2, 4),
            GenerateStageId::Build => (3, 4),
        }
    } else {
        match stage {
            GenerateStageId::Probe => (0, 3),
            GenerateStageId::Download => (1, 3),
            GenerateStageId::PreparePreview => (1, 3),
            GenerateStageId::Build => (2, 3),
        }
    }
}

/// Returns the isolated per-job temp directory path (`cache/tmp/<job_id>`).
pub fn job_temp_dir(ws_root: &Path, job_id: &str) -> Result<PathBuf, JobError> {
    if job_id.is_empty()
        || !job_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(JobError::NotFound(format!("Invalid job id '{}'", job_id)));
    }
    let rel = format!("cache/tmp/{}", job_id);
    security::resolve_workspace_path_with_root(ws_root, &rel)
        .map_err(|e| JobError::Store(StoreError::Internal(e.to_string())))
}

/// Removes `cache/tmp/<job_id>` if present; best-effort cleanup of empty parent `cache/tmp`.
pub fn cleanup_job_temp_dir(ws_root: &Path, job_id: &str) {
    if let Ok(dir) = job_temp_dir(ws_root, job_id) {
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// Startup reconciliation result reporting interrupted leftover jobs & removed temp dirs.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobReconciliationReport {
    pub interrupted_job_ids: Vec<String>,
    pub cleaned_temp_dirs: usize,
}

/// Enforces ordered, monotonic, coalesced (`<= 4/s`), terminal-safe `JobEventPayload` emission
/// and SQLite progress persistence for a single active job.
pub struct JobEventTracker {
    store: LocalStore,
    job_id: String,
    project_id: String,
    kind: String,
    last_overall_percent: f64,
    last_stage_id: Option<String>,
    last_state: Option<String>,
    last_emit_at: Option<Instant>,
    terminal: bool,
    sink: Arc<dyn Fn(JobEventPayload) + Send + Sync>,
}

impl JobEventTracker {
    pub fn new(
        store: LocalStore,
        job_id: impl Into<String>,
        project_id: impl Into<String>,
        kind: impl Into<String>,
        sink: Arc<dyn Fn(JobEventPayload) + Send + Sync>,
    ) -> Self {
        Self {
            store,
            job_id: job_id.into(),
            project_id: project_id.into(),
            kind: kind.into(),
            last_overall_percent: 0.0,
            last_stage_id: None,
            last_state: None,
            last_emit_at: None,
            terminal: false,
            sink,
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.terminal
    }

    pub fn last_overall_percent(&self) -> f64 {
        self.last_overall_percent
    }

    /// Emits a `JobEventPayload` subject to monotonic percent clamping, terminal immutability,
    /// and `<= 4/s` coalescing (`force = true` for stage/state transitions or heartbeats).
    pub fn emit(&mut self, mut event: JobEventPayload, force: bool) -> bool {
        if self.terminal {
            return false;
        }

        let state_changed = self.last_state.as_deref() != Some(event.state.as_str());
        let stage_changed = self.last_stage_id.as_deref() != Some(event.stage.id.as_str());
        let is_terminal_state = matches!(
            event.state.as_str(),
            "succeeded" | "failed" | "cancelled" | "interrupted"
        );

        let now = Instant::now();
        let must_emit = force || state_changed || stage_changed || is_terminal_state;
        if !must_emit {
            if let Some(prev) = self.last_emit_at {
                if now.duration_since(prev) < PROGRESS_COALESCE_INTERVAL {
                    return false;
                }
            }
        }

        // Enforce strictly monotonic overall.percent across emitted events
        let clamped_pct = event.overall.percent.clamp(0.0, 100.0);
        if clamped_pct < self.last_overall_percent {
            event.overall.percent = self.last_overall_percent;
        } else {
            event.overall.percent = clamped_pct;
            self.last_overall_percent = clamped_pct;
        }

        if event.ts <= 0.0 {
            event.ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as f64;
        }

        self.last_emit_at = Some(now);
        self.last_state = Some(event.state.clone());
        self.last_stage_id = Some(event.stage.id.clone());

        if let Ok(json) = serde_json::to_string(&event) {
            let repo = JobRepo::new(&self.store);
            match event.state.as_str() {
                "succeeded" => {
                    let _ = repo.mark_succeeded(&self.job_id, &event.stage.id, &json);
                }
                "failed" => {
                    let code = event
                        .error
                        .as_ref()
                        .map(|e| e.code.as_str())
                        .unwrap_or("E_UNKNOWN");
                    let detail = event
                        .detail
                        .as_ref()
                        .and_then(|d| d.get("errorDetail"))
                        .map(String::as_str)
                        .unwrap_or("Job failed");
                    let _ =
                        repo.mark_failed(&self.job_id, &event.stage.id, code, detail, Some(&json));
                }
                "cancelled" => {
                    let _ = repo.mark_cancelled(&self.job_id, &event.stage.id, Some(&json));
                }
                "running" => {
                    let _ = repo.update_stage_and_progress(&self.job_id, &event.stage.id, &json);
                }
                _ => {}
            }
        }

        if is_terminal_state {
            self.terminal = true;
        }

        (self.sink)(event);
        true
    }

    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn kind(&self) -> &str {
        &self.kind
    }
}

#[derive(Debug, Clone)]
struct ActiveJobEntry {
    job_id: String,
    kind: String,
    idempotency_key: String,
    cancel_token: CancellationToken,
}

/// Outcome when asking `JobCoordinator` to enqueue/start a `generate` job.
#[derive(Debug, Clone)]
pub enum StartJobDisposition {
    /// Fresh job created (`queued` -> caller spawns background execution with `token`).
    Started {
        job: JobRecord,
        token: CancellationToken,
    },
    /// Matching active (`queued`/`running`) job or valid already-`succeeded` job returned idempotently.
    ReusedExisting { job: JobRecord },
}

#[derive(Default)]
struct CoordinatorInner {
    active_generate: Option<ActiveJobEntry>,
}

/// Thread-safe coordinator guaranteeing single-generate concurrency, idempotency, cancellation,
/// and crash reconciliation at startup.
#[derive(Clone, Default)]
pub struct JobCoordinator {
    inner: Arc<Mutex<CoordinatorInner>>,
}

impl JobCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reconciles crashed `running`/`queued` jobs -> `interrupted` at startup and removes
    /// orphaned `cache/tmp/<job_id>` directories (including unreferenced leftovers).
    pub fn reconcile_startup(ws_root: &Path, store: &LocalStore) -> JobReconciliationReport {
        let mut report = JobReconciliationReport::default();
        if let Ok(stale_jobs) = JobRepo::new(store).reconcile_stale_running_jobs() {
            for j in stale_jobs {
                cleanup_job_temp_dir(ws_root, &j.id);
                report.interrupted_job_ids.push(j.id);
            }
        }

        // Also sweep any leftover subdirs inside cache/tmp/ (e.g. from pre-SQLite-commit crash).
        let tmp_root = ws_root.join("cache").join("tmp");
        if let Ok(entries) = std::fs::read_dir(&tmp_root) {
            for entry in entries.flatten() {
                let path = entry.path();
                let removed = if path.is_dir() {
                    std::fs::remove_dir_all(&path).is_ok()
                } else if path.is_file() {
                    std::fs::remove_file(&path).is_ok()
                } else {
                    false
                };
                if removed {
                    report.cleaned_temp_dirs += 1;
                }
            }
        }

        report
    }

    /// Attempts to start a `generate` job for `(project_id, idempotency_key)`.
    ///
    /// Rules:
    /// - Same active `idempotency_key` -> returns `ReusedExisting` (never duplicated).
    /// - Different active `generate` job running -> returns `JobError::Busy` ("one generate at a time").
    /// - Prior `succeeded` job with same `idempotency_key` and valid `already_downloaded` flag -> `ReusedExisting`.
    pub fn begin_generate(
        &self,
        store: &LocalStore,
        project_id: &str,
        idempotency_key: &str,
        already_downloaded_and_cached: bool,
    ) -> Result<StartJobDisposition, JobError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| StoreError::Internal("JobCoordinator mutex poisoned".to_string()))?;
        let repo = JobRepo::new(store);

        if let Some(active) = guard.active_generate.as_ref() {
            if active.idempotency_key == idempotency_key {
                if let Some(existing) = repo.get(&active.job_id)? {
                    return Ok(StartJobDisposition::ReusedExisting { job: existing });
                }
            }
            return Err(JobError::Busy {
                active_job_id: active.job_id.clone(),
            });
        }

        // Also check DB in case an active generate row exists without in-memory registration
        if let Some(db_active) = repo.find_active_by_kind(JOB_KIND_GENERATE)? {
            if db_active.idempotency_key.as_deref() == Some(idempotency_key) {
                return Ok(StartJobDisposition::ReusedExisting { job: db_active });
            }
            return Err(JobError::Busy {
                active_job_id: db_active.id,
            });
        }

        if already_downloaded_and_cached {
            if let Some(latest) = repo.find_latest_by_idempotency(idempotency_key)? {
                if latest.status == "succeeded" {
                    return Ok(StartJobDisposition::ReusedExisting { job: latest });
                }
            }
        }

        let job = repo.create_queued(
            project_id,
            JOB_KIND_GENERATE,
            GenerateStageId::Probe.as_str(),
            Some(idempotency_key),
        )?;
        let token = CancellationToken::new();
        guard.active_generate = Some(ActiveJobEntry {
            job_id: job.id.clone(),
            kind: JOB_KIND_GENERATE.to_string(),
            idempotency_key: idempotency_key.to_string(),
            cancel_token: token.clone(),
        });

        Ok(StartJobDisposition::Started { job, token })
    }

    /// Prepares an existing `failed` / `cancelled` / `interrupted` generate job for retry.
    pub fn retry_generate(
        &self,
        store: &LocalStore,
        job_id: &str,
    ) -> Result<(JobRecord, CancellationToken), JobError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| StoreError::Internal("JobCoordinator mutex poisoned".to_string()))?;

        if let Some(active) = guard.active_generate.as_ref() {
            return Err(JobError::Busy {
                active_job_id: active.job_id.clone(),
            });
        }

        let repo = JobRepo::new(store);
        let existing = repo
            .get(job_id)?
            .ok_or_else(|| JobError::NotFound(job_id.to_string()))?;

        if existing.kind != JOB_KIND_GENERATE {
            return Err(JobError::NotRetryable(format!(
                "Unsupported job kind '{}'",
                existing.kind
            )));
        }
        if !matches!(
            existing.status.as_str(),
            "failed" | "cancelled" | "interrupted"
        ) {
            return Err(JobError::NotRetryable(existing.status));
        }

        let retried = repo.prepare_retry(job_id, GenerateStageId::Probe.as_str())?;
        let token = CancellationToken::new();
        guard.active_generate = Some(ActiveJobEntry {
            job_id: retried.id.clone(),
            kind: retried.kind.clone(),
            idempotency_key: retried.idempotency_key.clone().unwrap_or_default(),
            cancel_token: token.clone(),
        });

        Ok((retried, token))
    }

    /// Triggers cancellation on an active job's `CancellationToken` (and marks queued jobs cancelled).
    pub fn cancel_job(
        &self,
        ws_root: &Path,
        store: &LocalStore,
        job_id: &str,
    ) -> Result<bool, JobError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| StoreError::Internal("JobCoordinator mutex poisoned".to_string()))?;

        if let Some(active) = guard.active_generate.as_ref() {
            if active.job_id == job_id {
                active.cancel_token.cancel();
                let _ = JobRepo::new(store).mark_cancelled(job_id, "cancelled", None);
                cleanup_job_temp_dir(ws_root, job_id);
                guard.active_generate = None;
                return Ok(true);
            }
        }

        let repo = JobRepo::new(store);
        if let Some(row) = repo.get(job_id)? {
            if matches!(row.status.as_str(), "queued" | "running") {
                let stage = row.stage.as_deref().unwrap_or("probe");
                repo.mark_cancelled(job_id, stage, None)?;
                cleanup_job_temp_dir(ws_root, job_id);
                return Ok(true);
            }
            return Ok(false);
        }

        Err(JobError::NotFound(job_id.to_string()))
    }

    /// Releases the in-memory active slot when a background job finishes (idempotent).
    pub fn finish_active(&self, job_id: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            if let Some(active) = guard.active_generate.as_ref() {
                if active.job_id == job_id {
                    let _ = active.kind.as_str();
                    guard.active_generate = None;
                }
            }
        }
    }

    pub fn active_job_id(&self) -> Option<String> {
        self.inner
            .lock()
            .ok()
            .and_then(|g| g.active_generate.as_ref().map(|a| a.job_id.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn sample_event(job_id: &str, state: &str, stage: &str, percent: f64) -> JobEventPayload {
        JobEventPayload {
            job_id: job_id.to_string(),
            project_id: "proj-1".to_string(),
            kind: JOB_KIND_GENERATE.to_string(),
            state: state.to_string(),
            stage: JobEventStage {
                id: stage.to_string(),
                index: 1,
                count: 3,
                label_key: "job.stage.download".to_string(),
                label_args: None,
            },
            stage_progress: JobEventStageProgress {
                done: 10.0,
                total: Some(60.0),
                unit: "media_seconds".to_string(),
            },
            overall: JobEventOverall { percent },
            rate: None,
            eta: None,
            detail: None,
            error: None,
            ts: 1_700_000_000_000.0,
        }
    }

    #[test]
    fn test_job_event_tracker_monotonic_coalesced_and_terminal_final() {
        let dir = TempDir::new().unwrap();
        let store = LocalStore::open(&dir.path().join("jobs.db")).unwrap();
        let repo = JobRepo::new(&store);
        let job = repo
            .create_queued("proj-1", JOB_KIND_GENERATE, "probe", Some("k1"))
            .unwrap();
        repo.mark_running(&job.id, "probe").unwrap();

        let emitted = Arc::new(Mutex::new(Vec::<JobEventPayload>::new()));
        let emitted_clone = emitted.clone();
        let mut tracker = JobEventTracker::new(
            store.clone(),
            job.id.clone(),
            "proj-1",
            JOB_KIND_GENERATE,
            Arc::new(move |ev| {
                emitted_clone.lock().unwrap().push(ev);
            }),
        );

        // First running/download event emits immediately (stage transition)
        assert!(tracker.emit(sample_event(&job.id, "running", "download", 30.0), false));
        // Rapid duplicate within 250 ms without transition is coalesced
        assert!(!tracker.emit(sample_event(&job.id, "running", "download", 32.0), false));
        // Forced update with regression in percent clamps monotonically >= 30.0
        assert!(tracker.emit(sample_event(&job.id, "running", "download", 18.0), true));
        assert_eq!(tracker.last_overall_percent(), 30.0);

        // Terminal succeeded event is final; subsequent emits are rejected
        assert!(tracker.emit(sample_event(&job.id, "succeeded", "build", 100.0), true));
        assert!(tracker.is_terminal());
        assert!(!tracker.emit(sample_event(&job.id, "failed", "build", 10.0), true));

        let rows = emitted.lock().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].overall.percent, 30.0);
        assert_eq!(rows[1].overall.percent, 30.0);
        assert_eq!(rows[2].state, "succeeded");
        assert_eq!(rows[2].overall.percent, 100.0);

        let persisted = repo.get(&job.id).unwrap().unwrap();
        assert_eq!(persisted.status, "succeeded");
    }

    #[test]
    fn test_coordinator_one_at_a_time_and_idempotency_and_retry() {
        let dir = TempDir::new().unwrap();
        let ws = security::ensure_workspace_with_root(dir.path()).unwrap();
        let store = LocalStore::open(&ws.join("clipper.db")).unwrap();
        let coord = JobCoordinator::new();

        // 1. Start job A
        let key_a =
            compute_generate_idempotency_key("p1", "youtube", "WufkeDaJisQ", 0, 60_000, "f1", 5);
        let first = match coord.begin_generate(&store, "p1", &key_a, false).unwrap() {
            StartJobDisposition::Started { job, .. } => job,
            _ => panic!("Expected Started"),
        };

        // 2. Duplicate same idempotency_key reuses active job A
        match coord.begin_generate(&store, "p1", &key_a, false).unwrap() {
            StartJobDisposition::ReusedExisting { job } => assert_eq!(job.id, first.id),
            _ => panic!("Expected ReusedExisting"),
        }

        // 3. Different project/range while A is active is rejected with Busy
        let key_b =
            compute_generate_idempotency_key("p2", "youtube", "3snfJrlSyqo", 0, 60_000, "f1", 5);
        let busy_err = coord
            .begin_generate(&store, "p2", &key_b, false)
            .unwrap_err();
        assert!(matches!(busy_err, JobError::Busy { .. }));

        // 4. Fail A, release slot, and retry increments attempt cleanly
        JobRepo::new(&store)
            .mark_running(&first.id, "download")
            .unwrap();
        JobRepo::new(&store)
            .mark_failed(&first.id, "download", "E_NET_OFFLINE", "offline", None)
            .unwrap();
        coord.finish_active(&first.id);

        let (retried, _tok) = coord.retry_generate(&store, &first.id).unwrap();
        assert_eq!(retried.attempt, 2);
        assert_eq!(retried.status, "queued");

        // 5. Cancel clears active slot + temp dir
        let tmp_dir = job_temp_dir(&ws, &first.id).unwrap();
        fs::create_dir_all(&tmp_dir).unwrap();
        fs::write(tmp_dir.join("part.mp4"), b"partial").unwrap();
        assert!(coord.cancel_job(&ws, &store, &first.id).unwrap());
        assert!(!tmp_dir.exists());
        assert!(coord.active_job_id().is_none());
        let after_cancel = JobRepo::new(&store).get(&first.id).unwrap().unwrap();
        assert_eq!(after_cancel.status, "cancelled");
    }

    #[test]
    fn test_startup_reconciles_running_jobs_to_interrupted_and_cleans_temp_dirs() {
        let dir = TempDir::new().unwrap();
        let ws = security::ensure_workspace_with_root(dir.path()).unwrap();
        let store = LocalStore::open(&ws.join("clipper.db")).unwrap();
        let repo = JobRepo::new(&store);

        let j1 = repo
            .create_queued("p1", JOB_KIND_GENERATE, "download", Some("k1"))
            .unwrap();
        repo.mark_running(&j1.id, "download").unwrap();

        let leftover_dir = job_temp_dir(&ws, &j1.id).unwrap();
        fs::create_dir_all(&leftover_dir).unwrap();
        fs::write(leftover_dir.join("part.mp4"), b"orphaned bytes").unwrap();

        let orphan_other = ws.join("cache").join("tmp").join("orphan-dir");
        fs::create_dir_all(&orphan_other).unwrap();

        let report = JobCoordinator::reconcile_startup(&ws, &store);
        assert_eq!(report.interrupted_job_ids, vec![j1.id.clone()]);
        assert!(!leftover_dir.exists());
        assert!(!orphan_other.exists());

        let reconciled = repo.get(&j1.id).unwrap().unwrap();
        assert_eq!(reconciled.status, "interrupted");
        assert_eq!(reconciled.stage.as_deref(), Some("download"));
        assert!(reconciled.finished_at.is_some());
    }
}
