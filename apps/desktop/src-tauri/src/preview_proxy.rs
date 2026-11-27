//! Conditional H.264/AAC WebView2 preview proxy stage (`prepare_preview`, ARCHITECTURE §5.1, §6;
//! S1c Part D; S4/S5 `h264_mf` option rules).
//!
//! Only runs when the downloaded source clip is not natively WebView2-playable
//! (e.g. VP9/AV1/Opus). Keeps the original downloaded file untouched as the export source.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::download::{verify_media_with_ffprobe_with_env, DownloadError, VerifiedMediaInfo};
use crate::eta::{LiveEtaConfig, LiveEtaSnapshot, LiveStageEtaTracker};
use crate::jobs::{cleanup_job_temp_dir, job_temp_dir};
use crate::progress_parser::FfmpegPipeProgressParser;
use crate::resolver::{ComponentResolver, ResolverError};
use crate::security;
use crate::sidecars::{SidecarEnv, SidecarError, SidecarSpec, SidecarSupervisor};
use crate::store::LocalStore;

pub const PROXY_PRESET_ID: &str = "h264_mf_3500k_g30_yuv420p_aac128k";
pub const PROXY_TIMEOUT_SECS: u64 = 30 * 60;

#[derive(Error, Debug)]
pub enum PreviewProxyError {
    #[error("Component missing: {message}")]
    ComponentMissing { component: String, message: String },

    #[error("FFmpeg preview proxy failed: {detail}")]
    TranscodeFailed { detail: String, stderr_tail: String },

    #[error("Preview proxy verification failed: {reason}")]
    VerifyFailed { reason: String, detail: String },

    #[error("Preview proxy cancelled")]
    Cancelled,

    #[error("IO error during preview proxy: {0}")]
    Io(String),
}

impl PreviewProxyError {
    pub fn error_code(&self) -> &'static str {
        match self {
            PreviewProxyError::ComponentMissing { .. } => "E_COMPONENT_MISSING",
            PreviewProxyError::TranscodeFailed { .. } => "E_RENDER_FAILED",
            PreviewProxyError::VerifyFailed { .. } => "E_VERIFY_FAILED",
            PreviewProxyError::Cancelled => "E_JOB_CANCELLED",
            PreviewProxyError::Io(_) => "E_DISK_FULL",
        }
    }

    pub fn message_key(&self) -> String {
        format!("err.{}.body", self.error_code())
    }
}

