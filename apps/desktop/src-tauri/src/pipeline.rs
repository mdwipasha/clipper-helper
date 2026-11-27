//! End-to-end persistent `generate` pipeline orchestrating:
//! `probe` -> `download` (handling/cache/ffprobe) -> conditional `prepare_preview` (`h264_mf`) -> `build` (`CompositionV1`).
//!
//! Enforces monotonic `JobEventPayload` emission, `JobCoordinator` active-slot cleanup on drop,
//! and redacted `error_events` persistence (`PROGRESS_AND_ETA §2–§6`, `ARCHITECTURE §5.1`, `DATA_MODEL §7/§9`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::composition::{
    build_initial_composition, canonicalize_and_hash, write_project_composition_file,
};
use crate::download::{
    record_stage_error, run_download_stage_with_env, DownloadError, ProbeHandleCache,
};
use crate::eta::StallStatus;
use crate::installer::InUseRegistry;
use crate::jobs::{
    cleanup_job_temp_dir, compute_generate_overall_percent, stage_index_and_count, GenerateStageId,
    JobCoordinator, JobEventError, JobEventEta, JobEventOverall, JobEventPayload, JobEventRate,
    JobEventStage, JobEventStageProgress, JobEventTracker,
};
use crate::preview_proxy::{run_prepare_preview_stage, PreviewProxyError};
use crate::resolver::ComponentResolver;
use crate::security;
use crate::store::{
    ArtifactRepo, JobRecord, JobRepo, LocalStore, NewErrorEvent, ProjectRecord, ProjectRepo,
};

#[derive(Error, Debug)]
pub enum PipelineError {
    #[error("Pipeline cancelled")]
    Cancelled,
    #[error("[{code}] {detail}")]
    Failed { code: String, detail: String },
}

pub struct GenerateRunParams {
    pub ws_root: PathBuf,
    pub store: LocalStore,
    pub in_use: InUseRegistry,
    pub coordinator: JobCoordinator,
    pub probe_cache: ProbeHandleCache,
    pub job: JobRecord,
    pub project: ProjectRecord,
    pub selected_start_ms: u32,
    pub selected_end_ms: u32,
    pub device_profile: String,
    pub max_range_seconds: u32,
    pub handle_seconds: u32,
    pub extra_sidecar_env: Vec<(String, String)>,
    pub free_space_override: Option<u64>,
    pub cancel_token: CancellationToken,
    pub event_sink: Arc<dyn Fn(JobEventPayload) + Send + Sync>,
}

struct ActivePipelineGuard {
    ws_root: PathBuf,
    coordinator: JobCoordinator,
    job_id: String,
}

impl Drop for ActivePipelineGuard {
    fn drop(&mut self) {
        self.coordinator.finish_active(&self.job_id);
        cleanup_job_temp_dir(&self.ws_root, &self.job_id);
    }
}

fn now_epoch_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as f64
}

fn stage_meta(stage: GenerateStageId, needs_proxy: bool) -> JobEventStage {
    let (index, count) = stage_index_and_count(stage, needs_proxy);
    JobEventStage {
        id: stage.as_str().to_string(),
        index,
        count,
        label_key: stage.label_key().to_string(),
        label_args: None,
    }
}

fn eta_confidence_str(conf: crate::eta::EtaConfidence) -> String {
    match conf {
        crate::eta::EtaConfidence::Low => "low".to_string(),
        crate::eta::EtaConfidence::Medium => "medium".to_string(),
        crate::eta::EtaConfidence::High => "high".to_string(),
    }
}

fn stall_status_str(stall: StallStatus) -> &'static str {
    match stall {
        StallStatus::Normal => "normal",
        StallStatus::SlowWarning => "slow_warning",
        StallStatus::StalledRetry => "stalled_retry",
    }
}

