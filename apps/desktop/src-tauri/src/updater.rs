//! Application and component updater, resumable downloader, and integrity verification
//! (ARCHITECTURE §4, §9, §13; PROGRESS_AND_ETA §4–§5, §8; SECURITY §7–§10).

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use specta::Type;
use tokio_util::sync::CancellationToken;

use crate::manifest::{self, ComponentManifest, ComponentManifestEntry, ManifestPublicKey};
use crate::net::{create_secure_client, validate_download_url, NetError};
use crate::security;

/// Lock tracking in-progress downloads to prevent duplicate concurrent transfers of the same component.
static DOWNLOADING_COMPONENTS: LazyLock<Arc<Mutex<HashSet<String>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(HashSet::new())));

/// Progress event emitted during component download.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct DownloadProgress {
    pub component: String,
    pub stage: String, // "download"
    pub unit: String,  // "bytes"
    pub completed_bytes: u64,
    pub total_bytes: u64,
    pub percent: f64,
    pub rate_bps: f64,
    pub eta_seconds: Option<f64>,
}

#[derive(Serialize, Deserialize, Debug)]
struct PartMeta {
    etag: Option<String>,
    bytes: u64,
}

/// Computes the estimated extracted size for a component.
pub fn calculate_extracted_size(entry: &ComponentManifestEntry) -> u64 {
    if entry.archive == "zip" {
        if !entry.files.is_empty() {
            entry.files.iter().map(|f| f.size_bytes).sum()
        } else {
            // Default 2.5x estimate for zip compression
            entry.size_bytes.saturating_mul(5) / 2
        }
    } else {
        entry.size_bytes
    }
}