#[derive(Debug, Clone)]
pub struct ProxyStageProgress {
    pub snapshot: LiveEtaSnapshot,
    pub speed_ratio: Option<f64>,
    pub fps: Option<f64>,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ProxyStageOutcome {
    pub cache_key: String,
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub media: VerifiedMediaInfo,
    pub cache_hit: bool,
    pub wall_ms: u64,
}

pub fn compute_proxy_cache_key(source_cache_key: &str) -> String {
    let mut hasher = Sha256::new();
    let payload = format!("proxy_v1|{}|{}", source_cache_key, PROXY_PRESET_ID);
    hasher.update(payload.as_bytes());
    hex::encode(hasher.finalize())
}

/// Builds the supervised FFmpeg spec using S4/S5 `h264_mf` flags (3500k, `-g 30`, `yuv420p`, `aac`,
/// `-movflags +faststart`, `-progress pipe:1`, NO `-profile:v`).
pub fn build_ffmpeg_proxy_spec(
    ffmpeg_bin: PathBuf,
    ws_root: &Path,
    working_dir: &Path,
    source_abs: &Path,
    output_abs: &Path,
    extra_env: &[(String, String)],
) -> Result<SidecarSpec, PreviewProxyError> {
    if !ffmpeg_bin.is_absolute() {
        return Err(PreviewProxyError::ComponentMissing {
            component: "ffmpeg".to_string(),
            message: "FFmpeg binary path must be absolute".to_string(),
        });
    }

    let env = SidecarEnv::for_component("ffmpeg", ws_root);
    env.ensure_writable_dirs()
        .map_err(|e| PreviewProxyError::Io(e.to_string()))?;

    let args = vec![
        "-y".to_string(),
        "-i".to_string(),
        source_abs.to_string_lossy().to_string(),
        "-c:v".to_string(),
        "h264_mf".to_string(),
        "-b:v".to_string(),
        "3500k".to_string(),
        "-g".to_string(),
        "30".to_string(),
        "-pix_fmt".to_string(),
        "yuv420p".to_string(),
        "-c:a".to_string(),
        "aac".to_string(),
        "-b:a".to_string(),
        "128k".to_string(),
        "-movflags".to_string(),
        "+faststart".to_string(),
        "-progress".to_string(),
        "pipe:1".to_string(),
        "-nostats".to_string(),
        output_abs.to_string_lossy().to_string(),
    ];

    let mut spec = SidecarSpec::new(ffmpeg_bin, args)
        .working_dir(working_dir)
        .with_component_env(&env)
        .map_err(|e| PreviewProxyError::Io(e.to_string()))?
        .timeout(Duration::from_secs(PROXY_TIMEOUT_SECS))
        .max_total_bytes(16 * 1024 * 1024);

    for (k, v) in extra_env {
        spec = spec.env(k, v);
    }

    Ok(spec)
}

fn epoch_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn map_resolver(e: ResolverError) -> PreviewProxyError {
    match e {
        ResolverError::Missing(adv) => PreviewProxyError::ComponentMissing {
            component: adv.component,
            message: adv.message,
        },
        other => PreviewProxyError::ComponentMissing {
            component: "ffmpeg".to_string(),
            message: other.to_string(),
        },
    }
}

/// Transcodes a non-H.264/AAC source artifact to `cache/proxies/<proxy_key>.mp4` via `h264_mf`.
#[allow(clippy::too_many_arguments)]
pub async fn run_prepare_preview_stage<F>(
    ws_root: &Path,
    store: &LocalStore,
    resolver: &ComponentResolver,
    device_profile: &str,
    job_id: &str,
    source_cache_key: &str,
    source_abs: &Path,
    expected_duration_secs: f64,
    extra_env: &[(String, String)],
    cancel_token: &CancellationToken,
    on_progress: F,
) -> Result<ProxyStageOutcome, PreviewProxyError>
where
    F: Fn(ProxyStageProgress) + Send + Sync + 'static,
{
    if cancel_token.is_cancelled() {
        return Err(PreviewProxyError::Cancelled);
    }

    let ffmpeg_res = resolver
        .resolve_executable("ffmpeg", "ffmpeg.exe")
        .map_err(map_resolver)?;
    let ffprobe_res = resolver
        .resolve_executable("ffmpeg", "ffprobe.exe")
        .map_err(map_resolver)?;

    let cache_key = compute_proxy_cache_key(source_cache_key);
    let rel_path = format!("cache/proxies/{}.mp4", cache_key);
    let final_abs = security::resolve_workspace_path_with_root(ws_root, &rel_path)
        .map_err(|e| PreviewProxyError::Io(e.to_string()))?;

    // Cache hit if valid native proxy already exists
    if final_abs.is_file() {
        if let Ok(verified) = verify_media_with_ffprobe_with_env(
            &ffprobe_res.path,
            ws_root,
            &final_abs,
            Some(expected_duration_secs),
            extra_env,
            Some(cancel_token),
        )
        .await
        {
            if verified.is_webview_native {
                return Ok(ProxyStageOutcome {
                    cache_key,
                    rel_path,
                    abs_path: final_abs,
                    media: verified,
                    cache_hit: true,
                    wall_ms: 0,
                });
            }
        }
    }

    let tmp_dir =
        job_temp_dir(ws_root, job_id).map_err(|e| PreviewProxyError::Io(e.to_string()))?;
    std::fs::create_dir_all(&tmp_dir).map_err(|e| PreviewProxyError::Io(e.to_string()))?;
    let tmp_proxy = tmp_dir.join(format!("proxy_{}.mp4", cache_key));

    let spec = build_ffmpeg_proxy_spec(
        ffmpeg_res.path.clone(),
        ws_root,
        &tmp_dir,
        source_abs,
        &tmp_proxy,
        extra_env,
    )?;

    let started = Instant::now();
    let tracker = Arc::new(Mutex::new(LiveStageEtaTracker::new(
        LiveEtaConfig::for_prepare_preview(Some(store), device_profile, expected_duration_secs),
        epoch_ms_now(),
    )));
    let parser = Arc::new(Mutex::new(FfmpegPipeProgressParser::new(Some(
        expected_duration_secs,
    ))));
    let cb = Arc::new(on_progress);

    let parser_out = parser.clone();
    let tracker_out = tracker.clone();
    let cb_out = cb.clone();
    let tracker_tick = tracker.clone();
    let cb_tick = cb.clone();

    let sidecar_res = SidecarSupervisor::new()
        .spawn_streaming_with_hooks(
            &spec,
            move |line| {
                if let Ok(mut p) = parser_out.lock() {
                    if let Some(parsed) = p.push_line(&line) {
                        if let Ok(mut tr) = tracker_out.lock() {
                            let snap = tr.update(epoch_ms_now(), parsed.time_s);
                            cb_out(ProxyStageProgress {
                                snapshot: snap,
                                speed_ratio: parsed.speed_ratio,
                                fps: parsed.fps,
                                size_bytes: parsed.size_bytes,
                            });
                        }
                    }
                }
            },
            |_| {},
            move || {
                if let Ok(mut tr) = tracker_tick.lock() {
                    let snap = tr.update(epoch_ms_now(), None);
                    cb_tick(ProxyStageProgress {
                        snapshot: snap,
                        speed_ratio: None,
                        fps: None,
                        size_bytes: None,
                    });
                }
            },
            Some(cancel_token),
        )
        .await;

    match sidecar_res {
        Ok(res) if res.truncated => {
            cleanup_job_temp_dir(ws_root, job_id);
            return Err(PreviewProxyError::TranscodeFailed {
                detail: "FFmpeg proxy output exceeded cap".to_string(),
                stderr_tail: res.stderr_tail,
            });
        }
        Ok(_) => {}
        Err(SidecarError::Cancelled) => {
            cleanup_job_temp_dir(ws_root, job_id);
            return Err(PreviewProxyError::Cancelled);
        }
        Err(SidecarError::ExecutionFailed {
            exit_code,
            stderr_tail,
        }) => {
            cleanup_job_temp_dir(ws_root, job_id);
            return Err(PreviewProxyError::TranscodeFailed {
                detail: format!("FFmpeg h264_mf exited with {:?}", exit_code),
                stderr_tail,
            });
        }
        Err(other) => {
            cleanup_job_temp_dir(ws_root, job_id);
            return Err(PreviewProxyError::TranscodeFailed {
                detail: security::redact_sensitive_text(&other.to_string()),
                stderr_tail: String::new(),
            });
        }
    }

    let verified = match verify_media_with_ffprobe_with_env(
        &ffprobe_res.path,
        ws_root,
        &tmp_proxy,
        Some(expected_duration_secs),
        extra_env,
        Some(cancel_token),
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            cleanup_job_temp_dir(ws_root, job_id);
            return Err(match e {
                DownloadError::VerifyFailed { reason, detail } => {
                    PreviewProxyError::VerifyFailed { reason, detail }
                }
                other => PreviewProxyError::VerifyFailed {
                    reason: "Proxy pratinjau gagal diverifikasi.".to_string(),
                    detail: other.to_string(),
                },
            });
        }
    };

