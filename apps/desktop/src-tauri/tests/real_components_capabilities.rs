//! Integration tests running the REAL pinned binaries through the production `SidecarSupervisor`
//! and `installer` pipelines.
//!
//! Set `CLIPPER_REQUIRE_REAL_COMPONENTS=1` (as in CI after `fetch-pinned-components`) to fail
//! loudly if `tools/dev-fixtures/real/manifest.json` is missing.

use clipper_helper_desktop_lib::archive::ArchiveLimits;
use clipper_helper_desktop_lib::capabilities::{
    verify_installed_component_capabilities, CapabilityStatus, VerifyMode,
};
use clipper_helper_desktop_lib::installer::{
    install_verified_artifact_with_options, reconcile_startup, rollback_component,
    DefaultCapCheckHook, InUseRegistry, InstallError,
};
use clipper_helper_desktop_lib::manifest::ComponentManifest;
use clipper_helper_desktop_lib::security;
use clipper_helper_desktop_lib::sidecars::{
    create_yt_dlp_spec, SidecarEnv, SidecarError, SidecarSpec, SidecarSupervisor,
};
use clipper_helper_desktop_lib::store::LocalStore;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn real_fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tools/dev-fixtures/real")
}

fn load_real_manifest_or_skip(file_name: &str) -> Option<(PathBuf, ComponentManifest)> {
    let dir = real_fixtures_dir();
    let manifest_path = dir.join(file_name);
    let required = std::env::var("CLIPPER_REQUIRE_REAL_COMPONENTS").as_deref() == Ok("1");

    if !manifest_path.is_file() {
        if required {
            panic!(
                "CLIPPER_REQUIRE_REAL_COMPONENTS=1 but missing '{}'. Run `node tools/src/fetch-pinned-components.js` first.",
                manifest_path.display()
            );
        }
        eprintln!(
            "Skipping real binary test: '{}' not found (run `pnpm --filter @clipper-helper/tools fetch-pinned-components` to enable locally).",
            manifest_path.display()
        );
        return None;
    }

    let raw = fs::read_to_string(&manifest_path).expect("read real manifest");
    let manifest: ComponentManifest = serde_json::from_str(&raw).expect("parse real manifest");
    Some((dir, manifest))
}

fn local_payload_from_real_url(fixtures_dir: &Path, url: &str) -> PathBuf {
    let marker = "/real/";
    let rel = url
        .split_once(marker)
        .map(|(_, r)| r)
        .unwrap_or_else(|| panic!("Unexpected fixture URL without /real/: {}", url));
    let p = fixtures_dir.join(rel.replace('/', "\\"));
    assert!(
        p.is_file(),
        "Real component payload file missing on disk: {}",
        p.display()
    );
    p
}

fn setup_workspace() -> (TempDir, PathBuf, LocalStore, InUseRegistry) {
    let tmp = TempDir::new().expect("tempdir");
    let ws = security::ensure_workspace_with_root(tmp.path()).expect("init workspace");
    let store = LocalStore::open(&ws.join("clipper.db")).expect("open sqlite");
    let in_use = InUseRegistry::new();
    (tmp, ws, store, in_use)
}

