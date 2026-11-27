pub mod archive;
pub mod capabilities;
pub mod commands;
pub mod crash;
#[cfg(debug_assertions)]
pub mod dev_commands;
pub mod diagnostics;
pub mod encoder;
pub mod eta;
pub mod hardware;
pub mod installer;
pub mod jobs;
pub mod license;
pub mod logging;
pub mod manifest;
pub mod net;
pub mod probe;
pub mod render;
pub mod resolver;
pub mod security;
pub mod sidecars;
pub mod speech;
pub mod store;
pub mod updater;
pub mod url_intake;

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
        .with(logging::build_runtime_log_targets())
        .with(fmt_layer)
        .with(RedactionLayer)
        .with(logging::RotatingFileLogLayer)
        .try_init();
}

#[cfg(debug_assertions)]
pub fn create_specta_builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new()
        .events(tauri_specta::collect_events![
            commands::ComponentInstallProgressEvent,
            commands::ProbeProgressEvent
        ])
        .commands(tauri_specta::collect_commands![
            commands::get_app_info,
            commands::get_workspace_status,
            commands::get_components_overview,
            commands::fetch_components_manifest,
            commands::install_component,
            commands::install_all_required_components,
            commands::repair_component,
            commands::retry_quarantined_component,
            commands::verify_component,
            commands::select_custom_data_root,
            commands::use_default_data_root,
            commands::get_settings_overview,
            commands::update_settings,
            commands::get_crash_recovery_status,
            commands::dismiss_crash_recovery,
            commands::list_error_events,
            commands::record_webview_error,
            commands::generate_diagnostics_report,
            commands::save_diagnostics_report_file,
            commands::probe_youtube_url,
            commands::cancel_youtube_probe,
            commands::list_recent_projects,
            commands::get_project,
            commands::update_project_range,
            commands::get_preview_config,
            commands::record_preview_event,
            commands::delete_project,
            commands::open_logs_folder,
            commands::open_video_in_browser,
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
    let prod_builder = tauri_specta::Builder::<tauri::Wry>::new()
        .events(tauri_specta::collect_events![
            commands::ComponentInstallProgressEvent,
            commands::ProbeProgressEvent
        ])
        .commands(tauri_specta::collect_commands![
            commands::get_app_info,
            commands::get_workspace_status,
            commands::get_components_overview,
            commands::fetch_components_manifest,
            commands::install_component,
            commands::install_all_required_components,
            commands::repair_component,
            commands::retry_quarantined_component,
            commands::verify_component,
            commands::select_custom_data_root,
            commands::use_default_data_root,
            commands::get_settings_overview,
            commands::update_settings,
            commands::get_crash_recovery_status,
            commands::dismiss_crash_recovery,
            commands::list_error_events,
            commands::record_webview_error,
            commands::generate_diagnostics_report,
            commands::save_diagnostics_report_file,
            commands::probe_youtube_url,
            commands::cancel_youtube_probe,
            commands::list_recent_projects,
            commands::get_project,
            commands::update_project_range,
            commands::get_preview_config,
            commands::record_preview_event,
            commands::delete_project,
            commands::open_logs_folder,
            commands::open_video_in_browser,
        ]);
    let bindings_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/ipc/bindings.ts");
    prod_builder
        .export(
            specta_typescript::Typescript::default().header("// @ts-nocheck\n"),
            bindings_path,
        )
        .map_err(|e| format!("Failed to export production TypeScript bindings: {}", e))?;

    let dev_builder =
        tauri_specta::Builder::<tauri::Wry>::new().commands(tauri_specta::collect_commands![
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
        ]);
    let dev_bindings_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/ipc/bindings.dev.ts");
    dev_builder
        .export(
            specta_typescript::Typescript::default().header("// @ts-nocheck\n"),
            dev_bindings_path,
        )
        .map_err(|e| format!("Failed to export dev TypeScript bindings: {}", e))
}

#[cfg(not(debug_assertions))]
pub fn create_specta_builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new()
        .events(tauri_specta::collect_events![
            commands::ComponentInstallProgressEvent,
            commands::ProbeProgressEvent
        ])
        .commands(tauri_specta::collect_commands![
            commands::get_app_info,
            commands::get_workspace_status,
            commands::get_components_overview,
            commands::fetch_components_manifest,
            commands::install_component,
            commands::install_all_required_components,
            commands::repair_component,
            commands::retry_quarantined_component,
            commands::verify_component,
            commands::select_custom_data_root,
            commands::use_default_data_root,
            commands::get_settings_overview,
            commands::update_settings,
            commands::get_crash_recovery_status,
            commands::dismiss_crash_recovery,
            commands::list_error_events,
            commands::record_webview_error,
            commands::generate_diagnostics_report,
            commands::save_diagnostics_report_file,
            commands::probe_youtube_url,
            commands::cancel_youtube_probe,
            commands::list_recent_projects,
            commands::get_project,
            commands::update_project_range,
            commands::get_preview_config,
            commands::record_preview_event,
            commands::delete_project,
            commands::open_logs_folder,
            commands::open_video_in_browser,
        ])
}

