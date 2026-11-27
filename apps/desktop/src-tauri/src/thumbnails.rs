//! Background timeline filmstrip thumbnail extractor (M1.5 Part 4).
//!
//! Spawns `ffmpeg` strictly via `SidecarSupervisor` (`-vf fps=1/<interval>,scale=<w>:-2`, `-progress pipe:1`)
//! into `cache/tmp/`, enforces frame count and byte-budget caps, writes `manifest.json` + images
//! atomically to `cache/thumbs/<cache_key>/`, and supports cancellation + cache hits.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use specta::Type;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::eta::{LiveEtaConfig, LiveStageEtaTracker};
use crate::progress_parser::FfmpegPipeProgressParser;
use crate::resolver::{ComponentResolver, ResolverError};
use crate::security;
use crate::sidecars::{SidecarEnv, SidecarError, SidecarSpec, SidecarSupervisor};

pub const THUMBNAIL_INTERVAL_SECS: u32 = 2;
pub const THUMBNAIL_WIDTH_PX: u32 = 160;
pub const MAX_THUMBNAIL_FRAMES: usize = 320;
pub const MAX_THUMBNAIL_TOTAL_BYTES: u64 = 12 * 1024 * 1024;
pub const THUMBNAIL_TIMEOUT_SECS: u64 = 120;

#[derive(Error, Debug)]
pub enum ThumbnailError {
    #[error("Component missing: {0}")]
    ComponentMissing(String),

    #[error("FFmpeg thumbnail generation failed: {0}")]
    Failed(String),

    #[error("Thumbnail output exceeded safety limits: {0}")]
    ExceededLimits(String),

    #[error("Thumbnail generation cancelled")]
    Cancelled,

    #[error("IO error during thumbnail generation: {0}")]
    Io(String),
}

