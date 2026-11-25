pub mod archive;
pub mod capabilities;
pub mod commands;
#[cfg(debug_assertions)]
pub mod dev_commands;
pub mod eta;
pub mod hardware;
pub mod installer;
pub mod jobs;
pub mod license;
pub mod manifest;
pub mod net;
pub mod render;
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
    ])
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
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