#[tokio::test]
async fn test_real_yt_dlp_missing_temp_fails_and_sidecar_env_passes() {
    let Some((fixtures_dir, manifest)) = load_real_manifest_or_skip("manifest.json") else {
        return;
    };
    let yt_entry = manifest
        .components
        .iter()
        .find(|c| c.component == "yt-dlp")
        .expect("yt-dlp entry in manifest.json");
    let yt_payload = local_payload_from_real_url(&fixtures_dir, &yt_entry.url);

    let (_tmp, ws, store, in_use) = setup_workspace();
    let supervisor = SidecarSupervisor::new();

    // 1. Reproduce PyInstaller failure via PRODUCTION supervisor when TEMP/TMP are missing from SidecarSpec.
    let no_temp_spec = SidecarSpec::new(&yt_payload, ["--version"]);
    let repro_err = supervisor
        .run_to_completion(&no_temp_spec, None)
        .await
        .expect_err("yt-dlp.exe onefile MUST fail under env_clear() when TEMP/TMP are missing");
    match repro_err {
        SidecarError::ExecutionFailed {
            exit_code,
            stderr_tail,
        } => {
            assert_eq!(exit_code, Some(-1));
            assert!(
                stderr_tail.contains("Could not create temporary directory")
                    || stderr_tail.contains("PYI-"),
                "Expected PyInstaller temp dir failure in stderr, got: {}",
                stderr_tail
            );
        }
        other => panic!("Expected SidecarError::ExecutionFailed, got {:?}", other),
    }

    // 2. Install through production pipeline (atomic staging -> rename -> Strict supervisor capability check).
    let no_override: Option<DefaultCapCheckHook> = None;
    let outcome = install_verified_artifact_with_options(
        &ws,
        &store,
        &in_use,
        yt_entry,
        manifest.sequence,
        &yt_payload,
        &ArchiveLimits::default(),
        None,
        no_override,
    )
    .await
    .expect("real yt-dlp install and capability check must pass with SidecarEnv");

    let cap = outcome
        .capability_report
        .expect("yt-dlp capability report persisted");
    assert_eq!(cap.status, CapabilityStatus::Pass);
    assert_eq!(
        cap.reported_version.as_deref(),
        Some(yt_entry.version.as_str()),
        "yt-dlp --version must match manifest version"
    );

    // 3. Verify SidecarEnv::for_component + create_yt_dlp_spec isolate TEMP/TMP/USERPROFILE/cache-dir.
    let env_spec = SidecarEnv::for_component("yt-dlp", &ws);
    assert!(env_spec.temp_dir.is_dir());
    assert!(env_spec.profile_dir.as_ref().unwrap().is_dir());
    let installed_exe = outcome.install_path.join("yt-dlp.exe");
    let prod_spec = create_yt_dlp_spec(&installed_exe, ["--version"], &ws);
    assert!(prod_spec.args.contains(&"--ignore-config".to_string()));
    assert!(prod_spec.args.contains(&"--cache-dir".to_string()));
    let prod_out = supervisor
        .run_to_completion(&prod_spec, None)
        .await
        .expect("create_yt_dlp_spec --version must succeed");
    assert_eq!(
        String::from_utf8_lossy(&prod_out.stdout).trim(),
        yt_entry.version
    );

    // Also run capability verification explicitly via SidecarSupervisor.
    let verified = verify_installed_component_capabilities(
        "yt-dlp",
        &outcome.install_path,
        &yt_entry.version,
        VerifyMode::Strict,
        &supervisor,
        &ws,
    )
    .await
    .expect("direct capability check must pass");
    assert_eq!(
        verified.reported_version.as_deref(),
        Some(yt_entry.version.as_str())
    );

    // 4. Verify stale temp directory cleanup at startup reconciliation.
    let stale_mei = ws.join("cache").join("temp").join("_MEI_stale_999");
    fs::create_dir_all(&stale_mei).unwrap();
    fs::write(stale_mei.join("leftover.dll"), b"stale").unwrap();
    reconcile_startup(&ws, &store).expect("startup reconcile");
    assert!(
        !stale_mei.exists(),
        "stale temp dir must be cleaned at startup"
    );
}