    if let Some(parent) = final_abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| PreviewProxyError::Io(e.to_string()))?;
    }
    if final_abs.exists() {
        let _ = std::fs::remove_file(&final_abs);
    }
    std::fs::rename(&tmp_proxy, &final_abs).map_err(|e| {
        cleanup_job_temp_dir(ws_root, job_id);
        PreviewProxyError::Io(e.to_string())
    })?;
    cleanup_job_temp_dir(ws_root, job_id);

    let wall_ms = started.elapsed().as_millis() as u64;
    let _ = store.insert_stage_stat(
        device_profile,
        "prepare_preview",
        "h264_mf",
        expected_duration_secs,
        wall_ms.max(1) as i64,
    );

    Ok(ProxyStageOutcome {
        cache_key,
        rel_path,
        abs_path: final_abs,
        media: verified,
        cache_hit: false,
        wall_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::tests::setup_fake_components;
    use crate::download::verify_media_with_ffprobe;
    use crate::installer::InUseRegistry;
    use tempfile::TempDir;

    #[test]
    fn test_ffmpeg_proxy_spec_uses_h264_mf_3500k_g30_no_profile() {
        let tmp = TempDir::new().unwrap();
        let ws = security::ensure_workspace_with_root(tmp.path()).unwrap();
        let ffmpeg = ws.join("bin").join("ffmpeg").join("n9").join("ffmpeg.exe");
        let spec = build_ffmpeg_proxy_spec(
            ffmpeg,
            &ws,
            &ws,
            &ws.join("cache").join("src.webm"),
            &ws.join("cache").join("proxy.mp4"),
            &[],
        )
        .unwrap();

        assert!(spec.args.contains(&"h264_mf".to_string()));
        assert!(spec.args.contains(&"3500k".to_string()));
        assert!(spec.args.contains(&"-g".to_string()));
        assert!(spec.args.contains(&"30".to_string()));
        assert!(spec.args.contains(&"yuv420p".to_string()));
        assert!(spec.args.contains(&"pipe:1".to_string()));
        assert!(
            !spec.args.iter().any(|a| a == "-profile:v"),
            "h264_mf must NOT pass -profile:v per S4/S5"
        );
    }

    #[tokio::test]
    async fn test_prepare_preview_stage_fake_sidecar_progress_and_cache_hit() {
        let tmp = TempDir::new().unwrap();
        let ws = security::ensure_workspace_with_root(tmp.path()).unwrap();
        let store = LocalStore::open(&ws.join("clipper.db")).unwrap();
        let _ = setup_fake_components(&ws, &store);
        let resolver = ComponentResolver::new(ws.clone(), store.clone(), InUseRegistry::new());

        let source_abs = ws.join("cache").join("source_vp9.webm");
        std::fs::create_dir_all(source_abs.parent().unwrap()).unwrap();
        std::fs::write(&source_abs, b"FAKE_VP9_WEBM_DATA").unwrap();

        let progress_calls = Arc::new(Mutex::new(0usize));
        let pc = progress_calls.clone();
        let env = vec![
            ("CLIPPER_FAKE_FFMPEG_SCENARIO".into(), "success".into()),
            ("CLIPPER_FAKE_FFPROBE_SCENARIO".into(), "avc1".into()),
            ("CLIPPER_FAKE_FFPROBE_DURATION".into(), "70.0".into()),
        ];

        let out = run_prepare_preview_stage(
            &ws,
            &store,
            &resolver,
            "prof1",
            "job-proxy-1",
            "src_cache_key_1",
            &source_abs,
            70.0,
            &env,
            &CancellationToken::new(),
            move |_| {
                *pc.lock().unwrap() += 1;
            },
        )
        .await
        .unwrap();

        assert!(!out.cache_hit);
        assert!(out.rel_path.starts_with("cache/proxies/"));
        assert!(out.abs_path.is_file());
        assert!(*progress_calls.lock().unwrap() >= 3);
        assert!(store
            .get_median_rate("prof1", "prepare_preview", "h264_mf", 10)
            .unwrap()
            .is_some());

        // Second run hits proxy cache
        let out_hit = run_prepare_preview_stage(
            &ws,
            &store,
            &resolver,
            "prof1",
            "job-proxy-2",
            "src_cache_key_1",
            &source_abs,
            70.0,
            &env,
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        assert!(out_hit.cache_hit);
        assert_eq!(out_hit.cache_key, out.cache_key);
    }

    /// Local-only VP9 sample transcode test via real `h264_mf` + `ffprobe`.
    /// Run manually with `$env:CLIPPER_LIVE_VP9_SAMPLE="path\to\sample_vp9.webm"; cargo test --lib preview_proxy::tests::test_local_real_vp9_preview_proxy -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn test_local_real_vp9_preview_proxy() {
        let Ok(sample_str) = std::env::var("CLIPPER_LIVE_VP9_SAMPLE") else {
            return;
        };
        let sample_path = PathBuf::from(sample_str);
        if !sample_path.is_file() {
            return;
        }
        let ws = security::ensure_workspace().expect("ensure workspace");
        let store = LocalStore::open(&ws.join("clipper.db")).expect("open store");
        let resolver = ComponentResolver::new(ws.clone(), store.clone(), InUseRegistry::new());
        let ffprobe = resolver
            .resolve_executable("ffmpeg", "ffprobe.exe")
            .unwrap();
        let src_info = verify_media_with_ffprobe(&ffprobe.path, &ws, &sample_path, None, None)
            .await
            .expect("probe vp9 sample");
        assert!(!src_info.is_webview_native);

        let out = run_prepare_preview_stage(
            &ws,
            &store,
            &resolver,
            "live-vp9",
            &format!("live-proxy-{}", uuid::Uuid::new_v4()),
            "live_vp9_sample",
            &sample_path,
            src_info.duration_ms as f64 / 1000.0,
            &[],
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .expect("real h264_mf proxy");

        assert!(out.media.is_webview_native);
    }
}
