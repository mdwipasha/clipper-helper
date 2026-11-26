#![cfg(debug_assertions)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clipper_helper_desktop_lib::manifest::{
    ComponentManifest, ComponentManifestEntry, ManifestPublicKey,
};
use clipper_helper_desktop_lib::net::NetError;
use clipper_helper_desktop_lib::store::LocalStore;
use clipper_helper_desktop_lib::updater::{
    download_and_verify_component, fetch_and_verify_manifest, DownloadProgress,
};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

struct DevServerGuard {
    child: Child,
    url: String,
}

impl DevServerGuard {
    fn start(root_dir: &Path) -> Self {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let script_path = manifest_dir.join("../../../tools/src/dev-server.js");
        let script_path = dunce::canonicalize(&script_path).expect("canonicalize dev-server.js");

        let mut child = Command::new("node")
            .arg(script_path)
            .arg("--root")
            .arg(root_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn node dev-server.js");

        let stdout = child.stdout.take().expect("capture dev-server stdout");
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read server ready line");

        let prefix = "SERVER_READY: ";
        assert!(
            line.starts_with(prefix),
            "expected SERVER_READY prefix, got: {}",
            line
        );
        let url = line[prefix.len()..].trim().to_string();

        DevServerGuard { child, url }
    }
}

impl Drop for DevServerGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn compute_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

#[tokio::test]
async fn test_download_success_and_sha256_verification() {
    let temp_srv = TempDir::new().expect("srv dir");
    let server = DevServerGuard::start(temp_srv.path());

    // Create a dummy 32 KB payload
    let test_data = vec![0x42u8; 32 * 1024];
    let sha = compute_sha256(&test_data);
    let artifact_path = temp_srv.path().join("model.bin");
    std::fs::write(&artifact_path, &test_data).expect("write artifact");

    let temp_dest = TempDir::new().expect("dest dir");
    let dest_file = temp_dest.path().join("model.bin");

    let entry = ComponentManifestEntry {
        component: "test-model".into(),
        version: "1.0.0".into(),
        platform: "windows-x86_64".into(),
        url: format!("{}/model.bin", server.url),
        size_bytes: test_data.len() as u64,
        sha256: sha.clone(),
        archive: "none".into(),
        mirrors: vec![],
        files: vec![],
        min_app_version: "0.1.0".into(),
        max_app_version: None,
        rollout_percent: 100,
    };

    let progress_events = Arc::new(Mutex::new(Vec::<DownloadProgress>::new()));
    let p_clone = progress_events.clone();

    let res = download_and_verify_component(
        &entry,
        &dest_file,
        move |p| {
            p_clone.lock().unwrap().push(p);
        },
        None,
    )
    .await;

    assert!(res.is_ok(), "download must succeed: {:?}", res.err());
    assert!(dest_file.exists(), "destination file must exist");

    let downloaded_bytes = std::fs::read(&dest_file).expect("read dest");
    assert_eq!(compute_sha256(&downloaded_bytes), sha);

    let events = progress_events.lock().unwrap();
    assert!(!events.is_empty(), "progress events must be emitted");
    let last = events.last().unwrap();
    assert_eq!(last.percent, 100.0);
    assert_eq!(last.completed_bytes, test_data.len() as u64);
}

#[tokio::test]
async fn test_download_wrong_content_sha_mismatch() {
    let temp_srv = TempDir::new().expect("srv dir");
    let server = DevServerGuard::start(temp_srv.path());

    let test_data = vec![0x11u8; 16 * 1024];
    let expected_sha = compute_sha256(&test_data);
    let artifact_path = temp_srv.path().join("corrupt.bin");
    std::fs::write(&artifact_path, &test_data).expect("write artifact");

    let temp_dest = TempDir::new().expect("dest dir");
    let dest_file = temp_dest.path().join("corrupt.bin");

    // dev-server ?test_mode=wrong_content corrupts the delivered bytes
    let entry = ComponentManifestEntry {
        component: "corrupt-comp".into(),
        version: "1.0.0".into(),
        platform: "windows-x86_64".into(),
        url: format!("{}/corrupt.bin?test_mode=wrong_content", server.url),
        size_bytes: test_data.len() as u64,
        sha256: expected_sha,
        archive: "none".into(),
        mirrors: vec![],
        files: vec![],
        min_app_version: "0.1.0".into(),
        max_app_version: None,
        rollout_percent: 100,
    };

    let res = download_and_verify_component(&entry, &dest_file, |_| {}, None).await;

    assert!(matches!(res, Err(NetError::VerificationFailed(_, _, _))));
    assert!(!dest_file.exists(), "corrupted file must not be installed");
    let part_file = PathBuf::from(format!("{}.part", dest_file.display()));
    assert!(
        !part_file.exists(),
        ".part file must be cleaned up on mismatch"
    );
}

#[tokio::test]
async fn test_download_oversize_aborts() {
    let temp_srv = TempDir::new().expect("srv dir");
    let server = DevServerGuard::start(temp_srv.path());

    let test_data = vec![0x33u8; 8 * 1024];
    let artifact_path = temp_srv.path().join("oversize.bin");
    std::fs::write(&artifact_path, &test_data).expect("write artifact");

    let temp_dest = TempDir::new().expect("dest dir");
    let dest_file = temp_dest.path().join("oversize.bin");

    // dev-server ?test_mode=oversize appends extra 8KB beyond declared length
    let entry = ComponentManifestEntry {
        component: "oversize-comp".into(),
        version: "1.0.0".into(),
        platform: "windows-x86_64".into(),
        url: format!("{}/oversize.bin?test_mode=oversize", server.url),
        size_bytes: test_data.len() as u64,
        sha256: compute_sha256(&test_data),
        archive: "none".into(),
        mirrors: vec![],
        files: vec![],
        min_app_version: "0.1.0".into(),
        max_app_version: None,
        rollout_percent: 100,
    };

    let res = download_and_verify_component(&entry, &dest_file, |_| {}, None).await;

    assert!(matches!(res, Err(NetError::SizeExceeded { .. })));
    assert!(!dest_file.exists());
}

#[tokio::test]
async fn test_download_redirect_to_non_allowlisted_host_rejected() {
    let temp_srv = TempDir::new().expect("srv dir");
    let server = DevServerGuard::start(temp_srv.path());

    let temp_dest = TempDir::new().expect("dest dir");
    let dest_file = temp_dest.path().join("redirected.bin");

    // dev-server ?test_mode=redirect redirects to malicious host
    let entry = ComponentManifestEntry {
        component: "redirect-comp".into(),
        version: "1.0.0".into(),
        platform: "windows-x86_64".into(),
        url: format!(
            "{}/dummy.bin?test_mode=redirect&target=https://malicious-attacker.com/payload.exe",
            server.url
        ),
        size_bytes: 1024,
        sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
        archive: "none".into(),
        mirrors: vec![],
        files: vec![],
        min_app_version: "0.1.0".into(),
        max_app_version: None,
        rollout_percent: 100,
    };

    let res = download_and_verify_component(&entry, &dest_file, |_| {}, None).await;

    assert!(res.is_err());
    assert!(!dest_file.exists());
}

#[tokio::test]
async fn test_download_dropped_connection() {
    let temp_srv = TempDir::new().expect("srv dir");
    let server = DevServerGuard::start(temp_srv.path());

    let test_data = vec![0x88u8; 16 * 1024];
    let artifact_path = temp_srv.path().join("dropped.bin");
    std::fs::write(&artifact_path, &test_data).expect("write artifact");

    let temp_dest = TempDir::new().expect("dest dir");
    let dest_file = temp_dest.path().join("dropped.bin");

    // dev-server ?test_mode=drop abruptly destroys connection after 256 bytes
    let entry = ComponentManifestEntry {
        component: "dropped-comp".into(),
        version: "1.0.0".into(),
        platform: "windows-x86_64".into(),
        url: format!("{}/dropped.bin?test_mode=drop&after=256", server.url),
        size_bytes: test_data.len() as u64,
        sha256: compute_sha256(&test_data),
        archive: "none".into(),
        mirrors: vec![],
        files: vec![],
        min_app_version: "0.1.0".into(),
        max_app_version: None,
        rollout_percent: 100,
    };

    let res = download_and_verify_component(&entry, &dest_file, |_| {}, None).await;

    // After retries fail, it should return NetError::Offline or Io
    assert!(res.is_err(), "abrupt drop must result in error");
    assert!(!dest_file.exists());
}

#[tokio::test]
async fn test_download_cancellation_preserves_part() {
    let temp_srv = TempDir::new().expect("srv dir");
    let server = DevServerGuard::start(temp_srv.path());

    // 64 KB throttled payload
    let test_data = vec![0x77u8; 64 * 1024];
    let artifact_path = temp_srv.path().join("throttled.bin");
    std::fs::write(&artifact_path, &test_data).expect("write artifact");

    let temp_dest = TempDir::new().expect("dest dir");
    let dest_file = temp_dest.path().join("throttled.bin");

    let entry = ComponentManifestEntry {
        component: "throttled-comp".into(),
        version: "1.0.0".into(),
        platform: "windows-x86_64".into(),
        url: format!("{}/throttled.bin?test_mode=throttle&rate=1024", server.url),
        size_bytes: test_data.len() as u64,
        sha256: compute_sha256(&test_data),
        archive: "none".into(),
        mirrors: vec![],
        files: vec![],
        min_app_version: "0.1.0".into(),
        max_app_version: None,
        rollout_percent: 100,
    };

    let cancel_token = CancellationToken::new();
    let token_clone = cancel_token.clone();

    // Cancel after 150ms
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        token_clone.cancel();
    });

    let res = download_and_verify_component(&entry, &dest_file, |_| {}, Some(&cancel_token)).await;

    assert!(matches!(res, Err(NetError::Cancelled)));
    assert!(!dest_file.exists());

    let part_file = PathBuf::from(format!("{}.part", dest_file.display()));
    assert!(part_file.exists(), ".part file must remain for resumption");
}

