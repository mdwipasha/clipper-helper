use std::fs;

use clipper_helper_desktop_lib::security::{self, ensure_workspace_with_root, SecurityError};
use clipper_helper_desktop_lib::store::LocalStore;

#[cfg(debug_assertions)]
#[test]
fn test_startup_from_non_existent_root_creates_workspace_and_initializes_db() {
    let unique_id = format!("test_ws_startup_{}", std::process::id());
    let test_dir = std::env::temp_dir().join(&unique_id);
    let _ = fs::remove_dir_all(&test_dir);

    assert!(
        !test_dir.exists(),
        "Target directory must not exist prior to test"
    );

    // Set environment override for test
    std::env::set_var("CLIPPER_WORKSPACE_ROOT", &test_dir);

    // Step 1: ensure_workspace creates root and subdirectories
    let ensured_root = security::ensure_workspace().expect("ensure_workspace failed");
    assert!(ensured_root.exists(), "Ensured root must exist");

    for sub in &["bin", "models", "cache", "projects", "logs"] {
        let p = ensured_root.join(sub);
        assert!(p.is_dir(), "Expected subdir {:?} to be a directory", p);
    }

    // Step 2: Open LocalStore at clipper.db and verify user_version
    let db_path = ensured_root.join("clipper.db");
    assert!(!db_path.exists(), "DB file should not exist yet");

    let store = LocalStore::open(&db_path).expect("failed to open LocalStore");
    assert!(db_path.exists(), "DB file must be created on open");

    let version = store
        .schema_version()
        .expect("failed to read schema version");
    assert_eq!(
        version,
        clipper_helper_desktop_lib::store::CURRENT_SCHEMA_VERSION,
        "Schema version must be migrated to current version"
    );

    // Clean up
    std::env::remove_var("CLIPPER_WORKSPACE_ROOT");
    let _ = fs::remove_dir_all(&test_dir);
}

#[test]
fn test_startup_with_denied_location_returns_typed_error() {
    let unique_id = format!("test_ws_denied_{}.tmp", std::process::id());
    let temp_file = std::env::temp_dir().join(&unique_id);
    fs::write(&temp_file, "blocking file").expect("write temp file");

    let res = ensure_workspace_with_root(&temp_file);
    assert!(res.is_err(), "Must fail when root is a file");

    match res.unwrap_err() {
        SecurityError::RootResolutionFailed(msg) => {
            assert!(
                msg.contains("is a file, not a directory"),
                "Expected file error, got: {}",
                msg
            );
        }
        other => panic!("Expected RootResolutionFailed, got: {:?}", other),
    }

    let _ = fs::remove_file(&temp_file);
}

#[test]
fn test_junction_escape_outside_workspace_refused() {
    let unique_id = format!("test_ws_junc_escape_{}", std::process::id());
    let base_dir = std::env::temp_dir().join(&unique_id);
    let ws_root = base_dir.join("workspace");
    let outside_dir = base_dir.join("outside");

    let _ = fs::remove_dir_all(&base_dir);
    fs::create_dir_all(&ws_root).expect("create ws_root");
    fs::create_dir_all(&outside_dir).expect("create outside_dir");

    // On Windows, create a directory junction pointing outside
    #[cfg(windows)]
    {
        let junc_link = ws_root.join("projects");
        let status = std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                &junc_link.to_string_lossy(),
                &outside_dir.to_string_lossy(),
            ])
            .output();

        if let Ok(out) = status {
            if out.status.success() {
                // Ensure workspace should detect that the subdirectory resolves outside root
                let res = ensure_workspace_with_root(&ws_root);
                assert!(
                    res.is_err(),
                    "Junction pointing outside workspace must be refused"
                );
                assert!(matches!(res.unwrap_err(), SecurityError::BoundaryEscape(_)));

                // Also verify resolve_workspace_path_with_root detects escape
                let path_res =
                    security::resolve_workspace_path_with_root(&ws_root, "projects/secret.txt");
                assert!(
                    path_res.is_err(),
                    "Path through escaping junction must be refused"
                );
                assert!(matches!(
                    path_res.unwrap_err(),
                    SecurityError::BoundaryEscape(_)
                ));
            }
        }
    }

    let _ = fs::remove_dir_all(&base_dir);
}

#[test]
fn test_startup_explicit_root_creates_workspace_and_initializes_db() {
    let dir = tempfile::TempDir::new().expect("create temp dir");
    let test_dir = dir.path().join("workspace");

    let ensured_root = ensure_workspace_with_root(&test_dir).expect("ensure_workspace_with_root");
    assert!(ensured_root.exists());
    for sub in &["bin", "models", "cache", "projects", "logs"] {
        assert!(ensured_root.join(sub).is_dir());
    }

    let db_path = ensured_root.join("clipper.db");
    let store = LocalStore::open(&db_path).expect("failed to open LocalStore");
    assert!(db_path.exists());
    assert!(store.schema_version().expect("schema version") >= 1);
}

#[test]
fn test_node_discovery_exists_only_under_debug_assertions() {
    let lib_src = include_str!("../src/lib.rs");
    assert!(
        lib_src.contains("#[cfg(debug_assertions)]\r\npub mod dev_commands;")
            || lib_src.contains("#[cfg(debug_assertions)]\npub mod dev_commands;"),
        "dev_commands module must be strictly gated under #[cfg(debug_assertions)]"
    );

    let dev_src = include_str!("../src/dev_commands.rs");
    assert!(
        dev_src.contains("#![cfg(debug_assertions)]"),
        "dev_commands.rs must enforce #![cfg(debug_assertions)] at crate module level"
    );

    let prod_modules: &[(&str, &str)] = &[
        ("archive.rs", include_str!("../src/archive.rs")),
        ("capabilities.rs", include_str!("../src/capabilities.rs")),
        ("commands.rs", include_str!("../src/commands.rs")),
        ("installer.rs", include_str!("../src/installer.rs")),
        ("manifest.rs", include_str!("../src/manifest.rs")),
        ("net.rs", include_str!("../src/net.rs")),
        ("resolver.rs", include_str!("../src/resolver.rs")),
        ("security.rs", include_str!("../src/security.rs")),
        ("sidecars.rs", include_str!("../src/sidecars.rs")),
        ("store.rs", include_str!("../src/store.rs")),
        ("updater.rs", include_str!("../src/updater.rs")),
    ];

    for (name, src) in prod_modules {
        for forbidden in [
            "find_node_executable",
            "CLIPPER_DEV_NODE_PATH",
            "fnm_multishells",
            "NVM_HOME",
            "NVM_SYMLINK",
            "where.exe",
        ] {
            assert!(
                !src.contains(forbidden),
                "Production module {} must not contain Node discovery reference '{}'",
                name,
                forbidden
            );
        }
    }
}
