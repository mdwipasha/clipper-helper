pub mod archive;
pub mod capabilities;
pub mod commands;
pub mod crash;
#[cfg(debug_assertions)]
pub mod dev_commands;
pub mod eta;
pub mod hardware;
pub mod installer;
pub mod jobs;
pub mod license;
pub mod logging;
pub mod manifest;
pub mod net;
pub mod render;
pub mod resolver;
pub mod security;
pub mod sidecars;
pub mod store;
pub mod updater;

use security::RedactionLayer;
use std::path::PathBuf;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Managed state holding the verified canonical workspace root.
#[derive(Clone, Debug)]
pub struct WorkspaceState {
    pub root: PathBuf,
}

pub fn init_logging() {
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_thread_ids(false);

    let _ = tracing_subscriber::registry()
        .with(fmt_layer)
        .with(RedactionLayer)
        .with(logging::RotatingFileLogLayer)
        .try_init();
}

#[cfg(debug_assertions)]
pub fn create_specta_builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new().commands(tauri_specta::collect_commands![
        commands::get_app_info,
        commands::get_workspace_status,
        dev_commands::dev_get_system_info,
        dev_commands::dev_reset_workspace,
        dev_commands::dev_run_fake_sidecar,
        dev_commands::dev_start_fixture_server,
        dev_commands::dev_get_fixture_server_status,
        dev_commands::dev_stop_fixture_server,
        dev_commands::dev_fetch_manifest,
        dev_commands::dev_download_component,
        dev_commands::dev_install_component,
        dev_commands::dev_get_component_diagnostics,
        dev_commands::dev_rollback_component,
        dev_commands::dev_gc_component,
        dev_commands::dev_verify_component,
        dev_commands::dev_retry_quarantined_component,
        dev_commands::dev_repair_component,
        dev_commands::dev_reset_manifest_sequence_floor,
        dev_commands::dev_set_custom_ffmpeg,
        dev_commands::dev_clear_custom_ffmpeg,
    ])
}

#[cfg(debug_assertions)]
pub fn export_typescript_bindings() -> Result<(), String> {
    let specta_builder = create_specta_builder();
    let bindings_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/ipc/bindings.ts");
    specta_builder
        .export(
            specta_typescript::Typescript::default().header("// @ts-nocheck\n"),
            bindings_path,
        )
        .map_err(|e| format!("Failed to export TypeScript bindings: {}", e))
}

#[cfg(not(debug_assertions))]
pub fn create_specta_builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new().commands(tauri_specta::collect_commands![
        commands::get_app_info,
        commands::get_workspace_status,
    ])
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(debug_assertions)]
    if std::env::args().any(|arg| arg == "--export-bindings") {
        export_typescript_bindings().expect("Failed to export TypeScript bindings");
        return;
    }

    init_logging();
    tracing::info!("Starting ClipperHelper Core");

    let specta_builder = create_specta_builder();

    #[cfg(debug_assertions)]
    {
        let bindings_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/ipc/bindings.ts");
        specta_builder
            .export(
                specta_typescript::Typescript::default().header("// @ts-nocheck\n"),
                bindings_path,
            )
            .expect("Failed to export TypeScript bindings");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(specta_builder.invoke_handler())
        .setup(move |app| {
            use tauri::Manager;
            specta_builder.mount_events(app);

            // Startup initialization: ensure workspace directory structure and initialize SQLite store
            let init_res: Result<
                (
                    PathBuf,
                    store::LocalStore,
                    installer::ReconciliationReport,
                ),
                security::SecurityError,
            > = (|| {
                let root = security::ensure_workspace()?;
                let db_path = root.join("clipper.db");
                let store = match store::LocalStore::open(&db_path) {
                    Ok(s) => s,
                    Err(store::StoreError::CorruptRecovered(name)) => {
                        tracing::warn!(
                            "Recovered corrupt SQLite DB ({}); reopening fresh DB for rebuild",
                            name
                        );
                        store::LocalStore::open(&db_path).map_err(|e| {
                            security::SecurityError::RootResolutionFailed(format!(
                                "Failed to reopen recovered store: {}",
                                e
                            ))
                        })?
                    }
                    Err(e) => {
                        return Err(security::SecurityError::RootResolutionFailed(format!(
                            "Failed to open store: {}",
                            e
                        )));
                    }
                };
                let rec = installer::reconcile_startup(&root, &store).map_err(|e| {
                    security::SecurityError::RootResolutionFailed(format!(
                        "Startup component reconciliation failed: {}",
                        e
                    ))
                })?;
                Ok((root, store, rec))
            })();

            let in_use_registry = installer::InUseRegistry::new();
            app.manage(in_use_registry);

            match init_res {
                Ok((root, store, rec)) => {
                    let logs_dir = root.join("logs");
                    if let Err(e) = logging::activate_file_logging(&logs_dir) {
                        tracing::warn!("Failed to activate rotating file logs: {}", e);
                    }
                    let app_ver = env!("CARGO_PKG_VERSION").to_string();
                    crash::install_panic_hook(logs_dir.clone(), app_ver.clone());
                    let crash_status = crash::begin_session(&logs_dir, &app_ver);
                    if crash_status.unclean_shutdown_detected {
                        tracing::warn!("Previous session ended unexpectedly; crash recovery banner active");
                        let detail_msg = crash_status
                            .crash_record
                            .as_ref()
                            .map(|c| c.message_redacted.clone())
                            .unwrap_or_else(|| "Unclean shutdown detected at startup".to_string());
                        let _ = store.record_error_event(store::NewErrorEvent {
                            code: "E_UNKNOWN".to_string(),
                            stage: "app_lifecycle".to_string(),
                            job_id: None,
                            message_key: "err.E_UNKNOWN.body".to_string(),
                            detail: detail_msg,
                            stderr_tail: crash_status
                                .crash_record
                                .as_ref()
                                .map(|c| c.backtrace_redacted.clone())
                                .unwrap_or_default(),
                            app_version: app_ver,
                            component_versions_json: "{}".to_string(),
                        });
                    }
                    let crash_state = std::sync::Mutex::new(crash_status);
                    tracing::info!(
                        "Workspace initialized at {} (reconciled: rebuilt={}, staging_cleaned={}, deferred_retried={})",
                        root.display(),
                        rec.rebuilt_from_receipts,
                        rec.orphan_staging_removed,
                        rec.deferred_deletes_succeeded
                    );
                    app.manage(WorkspaceState { root: root.clone() });
                    app.manage(store);
                    app.manage(rec);
                    app.manage(crash_state);
                    app.manage(commands::WorkspaceStatus {
                        is_initialized: true,
                        root: Some(root.to_string_lossy().to_string()),
                        error_code: None,
                        error_message: None,
                    });
                }
                Err(err) => {
                    tracing::error!("Workspace initialization failed: {}", err);
                    let err_code = err.error_code().to_string();
                    let err_msg = err.to_string();
                    app.manage(commands::WorkspaceStatus {
                        is_initialized: false,
                        root: None,
                        error_code: Some(err_code),
                        error_message: Some(err_msg),
                    });
                }
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::Exit = event {
                use tauri::Manager;
                if let Some(ws) = app_handle.try_state::<WorkspaceState>() {
                    crash::mark_clean_shutdown(&ws.root.join("logs"));
                }
                logging::flush_file_logging();
            }
        });
}