#[tokio::test]
async fn test_download_resume_after_partial_transfer() {
    let temp_srv = TempDir::new().expect("srv dir");
    let server = DevServerGuard::start(temp_srv.path());

    let test_data = vec![0x99u8; 32 * 1024];
    let full_sha = compute_sha256(&test_data);
    let artifact_path = temp_srv.path().join("resumable.bin");
    std::fs::write(&artifact_path, &test_data).expect("write artifact");

    let temp_dest = TempDir::new().expect("dest dir");
    let dest_file = temp_dest.path().join("resumable.bin");

    // Pre-seed .part file with the first 16 KB (exactly half)
    let part_file = PathBuf::from(format!("{}.part", dest_file.display()));
    let meta_file = PathBuf::from(format!("{}.part.meta", dest_file.display()));
    std::fs::write(&part_file, &test_data[..16 * 1024]).expect("write partial");

    let stat = std::fs::metadata(&artifact_path).expect("stat");
    let etag = format!("\"{:x}-{:x}\"", stat.len(), 12345u64);
    let meta_json = serde_json::json!({
        "etag": etag,
        "bytes": 16 * 1024,
    });
    std::fs::write(&meta_file, meta_json.to_string()).expect("write meta");

    let entry = ComponentManifestEntry {
        component: "resumable-comp".into(),
        version: "1.0.0".into(),
        platform: "windows-x86_64".into(),
        url: format!("{}/resumable.bin", server.url),
        size_bytes: test_data.len() as u64,
        sha256: full_sha.clone(),
        archive: "none".into(),
        mirrors: vec![],
        files: vec![],
        min_app_version: "0.1.0".into(),
        max_app_version: None,
        rollout_percent: 100,
    };

    let res = download_and_verify_component(&entry, &dest_file, |_| {}, None).await;

    assert!(
        res.is_ok(),
        "resumed download must succeed: {:?}",
        res.err()
    );
    assert!(dest_file.exists());
    assert!(!part_file.exists());
    assert!(!meta_file.exists());

    let downloaded = std::fs::read(&dest_file).expect("read dest");
    assert_eq!(compute_sha256(&downloaded), full_sha);
}