/// Runs workspace directory creation, SQLite open, reconciliation, logging activation,
/// panic hook registration, and session marker init, mounting all states onto `app`.
pub fn initialize_workspace_into_app(
    app: &tauri::AppHandle,
) -> Result<commands::WorkspaceStatus, security::SecurityError> {
    use tauri::Manager;

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

    let logs_dir = root.join("logs");
    if let Err(e) = logging::activate_file_logging(&logs_dir) {
        tracing::warn!("Failed to activate rotating file logs: {}", e);
    }
    let app_ver = env!("CARGO_PKG_VERSION").to_string();
    crash::install_panic_hook(logs_dir.clone(), app_ver.clone());
    crash::install_shutdown_handlers(logs_dir.clone(), app_ver.clone());
    let crash_status = crash::begin_session(&logs_dir, &app_ver);
    if crash_status.unclean_shutdown_detected {
        tracing::warn!("Previous session ended unexpectedly; crash recovery banner active");
        let detail_msg = crash_status
            .crash_record
            .as_ref()
            .map(|c| c.message_redacted.clone())
            .unwrap_or_else(|| "Unclean shutdown detected at startup".to_string());
        let _ = store.record_error_event(store::NewErrorEvent {
            code: "app_lifecycle/unclean_shutdown".to_string(),
            stage: "app_lifecycle".to_string(),
            job_id: None,
            message_key: "crash.bannerText".to_string(),
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
    let stale_probes_removed = probe::cleanup_stale_probe_files(&root, probe::PROBE_STALE_TTL);
    if stale_probes_removed > 0 {
        tracing::info!(
            "Removed {} stale cached probe JSON file(s) at startup",
            stale_probes_removed
        );
    }
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

    let ok_status = commands::WorkspaceStatus {
        is_initialized: true,
        root: Some(root.to_string_lossy().to_string()),
        error_code: None,
        error_message: None,
        configured_root_unreachable: false,
        unreachable_root_redacted: None,
    };
    if let Some(status_state) = app.try_state::<commands::WorkspaceStatusState>() {
        if let Ok(mut guard) = status_state.0.lock() {
            *guard = ok_status.clone();
        }
    } else {
        app.manage(commands::WorkspaceStatusState(std::sync::Mutex::new(
            ok_status.clone(),
        )));
    }
    Ok(ok_status)
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
        export_typescript_bindings().expect("Failed to export TypeScript bindings");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(specta_builder.invoke_handler())
        .setup(move |app| {
            use tauri::Manager;
            specta_builder.mount_events(app);

            let in_use_registry = installer::InUseRegistry::new();
            app.manage(in_use_registry);
            app.manage(commands::ManifestCacheState::default());
            app.manage(commands::WebViewErrorRateLimiter::default());
            app.manage(commands::ProbeCancelRegistry::default());

            if let Err(err) = initialize_workspace_into_app(app.handle()) {
                tracing::error!("Workspace initialization failed: {}", err);
                let (unreachable, unreachable_path) = match &err {
                    security::SecurityError::ConfiguredDataRootUnreachable {
                        target_redacted,
                        ..
                    } => (true, Some(target_redacted.clone())),
                    _ => (false, None),
                };
                let err_code = err.error_code().to_string();
                let err_msg = security::redact_sensitive_text(&err.to_string());
                app.manage(commands::WorkspaceStatusState(std::sync::Mutex::new(
                    commands::WorkspaceStatus {
                        is_initialized: false,
                        root: None,
                        error_code: Some(err_code),
                        error_message: Some(err_msg),
                        configured_root_unreachable: unreachable,
                        unreachable_root_redacted: unreachable_path,
                    },
                )));
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                use tauri::Manager;
                if let Some(ws) = app_handle.try_state::<WorkspaceState>() {
                    crash::mark_clean_shutdown(&ws.root.join("logs"));
                }
                logging::flush_file_logging();
            }
        });
}