impl ThumbnailError {
    pub fn error_code(&self) -> &'static str {
        match self {
            ThumbnailError::ComponentMissing(_) => "E_COMPONENT_MISSING",
            ThumbnailError::Failed(_) => "E_RENDER_FAILED",
            ThumbnailError::ExceededLimits(_) => "E_DISK_FULL",
            ThumbnailError::Cancelled => "E_JOB_CANCELLED",
            ThumbnailError::Io(_) => "E_DISK_FULL",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
pub struct ThumbnailManifest {
    pub cache_key: String,
    pub source_rel_path: String,
    pub interval_secs: u32,
    pub width_px: u32,
    pub duration_ms: u32,
    pub frame_rel_paths: Vec<String>,
    pub total_bytes: f64,
}

#[derive(Debug, Clone)]
pub struct ThumbnailProgress {
    pub done_secs: f64,
    pub total_secs: f64,
    pub percent: f64,
    pub eta_seconds: Option<f64>,
}

pub fn compute_thumbnails_cache_key(
    source_rel_path: &str,
    duration_ms: u32,
    interval_secs: u32,
    width_px: u32,
) -> String {
    let mut hasher = Sha256::new();
    let payload = format!(
        "thumbs_v1|{}|{}|{}|{}",
        source_rel_path, duration_ms, interval_secs, width_px
    );
    hasher.update(payload.as_bytes());
    hex::encode(hasher.finalize())
}

pub struct ThumbnailSpecRequest<'a> {
    pub ffmpeg_bin: &'a Path,
    pub ws_root: &'a Path,
    pub working_dir: &'a Path,
    pub input_abs: &'a Path,
    pub output_pattern_abs: &'a Path,
    pub interval_secs: u32,
    pub width_px: u32,
    pub extra_env: &'a [(String, String)],
}

pub fn build_ffmpeg_thumbnails_spec(
    req: &ThumbnailSpecRequest<'_>,
) -> Result<SidecarSpec, ThumbnailError> {
    if !req.ffmpeg_bin.is_absolute() {
        return Err(ThumbnailError::ComponentMissing(
            "FFmpeg path must be absolute".to_string(),
        ));
    }
    let env = SidecarEnv::for_component("ffmpeg", req.ws_root);
    env.ensure_writable_dirs()
        .map_err(|e| ThumbnailError::Io(e.to_string()))?;

    let vf = format!(
        "fps=1/{},scale={}:-2",
        req.interval_secs.max(1),
        req.width_px.max(32)
    );
    let args = vec![
        "-y".to_string(),
        "-i".to_string(),
        req.input_abs.to_string_lossy().to_string(),
        "-vf".to_string(),
        vf,
        "-frames:v".to_string(),
        MAX_THUMBNAIL_FRAMES.to_string(),
        "-q:v".to_string(),
        "6".to_string(),
        "-progress".to_string(),
        "pipe:1".to_string(),
        "-nostats".to_string(),
        req.output_pattern_abs.to_string_lossy().to_string(),
    ];

    let mut spec = SidecarSpec::new(req.ffmpeg_bin.to_path_buf(), args)
        .working_dir(req.working_dir)
        .with_component_env(&env)
        .map_err(|e| ThumbnailError::Io(e.to_string()))?
        .timeout(Duration::from_secs(THUMBNAIL_TIMEOUT_SECS))
        .max_total_bytes(8 * 1024 * 1024);
    for (k, v) in req.extra_env {
        spec = spec.env(k, v);
    }
    Ok(spec)
}

fn map_resolver(e: ResolverError) -> ThumbnailError {
    match e {
        ResolverError::Missing(adv) => ThumbnailError::ComponentMissing(adv.message),
        other => ThumbnailError::ComponentMissing(other.to_string()),
    }
}

fn try_load_cached_manifest(ws_root: &Path, manifest_rel: &str) -> Option<ThumbnailManifest> {
    let manifest_abs = security::resolve_workspace_path_with_root(ws_root, manifest_rel).ok()?;
    if !manifest_abs.is_file() {
        return None;
    }
    let raw = std::fs::read_to_string(&manifest_abs).ok()?;
    let manifest: ThumbnailManifest = serde_json::from_str(&raw).ok()?;
    if manifest.frame_rel_paths.is_empty()
        || manifest.frame_rel_paths.len() > MAX_THUMBNAIL_FRAMES
        || manifest.total_bytes > MAX_THUMBNAIL_TOTAL_BYTES as f64
    {
        return None;
    }
    for rel in &manifest.frame_rel_paths {
        let p = security::resolve_workspace_path_with_root(ws_root, rel).ok()?;
        if !p.is_file() {
            return None;
        }
    }
    Some(manifest)
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Generates (or reuses cached) 2 s timeline thumbnails under `cache/thumbs/<cache_key>/`.
#[allow(clippy::too_many_arguments)]
pub async fn run_thumbnails_stage<F>(
    ws_root: &Path,
    resolver: &ComponentResolver,
    source_rel_path: &str,
    duration_ms: u32,
    extra_env: &[(String, String)],
    cancel_token: &CancellationToken,
    on_progress: F,
) -> Result<ThumbnailManifest, ThumbnailError>
where
    F: Fn(ThumbnailProgress) + Send + Sync + 'static,
{
    if cancel_token.is_cancelled() {
        return Err(ThumbnailError::Cancelled);
    }

    let source_abs = security::resolve_workspace_path_with_root(ws_root, source_rel_path)
        .map_err(|e| ThumbnailError::Io(e.to_string()))?;
    if !source_abs.is_file() {
        return Err(ThumbnailError::Io(
            "Source media missing for thumbnails".to_string(),
        ));
    }

    let cache_key = compute_thumbnails_cache_key(
        source_rel_path,
        duration_ms,
        THUMBNAIL_INTERVAL_SECS,
        THUMBNAIL_WIDTH_PX,
    );
    let final_dir_rel = format!("cache/thumbs/{}", cache_key);
    let manifest_rel = format!("{}/manifest.json", final_dir_rel);

    if let Some(cached) = try_load_cached_manifest(ws_root, &manifest_rel) {
        on_progress(ThumbnailProgress {
            done_secs: duration_ms as f64 / 1000.0,
            total_secs: duration_ms as f64 / 1000.0,
            percent: 100.0,
            eta_seconds: Some(0.0),
        });
        return Ok(cached);
    }

    let ffmpeg_res = resolver
        .resolve_executable("ffmpeg", "ffmpeg.exe")
        .map_err(map_resolver)?;

    let run_id = uuid::Uuid::new_v4().to_string();
    let tmp_dir_rel = format!("cache/tmp/thumbs_{}", run_id);
    let tmp_dir_abs = security::resolve_workspace_path_with_root(ws_root, &tmp_dir_rel)
        .map_err(|e| ThumbnailError::Io(e.to_string()))?;
    std::fs::create_dir_all(&tmp_dir_abs).map_err(|e| ThumbnailError::Io(e.to_string()))?;
    let pattern_abs = tmp_dir_abs.join("thumb_%04d.jpg");

    let spec = build_ffmpeg_thumbnails_spec(&ThumbnailSpecRequest {
        ffmpeg_bin: &ffmpeg_res.path,
        ws_root,
        working_dir: &tmp_dir_abs,
        input_abs: &source_abs,
        output_pattern_abs: &pattern_abs,
        interval_secs: THUMBNAIL_INTERVAL_SECS,
        width_px: THUMBNAIL_WIDTH_PX,
        extra_env,
    })?;

    let total_secs = (duration_ms as f64 / 1000.0).max(0.5);
    let tracker = Arc::new(Mutex::new(LiveStageEtaTracker::new(
        LiveEtaConfig::for_prepare_preview(None, "thumbs", total_secs),
        now_epoch_ms(),
    )));
    let parser = Arc::new(Mutex::new(FfmpegPipeProgressParser::new(Some(total_secs))));
    let last_emit = Arc::new(Mutex::new(Instant::now() - Duration::from_secs(1)));
    let on_prog = Arc::new(on_progress);

    let tracker_cb = tracker.clone();
    let parser_cb = parser.clone();
    let last_emit_cb = last_emit.clone();
    let on_prog_cb = on_prog.clone();

    let exec_res = SidecarSupervisor::new()
        .spawn_streaming_with_hooks(
            &spec,
            move |line| {
                let maybe_sample = parser_cb.lock().ok().and_then(|mut p| p.push_line(&line));
                if let Some(sample) = maybe_sample {
                    let done = sample.time_s;
                    if let Ok(mut tr) = tracker_cb.lock() {
                        let snap = tr.update(now_epoch_ms(), done);
                        let should_emit = last_emit_cb
                            .lock()
                            .map(|mut t| {
                                if t.elapsed() >= Duration::from_millis(250) {
                                    *t = Instant::now();
                                    true
                                } else {
                                    false
                                }
                            })
                            .unwrap_or(true);
                        if should_emit {
                            on_prog_cb(ThumbnailProgress {
                                done_secs: snap.done_units,
                                total_secs: snap.total_units,
                                percent: (snap.fraction * 100.0).clamp(0.0, 99.0),
                                eta_seconds: snap.eta.map(|e| e.seconds),
                            });
                        }
                    }
                }
            },
            |_| {},
            {
                let tracker_tick = tracker.clone();
                let on_prog_tick = on_prog.clone();
                move || {
                    if let Ok(mut tr) = tracker_tick.lock() {
                        let snap = tr.update(now_epoch_ms(), None);
                        on_prog_tick(ThumbnailProgress {
                            done_secs: snap.done_units,
                            total_secs: snap.total_units,
                            percent: (snap.fraction * 100.0).clamp(0.0, 99.0),
                            eta_seconds: snap.eta.map(|e| e.seconds),
                        });
                    }
                }
            },
            Some(cancel_token),
        )
        .await;

    if let Err(err) = exec_res {
        let _ = std::fs::remove_dir_all(&tmp_dir_abs);
        return Err(match err {
            SidecarError::Cancelled => ThumbnailError::Cancelled,
            other => ThumbnailError::Failed(security::redact_sensitive_text(&other.to_string())),
        });
    }

    let mut frame_names = Vec::new();
    let mut total_bytes: u64 = 0;
    if let Ok(entries) = std::fs::read_dir(&tmp_dir_abs) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("thumb_") && name.ends_with(".jpg") {
                        let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
                        total_bytes = total_bytes.saturating_add(len);
                        frame_names.push(name.to_string());
                    }
                }
            }
        }
    }
    frame_names.sort();

    if frame_names.is_empty() {
        let _ = std::fs::remove_dir_all(&tmp_dir_abs);
        return Err(ThumbnailError::Failed(
            "FFmpeg produced zero thumbnail frames".to_string(),
        ));
    }
    if frame_names.len() > MAX_THUMBNAIL_FRAMES || total_bytes > MAX_THUMBNAIL_TOTAL_BYTES {
        let _ = std::fs::remove_dir_all(&tmp_dir_abs);
        return Err(ThumbnailError::ExceededLimits(format!(
            "frames={} bytes={}",
            frame_names.len(),
            total_bytes
        )));
    }

    let frame_rel_paths: Vec<String> = frame_names
        .iter()
        .map(|name| format!("{}/{}", final_dir_rel, name))
        .collect();
    let manifest = ThumbnailManifest {
        cache_key: cache_key.clone(),
        source_rel_path: source_rel_path.to_string(),
        interval_secs: THUMBNAIL_INTERVAL_SECS,
        width_px: THUMBNAIL_WIDTH_PX,
        duration_ms,
        frame_rel_paths,
        total_bytes: total_bytes as f64,
    };

    let manifest_json =
        serde_json::to_string_pretty(&manifest).map_err(|e| ThumbnailError::Io(e.to_string()))?;
    std::fs::write(tmp_dir_abs.join("manifest.json"), manifest_json.as_bytes())
        .map_err(|e| ThumbnailError::Io(e.to_string()))?;

    let final_dir_abs = security::resolve_workspace_path_with_root(ws_root, &final_dir_rel)
        .map_err(|e| ThumbnailError::Io(e.to_string()))?;
    if let Some(parent) = final_dir_abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ThumbnailError::Io(e.to_string()))?;
    }
    if final_dir_abs.exists() {
        let _ = std::fs::remove_dir_all(&final_dir_abs);
    }
    std::fs::rename(&tmp_dir_abs, &final_dir_abs).map_err(|e| {
        let _ = std::fs::remove_dir_all(&tmp_dir_abs);
        ThumbnailError::Io(e.to_string())
    })?;

    on_prog(ThumbnailProgress {
        done_secs: total_secs,
        total_secs,
        percent: 100.0,
        eta_seconds: Some(0.0),
    });

    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::tests::setup_fake_components;
    use crate::installer::InUseRegistry;
    use crate::store::LocalStore;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_thumbnails_stage_fake_sidecar_caches_and_supports_cancel() {
        let tmp = TempDir::new().unwrap();
        let ws = security::ensure_workspace_with_root(tmp.path()).unwrap();
        let store = LocalStore::open(&ws.join("clipper.db")).unwrap();
        let _ = setup_fake_components(&ws, &store);
        let resolver = ComponentResolver::new(ws.clone(), store, InUseRegistry::new());

        let src_rel = "cache/source_sample.mp4";
        std::fs::create_dir_all(ws.join("cache")).unwrap();
        std::fs::write(ws.join(src_rel), b"FAKE_MP4").unwrap();

        let events = Arc::new(Mutex::new(Vec::<ThumbnailProgress>::new()));
        let ev_cb = events.clone();
        let tok = CancellationToken::new();
        let manifest = run_thumbnails_stage(
            &ws,
            &resolver,
            src_rel,
            10_000,
            &[("CLIPPER_FAKE_THUMBS_SCENARIO".into(), "success".into())],
            &tok,
            move |p| {
                ev_cb.lock().unwrap().push(p);
            },
        )
        .await
        .unwrap();

        assert_eq!(manifest.frame_rel_paths.len(), 5);
        assert!(manifest.total_bytes > 0.0);
        for rel in &manifest.frame_rel_paths {
            assert!(rel.starts_with("cache/thumbs/"));
            assert!(ws.join(rel).is_file());
        }
        assert!(events.lock().unwrap().last().unwrap().percent == 100.0);

        // Cache hit even if fake-sidecar set to fail
        let cached = run_thumbnails_stage(
            &ws,
            &resolver,
            src_rel,
            10_000,
            &[("CLIPPER_FAKE_THUMBS_SCENARIO".into(), "fail".into())],
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(cached.cache_key, manifest.cache_key);

        // Cancel pre-flight
        let cancelled_tok = CancellationToken::new();
        cancelled_tok.cancel();
        let res =
            run_thumbnails_stage(&ws, &resolver, src_rel, 12_000, &[], &cancelled_tok, |_| {})
                .await
                .unwrap_err();
        assert!(matches!(res, ThumbnailError::Cancelled));
    }

    #[tokio::test]
    #[ignore = "Local-only real FFmpeg thumbnail check (set CLIPPER_LIVE_SAMPLE_MP4)"]
    async fn test_local_real_ffmpeg_thumbnails() {
        let Ok(sample_str) = std::env::var("CLIPPER_LIVE_SAMPLE_MP4") else {
            return;
        };
        let tmp = TempDir::new().unwrap();
        let ws = security::ensure_workspace_with_root(tmp.path()).unwrap();
        let store = LocalStore::open(&ws.join("clipper.db")).unwrap();
        let resolver = ComponentResolver::new(ws.clone(), store, InUseRegistry::new());
        let src_rel = "cache/live_sample.mp4";
        std::fs::create_dir_all(ws.join("cache")).unwrap();
        std::fs::copy(sample_str, ws.join(src_rel)).unwrap();
        let m = run_thumbnails_stage(
            &ws,
            &resolver,
            src_rel,
            30_000,
            &[],
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        assert!(!m.frame_rel_paths.is_empty());
    }
}