#[tokio::test]
async fn test_real_all_components_capabilities_and_gpl_refusal_via_supervisor() {
    let Some((fixtures_dir, manifest)) = load_real_manifest_or_skip("manifest.json") else {
        return;
    };

    let (_tmp, ws, store, in_use) = setup_workspace();

    // Install & verify ALL pinned binary components (`ffmpeg`, `yt-dlp`, `qjs`, `whisper-cli`)
    // through the production installer + SidecarSupervisor capability verifier.
    let mut lgpl_ffmpeg_ver: Option<String> = None;
    let mut qjs_v1: Option<String> = None;

    for entry in manifest
        .components
        .iter()
        .filter(|c| !security::is_allowed_model(&c.component))
    {
        let payload = local_payload_from_real_url(&fixtures_dir, &entry.url);
        let no_override: Option<DefaultCapCheckHook> = None;
        let outcome = install_verified_artifact_with_options(
            &ws,
            &store,
            &in_use,
            entry,
            manifest.sequence,
            &payload,
            &ArchiveLimits::default(),
            None,
            no_override,
        )
        .await
        .unwrap_or_else(|e| {
            panic!(
                "Real component {} v{} failed install/capability check: {}",
                entry.component, entry.version, e
            )
        });

        let report = outcome
            .capability_report
            .unwrap_or_else(|| panic!("Missing capability report for {}", entry.component));
        assert_eq!(
            report.status,
            CapabilityStatus::Pass,
            "Capability report did not pass for {}",
            entry.component
        );

        if entry.component == "ffmpeg" {
            let ff = report.ffmpeg.expect("ffmpeg capability details");
            assert!(!ff.has_gpl);
            assert!(!ff.has_nonfree);
            assert!(ff.gpl_filters_present.is_empty());
            assert!(ff.missing_encoders.is_empty());
            assert!(ff.missing_decoders.is_empty());
            assert!(ff.missing_filters.is_empty());
            assert!(ff.missing_protocols.is_empty());
            assert!(ff.missing_formats.is_empty());
            lgpl_ffmpeg_ver = Some(entry.version.clone());
        } else if entry.component == "qjs" {
            qjs_v1 = Some(entry.version.clone());
        }
    }

    // Prove real GPL FFmpeg refusal -> quarantined -> rolled back to previous LGPL version.
    if let Some((_, gpl_manifest)) = load_real_manifest_or_skip("manifest-gpl-ffmpeg.json") {
        if let Some(gpl_entry) = gpl_manifest
            .components
            .iter()
            .find(|c| c.component == "ffmpeg")
        {
            let gpl_payload = local_payload_from_real_url(&fixtures_dir, &gpl_entry.url);
            let no_override: Option<DefaultCapCheckHook> = None;
            let err = install_verified_artifact_with_options(
                &ws,
                &store,
                &in_use,
                gpl_entry,
                gpl_manifest.sequence,
                &gpl_payload,
                &ArchiveLimits::default(),
                None,
                no_override,
            )
            .await
            .expect_err("Real GPL FFmpeg MUST be refused under Strict verification");

            match err {
                InstallError::CapabilityFailed {
                    component,
                    version,
                    rolled_back_to,
                    report,
                    ..
                } => {
                    assert_eq!(component, "ffmpeg");
                    assert_eq!(version, gpl_entry.version);
                    assert_eq!(rolled_back_to, lgpl_ffmpeg_ver);
                    let rep = report.expect("GPL failure report");
                    assert!(rep.ffmpeg.as_ref().is_some_and(|f| f.has_gpl));
                }
                other => panic!("Expected InstallError::CapabilityFailed, got {:?}", other),
            }

            let cur = store
                .get_current_component("ffmpeg")
                .unwrap()
                .expect("current ffmpeg restored");
            assert_eq!(Some(cur.version), lgpl_ffmpeg_ver);
            let qrow = store
                .get_component_version("ffmpeg", &gpl_entry.version)
                .unwrap()
                .expect("quarantined GPL row");
            assert_eq!(qrow.status, "quarantined");
        }
    }

    // Prove real two-version install + rollback + roll-forward via manifest-v2.json (`qjs`).
    if let (Some(v1), Some((_, v2_manifest))) =
        (qjs_v1, load_real_manifest_or_skip("manifest-v2.json"))
    {
        if let Some(qjs_v2) = v2_manifest.components.iter().find(|c| c.component == "qjs") {
            let v2_payload = local_payload_from_real_url(&fixtures_dir, &qjs_v2.url);
            let no_override: Option<DefaultCapCheckHook> = None;
            install_verified_artifact_with_options(
                &ws,
                &store,
                &in_use,
                qjs_v2,
                v2_manifest.sequence,
                &v2_payload,
                &ArchiveLimits::default(),
                None,
                no_override,
            )
            .await
            .expect("v2 qjs install");

            let rb = rollback_component(&ws, &store, "qjs", None).expect("rollback to v1");
            assert_eq!(rb, v1);
            assert_eq!(
                store.get_current_component("qjs").unwrap().unwrap().version,
                v1
            );

            let rf = rollback_component(&ws, &store, "qjs", Some(&qjs_v2.version))
                .expect("roll forward");
            assert_eq!(rf, qjs_v2.version);
            assert_eq!(
                store.get_current_component("qjs").unwrap().unwrap().version,
                qjs_v2.version
            );
        }
    }
}