fn action_keys_for_error(code: &str, retryable: bool) -> Vec<String> {
    let mut keys = Vec::new();
    match code {
        "E_COMPONENT_MISSING" => keys.push("repair".to_string()),
        "E_NET_OFFLINE" | "E_NET_TIMEOUT" | "E_YT_RATE_LIMITED" => {
            keys.push("check_connection".to_string());
        }
        "E_DISK_LOW" | "E_DISK_FULL" => keys.push("clear_cache".to_string()),
        _ => {}
    }
    if retryable && !keys.iter().any(|k| k == "retry") {
        keys.push("retry".to_string());
    }
    keys
}

fn emit_cancelled(
    tracker: &Arc<Mutex<JobEventTracker>>,
    job_id: &str,
    project_id: &str,
    kind: &str,
    stage: GenerateStageId,
    needs_proxy: bool,
) {
    if let Ok(mut guard) = tracker.lock() {
        let pct = guard.last_overall_percent();
        guard.emit(
            JobEventPayload {
                job_id: job_id.to_string(),
                project_id: project_id.to_string(),
                kind: kind.to_string(),
                state: "cancelled".to_string(),
                stage: stage_meta(stage, needs_proxy),
                stage_progress: JobEventStageProgress {
                    done: 0.0,
                    total: None,
                    unit: stage.unit().to_string(),
                },
                overall: JobEventOverall { percent: pct },
                rate: None,
                eta: None,
                detail: None,
                error: Some(JobEventError {
                    code: "E_JOB_CANCELLED".to_string(),
                    message_key: "job.cancelledNotice".to_string(),
                    retryable: true,
                    action_keys: vec!["retry".to_string()],
                }),
                ts: now_epoch_ms(),
            },
            true,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_failed(
    tracker: &Arc<Mutex<JobEventTracker>>,
    job_id: &str,
    project_id: &str,
    kind: &str,
    stage: GenerateStageId,
    needs_proxy: bool,
    code: &str,
    message_key: &str,
    redacted_detail: String,
    retryable: bool,
) {
    if let Ok(mut guard) = tracker.lock() {
        let pct = guard.last_overall_percent();
        let mut detail = HashMap::new();
        detail.insert("errorDetail".to_string(), redacted_detail);
        guard.emit(
            JobEventPayload {
                job_id: job_id.to_string(),
                project_id: project_id.to_string(),
                kind: kind.to_string(),
                state: "failed".to_string(),
                stage: stage_meta(stage, needs_proxy),
                stage_progress: JobEventStageProgress {
                    done: 0.0,
                    total: None,
                    unit: stage.unit().to_string(),
                },
                overall: JobEventOverall { percent: pct },
                rate: None,
                eta: None,
                detail: Some(detail),
                error: Some(JobEventError {
                    code: code.to_string(),
                    message_key: message_key.to_string(),
                    retryable,
                    action_keys: action_keys_for_error(code, retryable),
                }),
                ts: now_epoch_ms(),
            },
            true,
        );
    }
}

/// Runs the full M1.4 `generate` pipeline (`probe` -> `download` -> conditional `prepare_preview` -> `build`),
/// persisting progress, artifacts, composition, and terminal state cleanly.
pub async fn execute_generate_job(params: GenerateRunParams) -> Result<(), PipelineError> {
    let GenerateRunParams {
        ws_root,
        store,
        in_use,
        coordinator,
        probe_cache,
        job,
        project,
        selected_start_ms,
        selected_end_ms,
        device_profile,
        max_range_seconds,
        handle_seconds,
        extra_sidecar_env,
        free_space_override,
        cancel_token,
        event_sink,
    } = params;

    let job_id = job.id.clone();
    let project_id = project.id.clone();
    let kind = job.kind.clone();
    let app_version = env!("CARGO_PKG_VERSION").to_string();

    let _guard = ActivePipelineGuard {
        ws_root: ws_root.clone(),
        coordinator,
        job_id: job_id.clone(),
    };

    let tracker = Arc::new(Mutex::new(JobEventTracker::new(
        store.clone(),
        job_id.clone(),
        project_id.clone(),
        kind.clone(),
        event_sink,
    )));

    if let Err(e) = JobRepo::new(&store).mark_running(&job_id, GenerateStageId::Probe.as_str()) {
        let msg = security::redact_sensitive_text(&e.to_string());
        emit_failed(
            &tracker,
            &job_id,
            &project_id,
            &kind,
            GenerateStageId::Probe,
            false,
            "E_UNKNOWN",
            "err.E_UNKNOWN.body",
            msg.clone(),
            true,
        );
        return Err(PipelineError::Failed {
            code: "E_UNKNOWN".to_string(),
            detail: msg,
        });
    }

    if cancel_token.is_cancelled() {
        emit_cancelled(
            &tracker,
            &job_id,
            &project_id,
            &kind,
            GenerateStageId::Probe,
            false,
        );
        return Err(PipelineError::Cancelled);
    }

    // Stage 1: Probe validation / cached ProbeHandle check (0% -> 5%)
    if let Ok(mut guard) = tracker.lock() {
        guard.emit(
            JobEventPayload {
                job_id: job_id.clone(),
                project_id: project_id.clone(),
                kind: kind.clone(),
                state: "running".to_string(),
                stage: stage_meta(GenerateStageId::Probe, false),
                stage_progress: JobEventStageProgress {
                    done: 0.0,
                    total: Some(1.0),
                    unit: "steps".to_string(),
                },
                overall: JobEventOverall { percent: 0.0 },
                rate: None,
                eta: None,
                detail: None,
                error: None,
                ts: now_epoch_ms(),
            },
            true,
        );
    }

    let initial_probe = probe_cache.take(&project.source_provider, &project.source_video_id);
    if let Ok(mut guard) = tracker.lock() {
        let mut detail = HashMap::new();
        detail.insert(
            "probeCached".to_string(),
            if initial_probe.is_some() {
                "true"
            } else {
                "false"
            }
            .to_string(),
        );
        guard.emit(
            JobEventPayload {
                job_id: job_id.clone(),
                project_id: project_id.clone(),
                kind: kind.clone(),
                state: "running".to_string(),
                stage: stage_meta(GenerateStageId::Probe, false),
                stage_progress: JobEventStageProgress {
                    done: 1.0,
                    total: Some(1.0),
                    unit: "steps".to_string(),
                },
                overall: JobEventOverall {
                    percent: compute_generate_overall_percent(GenerateStageId::Probe, 1.0, false),
                },
                rate: None,
                eta: None,
                detail: Some(detail),
                error: None,
                ts: now_epoch_ms(),
            },
            true,
        );
    }

    // Stage 2: Download (5% -> 90%, leaving room for optional prepare_preview)
    let tracker_dl = tracker.clone();
    let job_id_dl = job_id.clone();
    let project_id_dl = project_id.clone();
    let kind_dl = kind.clone();
    let prev_stall = Arc::new(Mutex::new(StallStatus::Normal));

    if let Ok(mut guard) = tracker.lock() {
        guard.emit(
            JobEventPayload {
                job_id: job_id.clone(),
                project_id: project_id.clone(),
                kind: kind.clone(),
                state: "running".to_string(),
                stage: stage_meta(GenerateStageId::Download, false),
                stage_progress: JobEventStageProgress {
                    done: 0.0,
                    total: None,
                    unit: "media_seconds".to_string(),
                },
                overall: JobEventOverall {
                    percent: compute_generate_overall_percent(GenerateStageId::Download, 0.0, true),
                },
                rate: None,
                eta: None,
                detail: Some(HashMap::from([("startup".to_string(), "true".to_string())])),
                error: None,
                ts: now_epoch_ms(),
            },
            true,
        );
    }

    let resolver = ComponentResolver::new(ws_root.clone(), store.clone(), in_use);
    let dl_res = run_download_stage_with_env(
        &ws_root,
        &store,
        &resolver,
        &device_profile,
        &job_id,
        &project,
        selected_start_ms,
        selected_end_ms,
        max_range_seconds,
        handle_seconds,
        initial_probe,
        free_space_override,
        &extra_sidecar_env,
        &cancel_token,
        move |prog| {
            let snap = prog.snapshot;
            let stall_changed = prev_stall
                .lock()
                .map(|mut prev| {
                    let changed = *prev != snap.stall_status;
                    *prev = snap.stall_status;
                    changed
                })
                .unwrap_or(false);

            let mut detail = HashMap::new();
            detail.insert(
                "startup".to_string(),
                if snap.is_startup_indeterminate {
                    "true"
                } else {
                    "false"
                }
                .to_string(),
            );
            detail.insert(
                "stallStatus".to_string(),
                stall_status_str(snap.stall_status).to_string(),
            );
            detail.insert(
                "stallElapsedMs".to_string(),
                snap.stall_elapsed_ms.to_string(),
            );
            if let Some(sr) = prog.speed_ratio {
                detail.insert("speedRatio".to_string(), format!("{:.2}", sr));
            }
            if let Some(sz) = prog.size_bytes {
                detail.insert("sizeBytes".to_string(), sz.to_string());
            }

            let overall_pct = if snap.is_startup_indeterminate {
                5.0
            } else {
                compute_generate_overall_percent(GenerateStageId::Download, snap.fraction, true)
            };

            let payload = JobEventPayload {
                job_id: job_id_dl.clone(),
                project_id: project_id_dl.clone(),
                kind: kind_dl.clone(),
                state: "running".to_string(),
                stage: stage_meta(GenerateStageId::Download, false),
                stage_progress: JobEventStageProgress {
                    done: if snap.is_startup_indeterminate {
                        0.0
                    } else {
                        snap.done_units
                    },
                    total: if snap.is_startup_indeterminate {
                        None
                    } else {
                        Some(snap.total_units)
                    },
                    unit: "media_seconds".to_string(),
                },
                overall: JobEventOverall {
                    percent: overall_pct,
                },
                rate: snap.smoothed_rate_units_per_sec.map(|v| JobEventRate {
                    value: v,
                    unit: "media_seconds_per_sec".to_string(),
                    smoothed: true,
                }),
                eta: snap.eta.map(|e| JobEventEta {
                    seconds: e.seconds,
                    low_seconds: e.low_seconds,
                    high_seconds: e.high_seconds,
                    confidence: eta_confidence_str(e.confidence),
                }),
                detail: Some(detail),
                error: None,
                ts: now_epoch_ms(),
            };
            if let Ok(mut guard) = tracker_dl.lock() {
                guard.emit(payload, stall_changed);
            }
        },
    )
    .await;

    let dl_outcome = match dl_res {
        Ok(out) => out,
        Err(DownloadError::Cancelled) => {
            emit_cancelled(
                &tracker,
                &job_id,
                &project_id,
                &kind,
                GenerateStageId::Download,
                false,
            );
            return Err(PipelineError::Cancelled);
        }
        Err(err) => {
            record_stage_error(&store, &job_id, "download", &err, &app_version);
            let detail = security::redact_sensitive_text(&err.to_string());
            let code = err.error_code().to_string();
            let message_key = err.message_key();
            let retryable = err.retryable();
            emit_failed(
                &tracker,
                &job_id,
                &project_id,
                &kind,
                GenerateStageId::Download,
                false,
                &code,
                &message_key,
                detail.clone(),
                retryable,
            );
            return Err(PipelineError::Failed { code, detail });
        }
    };

    if let Err(e) = ArtifactRepo::new(&store).upsert(
        Some(&project_id),
        "source",
        &dl_outcome.cache_key,
        &dl_outcome.rel_path,
        dl_outcome.media.size_bytes,
        None,
    ) {
        tracing::warn!("Failed to record source artifact metadata: {}", e);
    }

    let needs_proxy = !dl_outcome.media.is_webview_native;
    let mut proxy_rel_path: Option<String> = None;
    let mut proxy_cache_hit = false;

    if needs_proxy {
        let tracker_px = tracker.clone();
        let job_id_px = job_id.clone();
        let project_id_px = project_id.clone();
        let kind_px = kind.clone();

        if let Ok(mut guard) = tracker.lock() {
            guard.emit(
                JobEventPayload {
                    job_id: job_id.clone(),
                    project_id: project_id.clone(),
                    kind: kind.clone(),
                    state: "running".to_string(),
                    stage: stage_meta(GenerateStageId::PreparePreview, true),
                    stage_progress: JobEventStageProgress {
                        done: 0.0,
                        total: Some(dl_outcome.window.requested_duration_secs()),
                        unit: "media_seconds".to_string(),
                    },
                    overall: JobEventOverall {
                        percent: compute_generate_overall_percent(
                            GenerateStageId::PreparePreview,
                            0.0,
                            true,
                        ),
                    },
                    rate: None,
                    eta: None,
                    detail: None,
                    error: None,
                    ts: now_epoch_ms(),
                },
                true,
            );
        }

        let px_res = run_prepare_preview_stage(
            &ws_root,
            &store,
            &resolver,
            &device_profile,
            &job_id,
            &dl_outcome.cache_key,
            &dl_outcome.abs_path,
            dl_outcome.window.requested_duration_secs(),
            &extra_sidecar_env,
            &cancel_token,
            move |prog| {
                let snap = prog.snapshot;
                let mut detail = HashMap::new();
                detail.insert(
                    "stallStatus".to_string(),
                    stall_status_str(snap.stall_status).to_string(),
                );
                if let Some(fps) = prog.fps {
                    detail.insert("fps".to_string(), format!("{:.1}", fps));
                }
                let payload = JobEventPayload {
                    job_id: job_id_px.clone(),
                    project_id: project_id_px.clone(),
                    kind: kind_px.clone(),
                    state: "running".to_string(),
                    stage: stage_meta(GenerateStageId::PreparePreview, true),
                    stage_progress: JobEventStageProgress {
                        done: snap.done_units,
                        total: Some(snap.total_units),
                        unit: "media_seconds".to_string(),
                    },
                    overall: JobEventOverall {
                        percent: compute_generate_overall_percent(
                            GenerateStageId::PreparePreview,
                            snap.fraction,
                            true,
                        ),
                    },
                    rate: snap.smoothed_rate_units_per_sec.map(|v| JobEventRate {
                        value: v,
                        unit: "media_seconds_per_sec".to_string(),
                        smoothed: true,
                    }),
                    eta: snap.eta.map(|e| JobEventEta {
                        seconds: e.seconds,
                        low_seconds: e.low_seconds,
                        high_seconds: e.high_seconds,
                        confidence: eta_confidence_str(e.confidence),
                    }),
                    detail: Some(detail),
                    error: None,
                    ts: now_epoch_ms(),
                };
                if let Ok(mut guard) = tracker_px.lock() {
                    guard.emit(payload, false);
                }
            },
        )
        .await;

        match px_res {
            Ok(px) => {
                proxy_cache_hit = px.cache_hit;
                if let Err(e) = ArtifactRepo::new(&store).upsert(
                    Some(&project_id),
                    "preview_proxy",
                    &px.cache_key,
                    &px.rel_path,
                    px.media.size_bytes,
                    None,
                ) {
                    tracing::warn!("Failed to record proxy artifact metadata: {}", e);
                }
                proxy_rel_path = Some(px.rel_path);
            }
            Err(PreviewProxyError::Cancelled) => {
                emit_cancelled(
                    &tracker,
                    &job_id,
                    &project_id,
                    &kind,
                    GenerateStageId::PreparePreview,
                    true,
                );
                return Err(PipelineError::Cancelled);
            }
            Err(err) => {
                let code = err.error_code().to_string();
                let message_key = err.message_key();
                let detail = security::redact_sensitive_text(&err.to_string());
                let _ = store.record_error_event(NewErrorEvent {
                    code: code.clone(),
                    stage: "prepare_preview".to_string(),
                    job_id: Some(job_id.clone()),
                    message_key: message_key.clone(),
                    detail: detail.clone(),
                    stderr_tail: String::new(),
                    app_version: app_version.clone(),
                    component_versions_json: "{}".to_string(),
                });
                emit_failed(
                    &tracker,
                    &job_id,
                    &project_id,
                    &kind,
                    GenerateStageId::PreparePreview,
                    true,
                    &code,
                    &message_key,
                    detail.clone(),
                    true,
                );
                return Err(PipelineError::Failed { code, detail });
            }
        }
    }

    if cancel_token.is_cancelled() {
        emit_cancelled(
            &tracker,
            &job_id,
            &project_id,
            &kind,
            GenerateStageId::Build,
            needs_proxy,
        );
        return Err(PipelineError::Cancelled);
    }

    // Stage 4: Build initial Composition JSON v1 & persist atomically
    if let Ok(mut guard) = tracker.lock() {
        guard.emit(
            JobEventPayload {
                job_id: job_id.clone(),
                project_id: project_id.clone(),
                kind: kind.clone(),
                state: "running".to_string(),
                stage: stage_meta(GenerateStageId::Build, needs_proxy),
                stage_progress: JobEventStageProgress {
                    done: 0.0,
                    total: Some(1.0),
                    unit: "steps".to_string(),
                },
                overall: JobEventOverall {
                    percent: compute_generate_overall_percent(
                        GenerateStageId::Build,
                        0.0,
                        needs_proxy,
                    ),
                },
                rate: None,
                eta: None,
                detail: None,
                error: None,
                ts: now_epoch_ms(),
            },
            true,
        );
    }

    let build_res = (|| -> Result<String, String> {
        let comp = build_initial_composition(&project, &dl_outcome, proxy_rel_path.clone())
            .map_err(|e| e.to_string())?;
        let (canonical_json, hash) = canonicalize_and_hash(&comp).map_err(|e| e.to_string())?;
        write_project_composition_file(&ws_root, &project_id, &canonical_json)
            .map_err(|e| e.to_string())?;
        ProjectRepo::new(&store)
            .save_composition(&project_id, &canonical_json, &hash, "downloaded")
            .map_err(|e| e.to_string())?;
        Ok(hash)
    })();

    match build_res {
        Ok(comp_hash) => {
            let mut detail = HashMap::new();
            detail.insert("cacheHit".to_string(), dl_outcome.cache_hit.to_string());
            detail.insert("proxyCacheHit".to_string(), proxy_cache_hit.to_string());
            detail.insert("needsProxy".to_string(), needs_proxy.to_string());
            detail.insert("sourceRelPath".to_string(), dl_outcome.rel_path.clone());
            if let Some(ref px_rel) = proxy_rel_path {
                detail.insert("previewRelPath".to_string(), px_rel.clone());
            }
            detail.insert(
                "sizeBytes".to_string(),
                dl_outcome.media.size_bytes.to_string(),
            );
            detail.insert("width".to_string(), dl_outcome.media.width.to_string());
            detail.insert("height".to_string(), dl_outcome.media.height.to_string());
            detail.insert("fps".to_string(), format!("{:.2}", dl_outcome.media.fps));
            detail.insert(
                "videoCodec".to_string(),
                dl_outcome.media.video_codec.clone(),
            );
            detail.insert(
                "audioCodec".to_string(),
                dl_outcome.media.audio_codec.clone(),
            );
            detail.insert(
                "handleBeforeMs".to_string(),
                dl_outcome.window.handle_before_ms.to_string(),
            );
            detail.insert(
                "handleAfterMs".to_string(),
                dl_outcome.window.handle_after_ms.to_string(),
            );
            detail.insert("compositionHash".to_string(), comp_hash);
            detail.insert(
                "reprobedAfter403".to_string(),
                dl_outcome.reprobed_after_403.to_string(),
            );

            if let Ok(mut guard) = tracker.lock() {
                guard.emit(
                    JobEventPayload {
                        job_id,
                        project_id,
                        kind,
                        state: "succeeded".to_string(),
                        stage: stage_meta(GenerateStageId::Build, needs_proxy),
                        stage_progress: JobEventStageProgress {
                            done: 1.0,
                            total: Some(1.0),
                            unit: "steps".to_string(),
                        },
                        overall: JobEventOverall { percent: 100.0 },
                        rate: None,
                        eta: None,
                        detail: Some(detail),
                        error: None,
                        ts: now_epoch_ms(),
                    },
                    true,
                );
            }
            Ok(())
        }
        Err(msg) => {
            let redacted = security::redact_sensitive_text(&msg);
            let _ = store.record_error_event(NewErrorEvent {
                code: "E_UNKNOWN".to_string(),
                stage: "build".to_string(),
                job_id: Some(job_id.clone()),
                message_key: "err.E_UNKNOWN.body".to_string(),
                detail: redacted.clone(),
                stderr_tail: String::new(),
                app_version,
                component_versions_json: "{}".to_string(),
            });
            emit_failed(
                &tracker,
                &job_id,
                &project_id,
                &kind,
                GenerateStageId::Build,
                needs_proxy,
                "E_UNKNOWN",
                "err.E_UNKNOWN.body",
                redacted.clone(),
                true,
            );
            Err(PipelineError::Failed {
                code: "E_UNKNOWN".to_string(),
                detail: redacted,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::tests::setup_fake_components;
    use crate::jobs::{compute_generate_idempotency_key, StartJobDisposition};
    use crate::probe::{PreviewMode, ProbeInfo, YT_FORMAT_SELECTOR};
    use crate::url_intake;
    use tempfile::TempDir;

    fn make_ranged_project(
        store: &LocalStore,
        video_id: &str,
        duration_seconds: u32,
        start_ms: u32,
        end_ms: u32,
    ) -> ProjectRecord {
        let raw_url = format!("https://www.youtube.com/watch?v={}", video_id);
        let parsed = url_intake::parse_youtube_url(&raw_url).unwrap();
        let info = ProbeInfo {
            id: video_id.to_string(),
            title: "Test Clip Title".to_string(),
            channel: "Test Channel".to_string(),
            duration_seconds,
            live_status: Some("not_live".to_string()),
            availability: Some("public".to_string()),
            playable_in_embed: true,
            was_live: false,
            is_live: false,
            estimated_bytes_per_second: Some(1_000_000.0),
            has_avc1_1080p: true,
            preview_mode: PreviewMode::Embed,
        };
        let repo = ProjectRepo::new(store);
        let proj = repo.create_from_probe(&parsed, &info).unwrap();
        repo.update_range(&proj.id, start_ms, end_ms).unwrap()
    }

    #[tokio::test]
    async fn test_execute_generate_job_avc1_skips_proxy_and_saves_composition() {
        let tmp = TempDir::new().unwrap();
        let ws = security::ensure_workspace_with_root(tmp.path()).unwrap();
        let store = LocalStore::open(&ws.join("clipper.db")).unwrap();
        let _ = setup_fake_components(&ws, &store);

        let proj = make_ranged_project(&store, "aqz-KE-bpKQ", 120, 15_000, 75_000);
        let coordinator = JobCoordinator::new();
        let key = compute_generate_idempotency_key(
            &proj.id,
            &proj.source_provider,
            &proj.source_video_id,
            15_000,
            75_000,
            YT_FORMAT_SELECTOR,
            5,
        );
        let (job, token) = match coordinator
            .begin_generate(&store, &proj.id, &key, false)
            .unwrap()
        {
            StartJobDisposition::Started { job, token } => (job, token),
            _ => panic!("expected fresh job"),
        };

        let events = Arc::new(Mutex::new(Vec::<JobEventPayload>::new()));
        let ev_sink = events.clone();

        execute_generate_job(GenerateRunParams {
            ws_root: ws.clone(),
            store: store.clone(),
            in_use: InUseRegistry::new(),
            coordinator: coordinator.clone(),
            probe_cache: ProbeHandleCache::new(),
            job: job.clone(),
            project: proj.clone(),
            selected_start_ms: 15_000,
            selected_end_ms: 75_000,
            device_profile: "test-cpu".to_string(),
            max_range_seconds: 300,
            handle_seconds: 5,
            extra_sidecar_env: vec![
                ("CLIPPER_FAKE_YTDLP_SCENARIO".into(), "success".into()),
                ("CLIPPER_FAKE_FFPROBE_SCENARIO".into(), "avc1".into()),
                ("CLIPPER_FAKE_FFPROBE_DURATION".into(), "70.0".into()),
            ],
            free_space_override: Some(10 * 1024 * 1024 * 1024),
            cancel_token: token,
            event_sink: Arc::new(move |ev| {
                ev_sink.lock().unwrap().push(ev);
            }),
        })
        .await
        .unwrap();

        let emitted = events.lock().unwrap().clone();
        assert!(!emitted.is_empty());
        assert_eq!(emitted.last().unwrap().state, "succeeded");
        assert_eq!(emitted.last().unwrap().overall.percent, 100.0);
        // Never ran prepare_preview
        assert!(!emitted.iter().any(|e| e.stage.id == "prepare_preview"));

        // Monotonic percent guarantee
        for w in emitted.windows(2) {
            assert!(w[1].overall.percent >= w[0].overall.percent);
        }

        let updated = ProjectRepo::new(&store).get(&proj.id).unwrap().unwrap();
        assert_eq!(updated.status, "downloaded");
        assert!(updated.composition_hash.is_some());
        assert!(ws
            .join("projects")
            .join(&proj.id)
            .join("composition.json")
            .is_file());
        assert!(coordinator.active_job_id().is_none());
    }

    #[tokio::test]
    async fn test_execute_generate_job_vp9_runs_prepare_preview_proxy() {
        let tmp = TempDir::new().unwrap();
        let ws = security::ensure_workspace_with_root(tmp.path()).unwrap();
        let store = LocalStore::open(&ws.join("clipper.db")).unwrap();
        let _ = setup_fake_components(&ws, &store);

        let proj = make_ranged_project(&store, "WufkeDaJisQ", 120, 15_000, 75_000);
        let coordinator = JobCoordinator::new();
        let key = compute_generate_idempotency_key(
            &proj.id,
            &proj.source_provider,
            &proj.source_video_id,
            15_000,
            75_000,
            YT_FORMAT_SELECTOR,
            5,
        );
        let (job, token) = match coordinator
            .begin_generate(&store, &proj.id, &key, false)
            .unwrap()
        {
            StartJobDisposition::Started { job, token } => (job, token),
            _ => panic!("expected fresh job"),
        };

        let events = Arc::new(Mutex::new(Vec::<JobEventPayload>::new()));
        let ev_sink = events.clone();

        execute_generate_job(GenerateRunParams {
            ws_root: ws.clone(),
            store: store.clone(),
            in_use: InUseRegistry::new(),
            coordinator: coordinator.clone(),
            probe_cache: ProbeHandleCache::new(),
            job: job.clone(),
            project: proj.clone(),
            selected_start_ms: 15_000,
            selected_end_ms: 75_000,
            device_profile: "test-cpu".to_string(),
            max_range_seconds: 300,
            handle_seconds: 5,
            extra_sidecar_env: vec![
                ("CLIPPER_FAKE_YTDLP_SCENARIO".into(), "success".into()),
                (
                    "CLIPPER_FAKE_FFPROBE_SCENARIO".into(),
                    "vp9_then_avc1".into(),
                ),
                ("CLIPPER_FAKE_FFPROBE_DURATION".into(), "70.0".into()),
                ("CLIPPER_FAKE_FFMPEG_SCENARIO".into(), "success".into()),
            ],
            free_space_override: Some(10 * 1024 * 1024 * 1024),
            cancel_token: token,
            event_sink: Arc::new(move |ev| {
                ev_sink.lock().unwrap().push(ev);
            }),
        })
        .await
        .unwrap();

        let emitted = events.lock().unwrap().clone();
        assert!(emitted.iter().any(|e| e.stage.id == "prepare_preview"));
        let last = emitted.last().unwrap();
        assert_eq!(last.state, "succeeded");
        let detail = last.detail.as_ref().unwrap();
        assert_eq!(detail.get("needsProxy").map(String::as_str), Some("true"));
        assert!(detail.get("previewRelPath").is_some());
    }
}