/// Downloads and verifies a component file or archive against its manifest entry.
///
/// Features:
/// - Pre-flight disk space check (download size + extracted size).
/// - Resumable via `.part` file + Range + If-Range headers with prefix hash verification.
/// - Incremental streaming SHA-256 calculation.
/// - Strict size bounds (aborts if body exceeds declared size).
/// - Concurrency lock per component.
/// - Progress events with EWMA smoothed rate and ETA per PROGRESS_AND_ETA §4–§5.
/// - 2-second synthetic heartbeat.
pub async fn download_and_verify_component<F>(
    entry: &ComponentManifestEntry,
    dest_path: &Path,
    on_progress: F,
    cancel_token: Option<&CancellationToken>,
) -> Result<(), NetError>
where
    F: Fn(DownloadProgress) + Send + Sync + 'static,
{
    // 1. Acquire per-component concurrency lock
    {
        let mut lock = DOWNLOADING_COMPONENTS
            .lock()
            .map_err(|_| NetError::Io("Download mutex poisoned".to_string()))?;
        if !lock.insert(entry.component.clone()) {
            return Err(NetError::AlreadyDownloading(entry.component.clone()));
        }
    }

    // Ensure lock is released upon exit
    struct LockGuard(String);
    impl Drop for LockGuard {
        fn drop(&mut self) {
            if let Ok(mut lock) = DOWNLOADING_COMPONENTS.lock() {
                lock.remove(&self.0);
            }
        }
    }
    let _guard = LockGuard(entry.component.clone());

    // 2. Validate URL
    let download_url = validate_download_url(&entry.url)?;

    // 3. Pre-flight disk space check (downloaded size + extracted size)
    let required_space = entry
        .size_bytes
        .saturating_add(calculate_extracted_size(entry));
    let check_dir = dest_path.parent().unwrap_or(dest_path);
    if let Err(err) = security::check_free_space_at(check_dir, required_space) {
        match err {
            security::SecurityError::DiskFull { .. } => {
                return Err(NetError::DiskFull(err.to_string()));
            }
            security::SecurityError::DiskLow { .. } => {
                return Err(NetError::DiskLow(err.to_string()));
            }
            _ => return Err(NetError::Io(err.to_string())),
        }
    }

    // 4. File paths: target, .part, and .part.meta
    if let Some(parent) = dest_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| NetError::Io(e.to_string()))?;
    }

    let part_path: PathBuf = format!("{}.part", dest_path.display()).into();
    let meta_path: PathBuf = format!("{}.part.meta", dest_path.display()).into();

    let mut existing_len: u64 = 0;
    let mut saved_etag: Option<String> = None;
    let mut hasher = Sha256::new();

    if part_path.exists() {
        if let Ok(meta_bytes) = std::fs::read_to_string(&meta_path) {
            if let Ok(meta) = serde_json::from_str::<PartMeta>(&meta_bytes) {
                saved_etag = meta.etag;
            }
        }

        if let Ok(meta_info) = std::fs::metadata(&part_path) {
            let len = meta_info.len();
            if len > 0 && len < entry.size_bytes {
                existing_len = len;
                // Hash existing prefix from disk
                if let Ok(mut f) = File::open(&part_path) {
                    let mut buf = [0u8; 65536];
                    let mut read_total = 0;
                    while read_total < existing_len {
                        let to_read =
                            std::cmp::min(buf.len() as u64, existing_len - read_total) as usize;
                        if let Ok(n) = f.read(&mut buf[..to_read]) {
                            if n == 0 {
                                break;
                            }
                            hasher.update(&buf[..n]);
                            read_total += n as u64;
                        } else {
                            break;
                        }
                    }
                    if read_total != existing_len {
                        // File read error: reset
                        existing_len = 0;
                        hasher = Sha256::new();
                    }
                }
            }
        }
    }

    // 5. Build request with retry & Range
    let client = create_secure_client()?;
    let mut file: File;
    let mut completed_bytes: u64;

    let mut attempts = 0;
    let mut res_response = None;

    while attempts < 3 {
        attempts += 1;
        let mut req = client.get(download_url.clone());

        if existing_len > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={}-", existing_len));
            if let Some(ref et) = saved_etag {
                req = req.header(reqwest::header::IF_RANGE, et.clone());
            }
        }

        match req.send().await {
            Ok(resp) => {
                if resp.status().is_server_error() && attempts < 3 {
                    tokio::time::sleep(Duration::from_millis(500 * (1 << (attempts - 1)))).await;
                    continue;
                }
                res_response = Some(resp);
                break;
            }
            Err(e) => {
                if attempts < 3 {
                    tokio::time::sleep(Duration::from_millis(500 * (1 << (attempts - 1)))).await;
                    continue;
                }
                return Err(NetError::Offline(e.to_string()));
            }
        }
    }

    let response =
        res_response.ok_or_else(|| NetError::Offline("Failed after 3 attempts".to_string()))?;
    let status = response.status();

    if status == reqwest::StatusCode::PARTIAL_CONTENT && existing_len > 0 {
        // Resuming: open in append mode
        file = OpenOptions::new()
            .append(true)
            .open(&part_path)
            .map_err(|e| NetError::Io(e.to_string()))?;
        completed_bytes = existing_len;
    } else if status.is_success() {
        // Full content or server reset: open in create/truncate mode
        file = File::create(&part_path).map_err(|e| NetError::Io(e.to_string()))?;
        hasher = Sha256::new();
        completed_bytes = 0;
    } else {
        return Err(NetError::HttpError(status.as_u16(), status.to_string()));
    }

    // Extract ETag
    let new_etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let meta = PartMeta {
        etag: new_etag,
        bytes: completed_bytes,
    };
    if let Ok(meta_json) = serde_json::to_string(&meta) {
        let _ = std::fs::write(&meta_path, meta_json);
    }

    // 6. Streaming loop with progress & rate calculation
    let mut stream = response.bytes_stream();
    let start_time = Instant::now();
    let mut last_progress_emit = Instant::now();
    let mut last_heartbeat = Instant::now();

    let mut smoothed_rate = 0.0;
    let mut last_bytes = completed_bytes;
    let mut last_rate_calc = Instant::now();

    while let Some(chunk_res) = stream.next().await {
        if let Some(token) = cancel_token {
            if token.is_cancelled() {
                let _ = file.flush();
                return Err(NetError::Cancelled);
            }
        }

        let chunk = chunk_res.map_err(|e| NetError::Offline(e.to_string()))?;
        let chunk_len = chunk.len() as u64;

        // Strict size check: abort if streamed bytes exceed declared size
        if completed_bytes.saturating_add(chunk_len) > entry.size_bytes {
            return Err(NetError::SizeExceeded {
                actual: completed_bytes.saturating_add(chunk_len),
                declared: entry.size_bytes,
            });
        }

        file.write_all(&chunk)
            .map_err(|e| NetError::Io(e.to_string()))?;
        hasher.update(&chunk);
        completed_bytes += chunk_len;

        // EWMA Rate & ETA Calculation
        let now = Instant::now();
        let elapsed_since_rate = now.duration_since(last_rate_calc).as_secs_f64();
        if elapsed_since_rate >= 0.25 {
            let instant_rate = (completed_bytes - last_bytes) as f64 / elapsed_since_rate;
            if smoothed_rate <= 0.0 {
                smoothed_rate = instant_rate;
            } else {
                smoothed_rate = 0.25 * instant_rate + 0.75 * smoothed_rate;
            }
            last_bytes = completed_bytes;
            last_rate_calc = now;
        }

        let eta_seconds = if smoothed_rate > 0.0 {
            Some((entry.size_bytes.saturating_sub(completed_bytes)) as f64 / smoothed_rate)
        } else {
            None
        };

        let percent = if entry.size_bytes > 0 {
            (completed_bytes as f64 / entry.size_bytes as f64) * 100.0
        } else {
            100.0
        };

        // Emit progress if >= 200ms or 2-second heartbeat
        let is_heartbeat = now.duration_since(last_heartbeat) >= Duration::from_secs(2);
        if now.duration_since(last_progress_emit) >= Duration::from_millis(200) || is_heartbeat {
            on_progress(DownloadProgress {
                component: entry.component.clone(),
                stage: "download".to_string(),
                unit: "bytes".to_string(),
                completed_bytes,
                total_bytes: entry.size_bytes,
                percent,
                rate_bps: smoothed_rate,
                eta_seconds,
            });
            last_progress_emit = now;
            if is_heartbeat {
                last_heartbeat = now;
            }
        }
    }

    file.flush().map_err(|e| NetError::Io(e.to_string()))?;

    // 7. Verify size
    if completed_bytes != entry.size_bytes {
        return Err(NetError::SizeMismatch {
            actual: completed_bytes,
            declared: entry.size_bytes,
        });
    }

    // 8. Verify streaming SHA-256
    let calculated_sha = hex::encode(hasher.finalize());
    if !calculated_sha.eq_ignore_ascii_case(&entry.sha256) {
        let _ = std::fs::remove_file(&part_path);
        let _ = std::fs::remove_file(&meta_path);
        return Err(NetError::VerificationFailed(
            entry.component.clone(),
            entry.sha256.clone(),
            calculated_sha,
        ));
    }

    // 9. Atomic finish: rename .part to destination
    if dest_path.exists() {
        let _ = std::fs::remove_file(dest_path);
    }
    std::fs::rename(&part_path, dest_path).map_err(|e| NetError::Io(e.to_string()))?;
    let _ = std::fs::remove_file(&meta_path);

    // Final 100% progress
    let total_elapsed = start_time.elapsed().as_secs_f64();
    let final_rate = if total_elapsed > 0.0 {
        completed_bytes as f64 / total_elapsed
    } else {
        smoothed_rate
    };

    on_progress(DownloadProgress {
        component: entry.component.clone(),
        stage: "download".to_string(),
        unit: "bytes".to_string(),
        completed_bytes,
        total_bytes: entry.size_bytes,
        percent: 100.0,
        rate_bps: final_rate,
        eta_seconds: Some(0.0),
    });

    Ok(())
}