#[tokio::test]
async fn test_concurrent_download_lock() {
    let temp_srv = TempDir::new().expect("srv dir");
    let server = DevServerGuard::start(temp_srv.path());

    let test_data = vec![0x55u8; 64 * 1024];
    let artifact_path = temp_srv.path().join("lock_test.bin");
    std::fs::write(&artifact_path, &test_data).expect("write artifact");

    let temp_dest = TempDir::new().expect("dest dir");
    let dest_file = temp_dest.path().join("lock_test.bin");

    let entry = ComponentManifestEntry {
        component: "locked-comp".into(),
        version: "1.0.0".into(),
        platform: "windows-x86_64".into(),
        url: format!("{}/lock_test.bin?test_mode=throttle&rate=1024", server.url),
        size_bytes: test_data.len() as u64,
        sha256: compute_sha256(&test_data),
        archive: "none".into(),
        mirrors: vec![],
        files: vec![],
        min_app_version: "0.1.0".into(),
        max_app_version: None,
        rollout_percent: 100,
    };

    let cancel_token = CancellationToken::new();
    let token_clone = cancel_token.clone();
    let entry_clone = entry.clone();
    let dest_clone = dest_file.clone();

    // Start first download in background
    let handle = tokio::spawn(async move {
        download_and_verify_component(&entry_clone, &dest_clone, |_| {}, Some(&token_clone)).await
    });

    // Give it 50ms to acquire lock
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Second download for same component must be rejected immediately
    let res2 = download_and_verify_component(&entry, &dest_file, |_| {}, None).await;
    assert!(matches!(res2, Err(NetError::AlreadyDownloading(_))));

    // Cancel first
    cancel_token.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn test_disk_space_preflight_check() {
    let temp_dest = TempDir::new().expect("dest dir");
    let dest_file = temp_dest.path().join("huge.bin");

    // Declare 100 Petabytes size
    let huge_bytes: u64 = 100 * 1024 * 1024 * 1024 * 1024 * 1024;
    let entry = ComponentManifestEntry {
        component: "huge-comp".into(),
        version: "1.0.0".into(),
        platform: "windows-x86_64".into(),
        url: "https://cdn.clipperhelper.com/huge.bin".into(),
        size_bytes: huge_bytes,
        sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
        archive: "none".into(),
        mirrors: vec![],
        files: vec![],
        min_app_version: "0.1.0".into(),
        max_app_version: None,
        rollout_percent: 100,
    };

    let res = download_and_verify_component(&entry, &dest_file, |_| {}, None).await;

    assert!(matches!(
        res,
        Err(NetError::DiskFull(_) | NetError::DiskLow(_))
    ));
}

#[tokio::test]
async fn test_fetch_and_verify_manifest_via_dev_server() {
    let temp_srv = TempDir::new().expect("srv dir");
    let server = DevServerGuard::start(temp_srv.path());

    // Generate dev Ed25519 keypair
    let signing_key = SigningKey::from_bytes(&[42u8; 32]);
    let verifying_key = signing_key.verifying_key();
    let pub_key_bytes = verifying_key.to_bytes();
    let pub_key_hex = hex::encode(pub_key_bytes);

    let manifest = ComponentManifest {
        schema: 1,
        sequence: 42,
        generated_at: "2026-11-24T12:00:00Z".into(),
        channel: "stable".into(),
        components: vec![ComponentManifestEntry {
            component: "whisper-cli".into(),
            version: "1.7.4".into(),
            platform: "windows-x86_64".into(),
            url:
                "https://github.com/ggerganov/whisper.cpp/releases/download/v1.7.4/whisper-cli.exe"
                    .into(),
            size_bytes: 123456,
            sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
            archive: "none".into(),
            mirrors: vec![],
            files: vec![],
            min_app_version: "0.1.0".into(),
            max_app_version: None,
            rollout_percent: 100,
        }],
    };

    let manifest_bytes = serde_json::to_vec_pretty(&manifest).expect("serialize manifest");
    let signature = signing_key.sign(&manifest_bytes);

    let sig_json = serde_json::json!({
        "kid": "dev-test-key",
        "signature": hex::encode(signature.to_bytes()),
    });
    let sig_bytes = serde_json::to_vec_pretty(&sig_json).expect("serialize sig");

    std::fs::write(temp_srv.path().join("manifest.json"), &manifest_bytes).expect("write manifest");
    std::fs::write(temp_srv.path().join("manifest.json.sig"), &sig_bytes).expect("write sig");

    let temp_db_dir = TempDir::new().expect("db dir");
    let db_path = temp_db_dir.path().join("clipper_test.db");
    let store = LocalStore::open(&db_path).expect("open store");

    let extra_keys = vec![ManifestPublicKey {
        kid: "dev-test-key",
        public_key_hex: Box::leak(pub_key_hex.into_boxed_str()),
    }];

    let manifest_url = format!("{}/manifest.json", server.url);
    let sig_url = format!("{}/manifest.json.sig", server.url);

    let accepted =
        fetch_and_verify_manifest(&manifest_url, &sig_url, "0.1.0", &store, Some(&extra_keys))
            .await
            .expect("fetch and verify must succeed");

    assert_eq!(accepted.sequence, 42);
    assert_eq!(accepted.components.len(), 1);
    assert_eq!(accepted.components[0].component, "whisper-cli");

    // Anti-rollback check: repeating with lower sequence must fail
    let stale_manifest = ComponentManifest {
        sequence: 41,
        ..manifest
    };
    let stale_bytes = serde_json::to_vec_pretty(&stale_manifest).expect("serialize");
    let stale_sig = signing_key.sign(&stale_bytes);
    let stale_sig_json = serde_json::json!({
        "kid": "dev-test-key",
        "signature": hex::encode(stale_sig.to_bytes()),
    });
    std::fs::write(temp_srv.path().join("stale.json"), &stale_bytes).expect("write");
    std::fs::write(
        temp_srv.path().join("stale.json.sig"),
        serde_json::to_vec(&stale_sig_json).unwrap(),
    )
    .expect("write");

    let stale_url = format!("{}/stale.json", server.url);
    let stale_sig_url = format!("{}/stale.json.sig", server.url);

    let stale_res = fetch_and_verify_manifest(
        &stale_url,
        &stale_sig_url,
        "0.1.0",
        &store,
        Some(&extra_keys),
    )
    .await;

    assert!(stale_res.is_err(), "stale sequence must be rejected");
}

#[tokio::test]
async fn test_dev_panel_full_flow_sequence() {
    use clipper_helper_desktop_lib::dev_commands::{
        dev_start_fixture_server, dev_stop_fixture_server, DEV_EXTRA_KEYS,
    };

    // Ensure server stopped initially
    let _ = dev_stop_fixture_server();

    // 1. Start server
    let start_res = dev_start_fixture_server().expect("start dev server");
    assert_eq!(start_res.state, "running");
    let server_url = start_res.url.expect("server url");

    // Temp store for manifest sequence tracking
    let temp_db_dir = TempDir::new().expect("db dir");
    let db_path = temp_db_dir.path().join("panel_test.db");
    let store = LocalStore::open(&db_path).expect("open store");

    // 2. Fetch manifest OK
    let manifest_url = format!("{}/manifest.json", server_url);
    let sig_url = format!("{}/manifest.json.sig", server_url);
    let manifest = fetch_and_verify_manifest(
        &manifest_url,
        &sig_url,
        "0.1.0",
        &store,
        Some(DEV_EXTRA_KEYS),
    )
    .await
    .expect("fetch and verify manifest must succeed");

    assert!(manifest.sequence >= 1);
    let tiny_entry = manifest
        .components
        .iter()
        .find(|c| c.component == "ggml-tiny.bin")
        .expect("ggml-tiny.bin must be present in dev fixtures");

    // 3. Download ggml-tiny.bin with progress
    let temp_dest = TempDir::new().expect("dest dir");
    let dest_file = temp_dest.path().join("ggml-tiny.bin");
    let progress_events = Arc::new(Mutex::new(Vec::<DownloadProgress>::new()));
    let p_clone = progress_events.clone();

    download_and_verify_component(
        tiny_entry,
        &dest_file,
        move |p| {
            p_clone.lock().unwrap().push(p);
        },
        None,
    )
    .await
    .expect("download and verify component must succeed");

    assert!(dest_file.exists(), "Downloaded file must exist");
    {
        let events = progress_events.lock().unwrap();
        assert!(!events.is_empty(), "Must emit progress events");
        let final_event = events.last().unwrap();
        assert_eq!(final_event.percent, 100.0);
        assert_eq!(final_event.stage, "download");
    }

    // 4. Tampered manifest refused (E_MANIFEST_INVALID)
    let tampered_manifest_url = format!("{}/manifest-tampered.json", server_url);
    let tampered_sig_url = format!("{}/manifest-tampered.json.sig", server_url);
    let tampered_res = fetch_and_verify_manifest(
        &tampered_manifest_url,
        &tampered_sig_url,
        "0.1.0",
        &store,
        Some(DEV_EXTRA_KEYS),
    )
    .await;

    assert!(tampered_res.is_err(), "Tampered manifest must be refused");
    let err = tampered_res.unwrap_err();
    assert_eq!(
        err.error_code(),
        "E_MANIFEST_INVALID",
        "Tampered manifest must produce E_MANIFEST_INVALID, got: {:?}",
        err
    );

    // 5. Server stopped
    let stop_res = dev_stop_fixture_server().expect("stop dev server");
    assert_eq!(stop_res.state, "stopped");

    // 6. Retry state (E_NET_OFFLINE, NOT refused)
    let offline_res = fetch_and_verify_manifest(
        &manifest_url,
        &sig_url,
        "0.1.0",
        &store,
        Some(DEV_EXTRA_KEYS),
    )
    .await;

    assert!(offline_res.is_err(), "Offline server must fail");
    let net_err = offline_res.unwrap_err();
    assert_eq!(
        net_err.error_code(),
        "E_NET_OFFLINE",
        "Offline server must produce E_NET_OFFLINE (neutral Retry state), got: {:?}",
        net_err
    );
}

#[tokio::test]
async fn test_fetch_manifest_wrong_key_unknown_kid_schema_and_stale() {
    let temp_srv = TempDir::new().expect("srv dir");
    let server = DevServerGuard::start(temp_srv.path());

    let trusted_key = SigningKey::from_bytes(&[51u8; 32]);
    let wrong_key = SigningKey::from_bytes(&[52u8; 32]);
    let pub_key_hex = hex::encode(trusted_key.verifying_key().to_bytes());
    let extra_keys = vec![ManifestPublicKey {
        kid: "trusted-kid",
        public_key_hex: Box::leak(pub_key_hex.into_boxed_str()),
    }];

    let valid_manifest = ComponentManifest {
        schema: 1,
        sequence: 10,
        generated_at: "2026-11-24T12:00:00Z".into(),
        channel: "stable".into(),
        components: vec![],
    };
    let manifest_bytes = serde_json::to_vec_pretty(&valid_manifest).unwrap();

    // 1. Wrong key (valid kid, signed by wrong key) -> E_MANIFEST_INVALID
    let wrong_sig = wrong_key.sign(&manifest_bytes);
    let wrong_sig_json = serde_json::json!({
        "kid": "trusted-kid",
        "signature": hex::encode(wrong_sig.to_bytes()),
    });
    std::fs::write(temp_srv.path().join("wrong_key.json"), &manifest_bytes).unwrap();
    std::fs::write(
        temp_srv.path().join("wrong_key.json.sig"),
        serde_json::to_vec(&wrong_sig_json).unwrap(),
    )
    .unwrap();

    let temp_db_dir = TempDir::new().expect("db dir");
    let store = LocalStore::open(&temp_db_dir.path().join("t.db")).unwrap();

    let err_wrong = fetch_and_verify_manifest(
        &format!("{}/wrong_key.json", server.url),
        &format!("{}/wrong_key.json.sig", server.url),
        "0.1.0",
        &store,
        Some(&extra_keys),
    )
    .await
    .unwrap_err();
    assert!(matches!(err_wrong, NetError::ManifestInvalid(_)));
    assert_eq!(err_wrong.error_code(), "E_MANIFEST_INVALID");

    // 2. Unknown kid -> E_MANIFEST_INVALID
    let good_sig = trusted_key.sign(&manifest_bytes);
    let unknown_kid_json = serde_json::json!({
        "kid": "unknown-kid-999",
        "signature": hex::encode(good_sig.to_bytes()),
    });
    std::fs::write(
        temp_srv.path().join("unknown_kid.json.sig"),
        serde_json::to_vec(&unknown_kid_json).unwrap(),
    )
    .unwrap();

    let err_unknown = fetch_and_verify_manifest(
        &format!("{}/wrong_key.json", server.url),
        &format!("{}/unknown_kid.json.sig", server.url),
        "0.1.0",
        &store,
        Some(&extra_keys),
    )
    .await
    .unwrap_err();
    assert!(matches!(err_unknown, NetError::ManifestInvalid(_)));
    assert_eq!(err_unknown.error_code(), "E_MANIFEST_INVALID");

    // 3. Schema mismatch -> E_MANIFEST_INVALID
    let bad_schema_manifest = ComponentManifest {
        schema: 99,
        sequence: 10,
        generated_at: "2026-11-24T12:00:00Z".into(),
        channel: "stable".into(),
        components: vec![],
    };
    let bad_schema_bytes = serde_json::to_vec_pretty(&bad_schema_manifest).unwrap();
    let bad_schema_sig = trusted_key.sign(&bad_schema_bytes);
    std::fs::write(temp_srv.path().join("bad_schema.json"), &bad_schema_bytes).unwrap();
    std::fs::write(
        temp_srv.path().join("bad_schema.json.sig"),
        serde_json::to_vec(&serde_json::json!({
            "kid": "trusted-kid",
            "signature": hex::encode(bad_schema_sig.to_bytes()),
        }))
        .unwrap(),
    )
    .unwrap();

    let err_schema = fetch_and_verify_manifest(
        &format!("{}/bad_schema.json", server.url),
        &format!("{}/bad_schema.json.sig", server.url),
        "0.1.0",
        &store,
        Some(&extra_keys),
    )
    .await
    .unwrap_err();
    assert!(matches!(err_schema, NetError::ManifestInvalid(_)));
    assert_eq!(err_schema.error_code(), "E_MANIFEST_INVALID");

    // 4. Sequence rollback -> E_MANIFEST_STALE
    std::fs::write(temp_srv.path().join("good.json"), &manifest_bytes).unwrap();
    std::fs::write(
        temp_srv.path().join("good.json.sig"),
        serde_json::to_vec(&serde_json::json!({
            "kid": "trusted-kid",
            "signature": hex::encode(good_sig.to_bytes()),
        }))
        .unwrap(),
    )
    .unwrap();
    fetch_and_verify_manifest(
        &format!("{}/good.json", server.url),
        &format!("{}/good.json.sig", server.url),
        "0.1.0",
        &store,
        Some(&extra_keys),
    )
    .await
    .expect("sequence 10 accepted");

    let older_manifest = ComponentManifest {
        sequence: 9,
        ..valid_manifest
    };
    let older_bytes = serde_json::to_vec_pretty(&older_manifest).unwrap();
    let older_sig = trusted_key.sign(&older_bytes);
    std::fs::write(temp_srv.path().join("older.json"), &older_bytes).unwrap();
    std::fs::write(
        temp_srv.path().join("older.json.sig"),
        serde_json::to_vec(&serde_json::json!({
            "kid": "trusted-kid",
            "signature": hex::encode(older_sig.to_bytes()),
        }))
        .unwrap(),
    )
    .unwrap();

    let err_stale = fetch_and_verify_manifest(
        &format!("{}/older.json", server.url),
        &format!("{}/older.json.sig", server.url),
        "0.1.0",
        &store,
        Some(&extra_keys),
    )
    .await
    .unwrap_err();
    assert!(matches!(err_stale, NetError::ManifestStale { .. }));
    assert_eq!(err_stale.error_code(), "E_MANIFEST_STALE");
}
