use std::fs;

use clipper_helper_desktop_lib::security::{self, ensure_workspace_with_root, SecurityError};
use clipper_helper_desktop_lib::store::LocalStore;

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
    assert_eq!(version, 1, "Schema version must be migrated to version 1");

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