/// Fetches the manifest and its detached signature from remote URLs, validates them,
/// and updates the sequence number in the database.
pub async fn fetch_and_verify_manifest(
    manifest_url: &str,
    sig_url: &str,
    app_version: &str,
    store: &crate::store::LocalStore,
    extra_keys: Option<&[ManifestPublicKey]>,
) -> Result<ComponentManifest, NetError> {
    let client = create_secure_client()?;

    let valid_manifest_url = validate_download_url(manifest_url)?;
    let valid_sig_url = validate_download_url(sig_url)?;

    let manifest_resp = client
        .get(valid_manifest_url)
        .send()
        .await
        .map_err(|e| NetError::Offline(e.to_string()))?;
    if !manifest_resp.status().is_success() {
        return Err(NetError::HttpError(
            manifest_resp.status().as_u16(),
            manifest_resp.status().to_string(),
        ));
    }
    let manifest_bytes = manifest_resp
        .bytes()
        .await
        .map_err(|e| NetError::Offline(e.to_string()))?;

    let sig_resp = client
        .get(valid_sig_url)
        .send()
        .await
        .map_err(|e| NetError::Offline(e.to_string()))?;
    if !sig_resp.status().is_success() {
        return Err(NetError::HttpError(
            sig_resp.status().as_u16(),
            sig_resp.status().to_string(),
        ));
    }
    let sig_bytes = sig_resp
        .bytes()
        .await
        .map_err(|e| NetError::Offline(e.to_string()))?;

    manifest::verify_and_accept_manifest(
        &manifest_bytes,
        &sig_bytes,
        app_version,
        store,
        extra_keys,
    )
    .map_err(|e| match e {
        manifest::ManifestError::BadSignature(_) => NetError::VerificationFailed(
            "manifest.json".into(),
            "valid signature".into(),
            "invalid".into(),
        ),
        other => NetError::Offline(other.to_string()),
    })
}
