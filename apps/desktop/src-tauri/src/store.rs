//! SQLite database and workspace store (ARCHITECTURE §4, DATA_MODEL §9).
//!
//! Responsibilities:
//! - Manage local database tables: projects, jobs, artifacts, stage_stats,
//!   templates_local, settings, outbox, account_snapshot.
//! - Run forward-only SQLite schema migrations via `PRAGMA user_version`.
//! - Implement LRU cache eviction for media artifacts up to configured capacity.
//! - Handle database corruption by renaming the corrupt file and creating a fresh one.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current schema version. Bump this when adding new migrations.
pub const CURRENT_SCHEMA_VERSION: u32 = 3;

pub const ERROR_EVENT_RETENTION_DAYS: u64 = 30;
pub const ERROR_EVENT_MAX_ROWS: usize = 500;

#[derive(Error, Debug)]
pub enum StoreError {
    #[error("Database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("Database file is corrupt and was renamed: {0}")]
    CorruptRecovered(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Setting not found: {0}")]
    SettingNotFound(String),

    #[error("Migration failed at version {version}: {detail}")]
    MigrationFailed { version: u32, detail: String },

    #[error("Internal store state error: {0}")]
    Internal(String),
}

/// Persisted versioned component/model state row in SQLite `components` table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComponentRow {
    pub component: String,
    pub version: String,
    pub kind: String,
    pub install_rel_path: String,
    pub status: String,
    pub is_current: bool,
    pub is_previous: bool,
    pub manifest_sequence: u64,
    pub files_json: String,
    pub capability_report_json: Option<String>,
    pub quarantine_reason: Option<String>,
    pub installed_at: u64,
    pub updated_at: u64,
}

/// Persisted structured core error event row in SQLite `error_events` table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, specta::Type)]
pub struct ErrorEventRow {
    pub id: f64,
    pub ts: f64,
    pub code: String,
    pub stage: String,
    pub job_id: Option<String>,
    pub message_key: String,
    pub detail_redacted: String,
    pub stderr_tail_redacted: String,
    pub app_version: String,
    pub component_versions_json: String,
}

/// Input payload for recording a new typed error event.
#[derive(Debug, Clone)]
pub struct NewErrorEvent {
    pub code: String,
    pub stage: String,
    pub job_id: Option<String>,
    pub message_key: String,
    pub detail: String,
    pub stderr_tail: String,
    pub app_version: String,
    pub component_versions_json: String,
}

/// Non-sensitive summary of a recent job row for local diagnostics reports.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SafeJobSummary {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub stage: Option<String>,
    pub error_code: Option<String>,
    pub video_id: Option<String>,
    pub created_at: u64,
    pub finished_at: Option<u64>,
    pub error_detail_redacted: Option<String>,
}

/// Aggregated median timing for one `(stage, variant)` in `stage_stats`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StageStatMedian {
    pub device_profile: String,
    pub stage: String,
    pub variant: String,
    pub sample_count: usize,
    pub median_rate_units_per_sec: f64,
}

/// Queued directory/file deletion deferred because of a Windows file lock.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeferredDeleteRow {
    pub id: i64,
    pub path: String,
    pub created_at: u64,
    pub attempts: u32,
    pub last_error: Option<String>,
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Thread-safe local SQLite store.
///
/// Wraps a `rusqlite::Connection` behind `Arc<Mutex<_>>` for safe
/// concurrent access from Tauri command handlers and background jobs.
#[derive(Clone)]
pub struct LocalStore {
    conn: Arc<Mutex<Connection>>,
    path: PathBuf,
}

impl LocalStore {
    /// Open (or create) the SQLite database at `path`.
    ///
    /// If the file exists but is corrupt (not a valid SQLite database),
    /// it is renamed to `<path>.corrupt.<unix_ts>` and a fresh database
    /// is created in its place.
    ///
    /// Applies pragmas (WAL, foreign keys, busy timeout) and runs any
    /// pending forward-only migrations.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = match Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        ) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    "Failed to open database, attempting corruption recovery: {}",
                    e
                );
                return Self::recover_corrupt(path);
            }
        };

        // Verify the database is actually usable by running a quick integrity probe.
        // A file that exists but isn't SQLite will fail here.
        match conn.query_row("SELECT 1", [], |r| r.get::<_, i64>(0)) {
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(
                    "Database probe failed, attempting corruption recovery: {}",
                    e
                );
                drop(conn);
                return Self::recover_corrupt(path);
            }
        }

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            path: path.to_path_buf(),
        };
        store.apply_pragmas()?;
        store.migrate()?;
        Ok(store)
    }

    /// Rename the corrupt file and create a fresh database.
    fn recover_corrupt(path: &Path) -> Result<Self, StoreError> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let corrupt_name = format!(
            "{}.corrupt.{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            ts
        );
        let corrupt_path = path.with_file_name(&corrupt_name);

        if path.exists() {
            std::fs::rename(path, &corrupt_path)?;
            tracing::warn!("Renamed corrupt database to {}", corrupt_name);
        }

        // Also rename WAL and SHM files if they exist
        let wal_path = path.with_extension("db-wal");
        if wal_path.exists() {
            let _ = std::fs::rename(
                &wal_path,
                path.with_file_name(format!("{}.wal", corrupt_name)),
            );
        }
        let shm_path = path.with_extension("db-shm");
        if shm_path.exists() {
            let _ = std::fs::rename(
                &shm_path,
                path.with_file_name(format!("{}.shm", corrupt_name)),
            );
        }

        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            path: path.to_path_buf(),
        };
        store.apply_pragmas()?;
        store.migrate()?;
        let _ = store.set_setting("needs_component_rebuild", "1");

        Err(StoreError::CorruptRecovered(corrupt_name))
    }

    /// Set WAL mode, foreign keys, and busy timeout.
    fn apply_pragmas(&self) -> Result<(), StoreError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        Ok(())
    }

    /// Run forward-only migrations based on `PRAGMA user_version`.
    fn migrate(&self) -> Result<(), StoreError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let version: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;

        if version < 1 {
            conn.execute_batch(MIGRATION_V1)
                .map_err(|e| StoreError::MigrationFailed {
                    version: 1,
                    detail: e.to_string(),
                })?;
            conn.pragma_update(None, "user_version", 1)?;
            tracing::info!("Database migrated to schema version 1");
        }

        if version < 2 {
            conn.execute_batch(MIGRATION_V2)
                .map_err(|e| StoreError::MigrationFailed {
                    version: 2,
                    detail: e.to_string(),
                })?;
            conn.pragma_update(None, "user_version", 2)?;
            tracing::info!("Database migrated to schema version 2");
        }

        if version < 3 {
            conn.execute_batch(MIGRATION_V3)
                .map_err(|e| StoreError::MigrationFailed {
                    version: 3,
                    detail: e.to_string(),
                })?;
            conn.pragma_update(None, "user_version", 3)?;
            tracing::info!("Database migrated to schema version 3");
        }

        let final_version: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if final_version != CURRENT_SCHEMA_VERSION {
            return Err(StoreError::MigrationFailed {
                version: final_version,
                detail: format!(
                    "Expected schema version {}, got {}",
                    CURRENT_SCHEMA_VERSION, final_version
                ),
            });
        }

        Ok(())
    }

    /// Get the current schema version.
    pub fn schema_version(&self) -> Result<u32, StoreError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let v: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        Ok(v)
    }

    /// Drops all tables, resets user_version to 0, and re-runs migrations.
    /// Only intended for DEV workspace resets.
    pub fn reset_database(&self) -> Result<(), StoreError> {
        {
            let conn = self.conn.lock().expect("store mutex poisoned");
            conn.execute_batch(
                "PRAGMA foreign_keys = OFF;
                 DROP TABLE IF EXISTS error_events;
                 DROP TABLE IF EXISTS deferred_deletes;
                 DROP TABLE IF EXISTS components;
                 DROP TABLE IF EXISTS account_snapshot;
                 DROP TABLE IF EXISTS outbox;
                 DROP TABLE IF EXISTS settings;
                 DROP TABLE IF EXISTS templates_local;
                 DROP TABLE IF EXISTS stage_stats;
                 DROP TABLE IF EXISTS artifacts;
                 DROP TABLE IF EXISTS jobs;
                 DROP TABLE IF EXISTS projects;
                 PRAGMA user_version = 0;
                 PRAGMA foreign_keys = ON;",
            )?;
        }
        self.migrate()?;
        Ok(())
    }

    // ── Settings DAO ────────────────────────────────────────────────────

    /// Read a setting value by key. Returns `None` if the key does not exist.
    pub fn get_setting(&self, key: &str) -> Result<Option<String>, StoreError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let mut stmt = conn.prepare_cached("SELECT value FROM settings WHERE key = ?1")?;
        let result = stmt.query_row(params![key], |r| r.get::<_, String>(0));
        match result {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(StoreError::Db(e)),
        }
    }

    /// Insert or update a setting.
    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Delete a setting by key. Returns true if a row was deleted.
    pub fn delete_setting(&self, key: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let affected = conn.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
        Ok(affected > 0)
    }

    /// Read the last accepted manifest sequence number (anti-rollback).
    pub fn get_manifest_sequence(&self) -> Result<u64, StoreError> {
        let val = self.get_setting("manifest_last_accepted_sequence")?;
        Ok(val.and_then(|s| s.parse().ok()).unwrap_or(0))
    }

    /// Update the last accepted manifest sequence number.
    pub fn set_manifest_sequence(&self, sequence: u64) -> Result<(), StoreError> {
        self.set_setting("manifest_last_accepted_sequence", &sequence.to_string())
    }

    /// Returns the persistent unique installation identifier, generating one on first run.
    pub fn get_or_create_install_id(&self) -> Result<String, StoreError> {
        if let Some(id) = self.get_setting("install_id")? {
            let trimmed = id.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let pid = std::process::id();
        let new_id = format!("{:016x}{:08x}{:08x}", now, pid, 1u32);
        self.set_setting("install_id", &new_id)?;
        Ok(new_id)
    }

    // ── Stage Stats DAO ─────────────────────────────────────────────────

    /// Record a completed stage timing for ETA calibration (PROGRESS §4.3).
    pub fn insert_stage_stat(
        &self,
        device_profile: &str,
        stage: &str,
        variant: &str,
        work_units: f64,
        wall_ms: i64,
    ) -> Result<(), StoreError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let conn = self.conn.lock().expect("store mutex poisoned");
        conn.execute(
            "INSERT INTO stage_stats (device_profile, stage, variant, work_units, wall_ms, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![device_profile, stage, variant, work_units, wall_ms, now],
        )?;
        Ok(())
    }

    /// Get the median processing rate (work_units / wall_ms) for the last `last_n`
    /// entries matching the given device profile, stage, and variant.
    ///
    /// Returns `None` if no matching rows exist.
    /// Rate is in work_units per millisecond.
    pub fn get_median_rate(
        &self,
        device_profile: &str,
        stage: &str,
        variant: &str,
        last_n: usize,
    ) -> Result<Option<f64>, StoreError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let mut stmt = conn.prepare_cached(
            "SELECT work_units, wall_ms FROM stage_stats
             WHERE device_profile = ?1 AND stage = ?2 AND variant = ?3
               AND wall_ms > 0
             ORDER BY created_at DESC
             LIMIT ?4",
        )?;

        let rates: Vec<f64> = stmt
            .query_map(
                params![device_profile, stage, variant, last_n as i64],
                |r| {
                    let wu: f64 = r.get(0)?;
                    let ms: i64 = r.get(1)?;
                    Ok(wu / ms as f64)
                },
            )?
            .filter_map(|r| r.ok())
            .collect();

        if rates.is_empty() {
            return Ok(None);
        }

        // Compute median
        let mut sorted = rates;
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = sorted.len() / 2;
        let median = if sorted.len().is_multiple_of(2) {
            (sorted[mid - 1] + sorted[mid]) / 2.0
        } else {
            sorted[mid]
        };

        Ok(Some(median))
    }

    // ── Components & Deferred Deletes DAO (Schema v2) ─────────────────

    fn map_component_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ComponentRow> {
        Ok(ComponentRow {
            component: r.get(0)?,
            version: r.get(1)?,
            kind: r.get(2)?,
            install_rel_path: r.get(3)?,
            status: r.get(4)?,
            is_current: r.get::<_, i64>(5)? != 0,
            is_previous: r.get::<_, i64>(6)? != 0,
            manifest_sequence: r.get::<_, i64>(7)? as u64,
            files_json: r.get(8)?,
            capability_report_json: r.get(9)?,
            quarantine_reason: r.get(10)?,
            installed_at: r.get::<_, i64>(11)? as u64,
            updated_at: r.get::<_, i64>(12)? as u64,
        })
    }

    fn lock_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StoreError> {
        self.conn
            .lock()
            .map_err(|_| StoreError::Internal("store mutex poisoned".to_string()))
    }

    /// Reads one specific `(component, version)` record.
    pub fn get_component_version(
        &self,
        component: &str,
        version: &str,
    ) -> Result<Option<ComponentRow>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare_cached(
            "SELECT component, version, kind, install_rel_path, status, is_current, is_previous,
                    manifest_sequence, files_json, capability_report_json, quarantine_reason,
                    installed_at, updated_at
             FROM components
             WHERE component = ?1 AND version = ?2",
        )?;
        match stmt.query_row(params![component, version], Self::map_component_row) {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(StoreError::Db(e)),
        }
    }

    /// Reads the active (`is_current = 1 AND status = 'installed'`) version for a component.
    pub fn get_current_component(
        &self,
        component: &str,
    ) -> Result<Option<ComponentRow>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare_cached(
            "SELECT component, version, kind, install_rel_path, status, is_current, is_previous,
                    manifest_sequence, files_json, capability_report_json, quarantine_reason,
                    installed_at, updated_at
             FROM components
             WHERE component = ?1 AND is_current = 1 AND status = 'installed'",
        )?;
        match stmt.query_row(params![component], Self::map_component_row) {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(StoreError::Db(e)),
        }
    }

    /// Reads the rollback target (`is_previous = 1 AND status = 'installed'`) for a component.
    pub fn get_previous_component(
        &self,
        component: &str,
    ) -> Result<Option<ComponentRow>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare_cached(
            "SELECT component, version, kind, install_rel_path, status, is_current, is_previous,
                    manifest_sequence, files_json, capability_report_json, quarantine_reason,
                    installed_at, updated_at
             FROM components
             WHERE component = ?1 AND is_previous = 1 AND status = 'installed'",
        )?;
        match stmt.query_row(params![component], Self::map_component_row) {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(StoreError::Db(e)),
        }
    }

    /// Lists all tracked versions (installed + quarantined) for a component, newest first.
    pub fn list_component_versions(
        &self,
        component: &str,
    ) -> Result<Vec<ComponentRow>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare_cached(
            "SELECT component, version, kind, install_rel_path, status, is_current, is_previous,
                    manifest_sequence, files_json, capability_report_json, quarantine_reason,
                    installed_at, updated_at
             FROM components
             WHERE component = ?1
             ORDER BY is_current DESC, is_previous DESC, installed_at DESC, manifest_sequence DESC",
        )?;
        let rows = stmt
            .query_map(params![component], Self::map_component_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Lists all rows in the `components` table across every component/model.
    pub fn list_all_components(&self) -> Result<Vec<ComponentRow>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare_cached(
            "SELECT component, version, kind, install_rel_path, status, is_current, is_previous,
                    manifest_sequence, files_json, capability_report_json, quarantine_reason,
                    installed_at, updated_at
             FROM components
             ORDER BY component ASC, is_current DESC, is_previous DESC, installed_at DESC",
        )?;
        let rows = stmt
            .query_map([], Self::map_component_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Atomically promotes `(component, version)` to `is_current = 1` in a single transaction,
    /// demoting any distinct previously current `installed` version to `is_previous = 1`.
    #[allow(clippy::too_many_arguments)]
    pub fn set_component_installed_current(
        &self,
        component: &str,
        version: &str,
        kind: &str,
        install_rel_path: &str,
        manifest_sequence: u64,
        files_json: &str,
        capability_report_json: Option<&str>,
        installed_at: u64,
    ) -> Result<(), StoreError> {
        let now = unix_now_secs() as i64;
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction()?;

        let existing_current: Option<String> = match tx.query_row(
            "SELECT version FROM components
             WHERE component = ?1 AND is_current = 1 AND status = 'installed'",
            params![component],
            |r| r.get(0),
        ) {
            Ok(v) => Some(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(StoreError::Db(e)),
        };

        if existing_current.as_deref() != Some(version) {
            tx.execute(
                "UPDATE components SET is_previous = 0, updated_at = ?2 WHERE component = ?1 AND is_previous = 1",
                params![component, now],
            )?;
            tx.execute(
                "UPDATE components SET is_current = 0, is_previous = 1, updated_at = ?2
                 WHERE component = ?1 AND is_current = 1 AND status = 'installed'",
                params![component, now],
            )?;
            // Also clear any stray is_current=1 (e.g. from unexpected state) so partial index holds
            tx.execute(
                "UPDATE components SET is_current = 0, updated_at = ?2
                 WHERE component = ?1 AND is_current = 1",
                params![component, now],
            )?;
        } else {
            tx.execute(
                "UPDATE components SET is_previous = 0, updated_at = ?3
                 WHERE component = ?1 AND version = ?2 AND is_previous = 1",
                params![component, version, now],
            )?;
        }

        tx.execute(
            "INSERT INTO components (
                component, version, kind, install_rel_path, status, is_current, is_previous,
                manifest_sequence, files_json, capability_report_json, quarantine_reason,
                installed_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, 'installed', 1, 0, ?5, ?6, ?7, NULL, ?8, ?9)
             ON CONFLICT(component, version) DO UPDATE SET
                kind = excluded.kind,
                install_rel_path = excluded.install_rel_path,
                status = 'installed',
                is_current = 1,
                is_previous = 0,
                manifest_sequence = excluded.manifest_sequence,
                files_json = excluded.files_json,
                capability_report_json = COALESCE(excluded.capability_report_json, components.capability_report_json),
                quarantine_reason = NULL,
                updated_at = excluded.updated_at",
            params![
                component,
                version,
                kind,
                install_rel_path,
                manifest_sequence as i64,
                files_json,
                capability_report_json,
                installed_at as i64,
                now
            ],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Marks `bad_version` as `quarantined` with a reason and capability report in one transaction,
    /// rolling `is_current` back to the prior `installed` version if one exists.
    #[allow(clippy::too_many_arguments)]
    pub fn quarantine_and_rollback_component(
        &self,
        component: &str,
        bad_version: &str,
        kind: &str,
        install_rel_path: &str,
        manifest_sequence: u64,
        files_json: &str,
        installed_at: u64,
        reason: &str,
        capability_report_json: Option<&str>,
    ) -> Result<Option<String>, StoreError> {
        let now = unix_now_secs() as i64;
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction()?;

        // Quarantine the bad version and clear its active pointers first.
        tx.execute(
            "INSERT INTO components (
                component, version, kind, install_rel_path, status, is_current, is_previous,
                manifest_sequence, files_json, capability_report_json, quarantine_reason,
                installed_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, 'quarantined', 0, 0, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(component, version) DO UPDATE SET
                status = 'quarantined',
                is_current = 0,
                is_previous = 0,
                capability_report_json = COALESCE(excluded.capability_report_json, components.capability_report_json),
                quarantine_reason = excluded.quarantine_reason,
                updated_at = excluded.updated_at",
            params![
                component,
                bad_version,
                kind,
                install_rel_path,
                manifest_sequence as i64,
                files_json,
                capability_report_json,
                reason,
                installed_at as i64,
                now
            ],
        )?;

        // Check if another version is already current (excluding bad_version)
        let current_other: Option<String> = match tx.query_row(
            "SELECT version FROM components
             WHERE component = ?1 AND version != ?2 AND is_current = 1 AND status = 'installed'",
            params![component, bad_version],
            |r| r.get(0),
        ) {
            Ok(v) => Some(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(StoreError::Db(e)),
        };

        let restored = if let Some(curr) = current_other {
            Some(curr)
        } else {
            // Find best candidate to restore (is_previous first, else newest installed)
            let candidate: Option<String> = match tx.query_row(
                "SELECT version FROM components
                 WHERE component = ?1 AND version != ?2 AND status = 'installed'
                 ORDER BY is_previous DESC, installed_at DESC, manifest_sequence DESC
                 LIMIT 1",
                params![component, bad_version],
                |r| r.get(0),
            ) {
                Ok(v) => Some(v),
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(e) => return Err(StoreError::Db(e)),
            };

            if let Some(ref prev_ver) = candidate {
                tx.execute(
                    "UPDATE components SET is_current = 1, is_previous = 0, updated_at = ?3
                     WHERE component = ?1 AND version = ?2",
                    params![component, prev_ver, now],
                )?;
            }
            candidate
        };

        tx.commit()?;
        Ok(restored)
    }

    /// Switches active `is_current` pointer to another already-installed, non-quarantined version.
    /// Used both for rollback (`current -> previous`) and roll-forward between two installed versions.
    pub fn switch_active_component_version(
        &self,
        component: &str,
        target_version: &str,
    ) -> Result<(), StoreError> {
        let now = unix_now_secs() as i64;
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction()?;

        let target_status: String = tx.query_row(
            "SELECT status FROM components WHERE component = ?1 AND version = ?2",
            params![component, target_version],
            |r| r.get(0),
        )?;
        if target_status != "installed" {
            return Err(StoreError::Internal(format!(
                "Cannot activate {} v{}: version is {}",
                component, target_version, target_status
            )));
        }

        let current_ver: Option<String> = match tx.query_row(
            "SELECT version FROM components WHERE component = ?1 AND is_current = 1",
            params![component],
            |r| r.get(0),
        ) {
            Ok(v) => Some(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(StoreError::Db(e)),
        };

        if current_ver.as_deref() == Some(target_version) {
            tx.commit()?;
            return Ok(());
        }

        tx.execute(
            "UPDATE components SET is_previous = 0, updated_at = ?2 WHERE component = ?1 AND is_previous = 1",
            params![component, now],
        )?;
        tx.execute(
            "UPDATE components SET is_current = 0, is_previous = 1, updated_at = ?2
             WHERE component = ?1 AND is_current = 1 AND status = 'installed'",
            params![component, now],
        )?;
        tx.execute(
            "UPDATE components SET is_current = 0, updated_at = ?2 WHERE component = ?1 AND is_current = 1",
            params![component, now],
        )?;
        tx.execute(
            "UPDATE components SET is_current = 1, is_previous = 0, updated_at = ?3
             WHERE component = ?1 AND version = ?2",
            params![component, target_version, now],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Stores a serialized `CapabilityReport` JSON string on an existing component row.
    pub fn update_component_capability_report(
        &self,
        component: &str,
        version: &str,
        capability_report_json: &str,
    ) -> Result<(), StoreError> {
        let now = unix_now_secs() as i64;
        let conn = self.lock_conn()?;
        conn.execute(
            "UPDATE components SET capability_report_json = ?3, updated_at = ?4
             WHERE component = ?1 AND version = ?2",
            params![component, version, capability_report_json, now],
        )?;
        Ok(())
    }

    /// Removes a specific component version row (used during GC or cleanup of broken versions).
    pub fn delete_component_version_row(
        &self,
        component: &str,
        version: &str,
    ) -> Result<bool, StoreError> {
        let conn = self.lock_conn()?;
        let n = conn.execute(
            "DELETE FROM components WHERE component = ?1 AND version = ?2",
            params![component, version],
        )?;
        Ok(n > 0)
    }

    /// Replaces the entire `components` table atomically (used after DB corruption rebuild from `install.json`).
    pub fn replace_rebuilt_components(&self, rows: &[ComponentRow]) -> Result<(), StoreError> {
        let mut conn = self.lock_conn()?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM components", [])?;
        for r in rows {
            tx.execute(
                "INSERT INTO components (
                    component, version, kind, install_rel_path, status, is_current, is_previous,
                    manifest_sequence, files_json, capability_report_json, quarantine_reason,
                    installed_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    r.component,
                    r.version,
                    r.kind,
                    r.install_rel_path,
                    r.status,
                    if r.is_current { 1i64 } else { 0i64 },
                    if r.is_previous { 1i64 } else { 0i64 },
                    r.manifest_sequence as i64,
                    r.files_json,
                    r.capability_report_json,
                    r.quarantine_reason,
                    r.installed_at as i64,
                    r.updated_at as i64,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Records a path whose deletion was blocked by an OS file lock so startup retries it.
    pub fn add_deferred_delete(&self, path: &str, error_msg: &str) -> Result<(), StoreError> {
        let now = unix_now_secs() as i64;
        let conn = self.lock_conn()?;
        conn.execute(
            "INSERT INTO deferred_deletes (path, created_at, attempts, last_error)
             VALUES (?1, ?2, 1, ?3)
             ON CONFLICT(path) DO UPDATE SET
                attempts = deferred_deletes.attempts + 1,
                last_error = excluded.last_error",
            params![path, now, error_msg],
        )?;
        Ok(())
    }

    /// Returns all pending deferred deletion records.
    pub fn list_deferred_deletes(&self) -> Result<Vec<DeferredDeleteRow>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare_cached(
            "SELECT id, path, created_at, attempts, last_error FROM deferred_deletes ORDER BY id ASC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(DeferredDeleteRow {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    created_at: r.get::<_, i64>(2)? as u64,
                    attempts: r.get::<_, i64>(3)? as u32,
                    last_error: r.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Removes a deferred deletion record once the filesystem cleanup succeeds.
    pub fn remove_deferred_delete(&self, path: &str) -> Result<bool, StoreError> {
        let conn = self.lock_conn()?;
        let n = conn.execute(
            "DELETE FROM deferred_deletes WHERE path = ?1",
            params![path],
        )?;
        Ok(n > 0)
    }

    // ── Error Events & Diagnostics Helpers (Schema v3) ─────────────────

    fn truncate_text(s: &str, max_bytes: usize) -> String {
        if s.len() <= max_bytes {
            return s.to_string();
        }
        let mut end = max_bytes;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...[TRUNCATED]", &s[..end])
    }

    /// Inserts a redacted error event and prunes entries exceeding 30 days or 500 rows.
    pub fn record_error_event(&self, event: NewErrorEvent) -> Result<i64, StoreError> {
        self.record_error_event_at(event, unix_now_secs())
    }

    /// Inserts an error event with explicit timestamp (useful for retention tests) and prunes.
    pub fn record_error_event_at(
        &self,
        event: NewErrorEvent,
        ts_secs: u64,
    ) -> Result<i64, StoreError> {
        let detail_redacted =
            Self::truncate_text(&crate::security::redact_sensitive_text(&event.detail), 4096);
        let stderr_tail_redacted = Self::truncate_text(
            &crate::security::redact_sensitive_text(&event.stderr_tail),
            8192,
        );
        let min_ts = ts_secs.saturating_sub(ERROR_EVENT_RETENTION_DAYS * 86_400) as i64;

        let mut conn = self.lock_conn()?;
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO error_events (
                ts, code, stage, job_id, message_key, detail_redacted,
                stderr_tail_redacted, app_version, component_versions_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                ts_secs as i64,
                event.code,
                event.stage,
                event.job_id,
                event.message_key,
                detail_redacted,
                stderr_tail_redacted,
                event.app_version,
                event.component_versions_json
            ],
        )?;
        let row_id = tx.last_insert_rowid();

        // Enforce 30-day age retention & max 500 newest rows.
        tx.execute("DELETE FROM error_events WHERE ts < ?1", params![min_ts])?;
        tx.execute(
            "DELETE FROM error_events WHERE id NOT IN (
                SELECT id FROM error_events ORDER BY ts DESC, id DESC LIMIT ?1
             )",
            params![ERROR_EVENT_MAX_ROWS as i64],
        )?;
        tx.commit()?;
        Ok(row_id)
    }

    /// Lists recorded error events newest-first, optionally filtered by exact `code`.
    pub fn list_error_events(
        &self,
        code_filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ErrorEventRow>, StoreError> {
        let conn = self.lock_conn()?;
        let capped = limit.clamp(1, ERROR_EVENT_MAX_ROWS) as i64;
        let map_row = |r: &rusqlite::Row<'_>| -> rusqlite::Result<ErrorEventRow> {
            Ok(ErrorEventRow {
                id: r.get::<_, i64>(0)? as f64,
                ts: r.get::<_, i64>(1)? as f64,
                code: r.get(2)?,
                stage: r.get(3)?,
                job_id: r.get(4)?,
                message_key: r.get(5)?,
                detail_redacted: r.get(6)?,
                stderr_tail_redacted: r.get(7)?,
                app_version: r.get(8)?,
                component_versions_json: r.get(9)?,
            })
        };

        let mut out = Vec::new();
        if let Some(code) = code_filter.map(str::trim).filter(|s| !s.is_empty()) {
            let mut stmt = conn.prepare_cached(
                "SELECT id, ts, code, stage, job_id, message_key, detail_redacted,
                        stderr_tail_redacted, app_version, component_versions_json
                 FROM error_events
                 WHERE code = ?1
                 ORDER BY ts DESC, id DESC
                 LIMIT ?2",
            )?;
            for row in stmt.query_map(params![code, capped], map_row)? {
                out.push(row?);
            }
        } else {
            let mut stmt = conn.prepare_cached(
                "SELECT id, ts, code, stage, job_id, message_key, detail_redacted,
                        stderr_tail_redacted, app_version, component_versions_json
                 FROM error_events
                 ORDER BY ts DESC, id DESC
                 LIMIT ?1",
            )?;
            for row in stmt.query_map(params![capped], map_row)? {
                out.push(row?);
            }
        }
        Ok(out)
    }

    /// Serializes ONLY explicitly allowlisted non-sensitive settings for diagnostics reports.
    pub fn export_safe_settings_allowlist(
        &self,
    ) -> Result<std::collections::BTreeMap<String, String>, StoreError> {
        const ALLOWLIST_KEYS: &[&str] = &[
            "language",
            "speech_model",
            "speech_model_downgrade_suggested",
            "speech_model_downgrade_dismissed_for",
            "speech_model_consecutive_slow_runs",
            "preferred_encoder",
            "encoder_fingerprint",
            "encoder_fallback_history",
            "custom_ffmpeg_warning",
            "custom_ffmpeg_path",
            "output_folder",
            "manifest_last_accepted_sequence",
        ];

        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare_cached("SELECT key, value FROM settings")?;
        let mut out = std::collections::BTreeMap::new();
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (k, v) = row?;
            if ALLOWLIST_KEYS.contains(&k.as_str()) {
                let safe_val = crate::security::redact_report_envelope(&v);
                out.insert(k, safe_val);
            }
        }
        Ok(out)
    }

    /// Returns up to `limit` most recent jobs with non-sensitive fields and validated 11-char video IDs.
    pub fn list_recent_job_summaries(
        &self,
        limit: usize,
        include_video_ids: bool,
    ) -> Result<Vec<SafeJobSummary>, StoreError> {
        let conn = self.lock_conn()?;
        let mut stmt = conn.prepare_cached(
            "SELECT j.id, j.kind, j.status, j.stage, j.error_code, j.error_detail,
                    j.created_at, j.finished_at, p.source_video_id
             FROM jobs j
             LEFT JOIN projects p ON p.id = j.project_id
             ORDER BY j.created_at DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit.clamp(1, 50) as i64], |r| {
            let raw_vid: Option<String> = r.get(8)?;
            let valid_vid = if include_video_ids {
                raw_vid.and_then(|v| sanitize_youtube_video_id(&v))
            } else {
                None
            };
            let raw_err: Option<String> = r.get(5)?;
            Ok(SafeJobSummary {
                id: r.get(0)?,
                kind: r.get(1)?,
                status: r.get(2)?,
                stage: r.get(3)?,
                error_code: r.get(4)?,
                video_id: valid_vid,
                created_at: r.get::<_, i64>(6)? as u64,
                finished_at: r.get::<_, Option<i64>>(7)?.map(|v| v as u64),
                error_detail_redacted: raw_err.map(|e| crate::security::redact_sensitive_text(&e)),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::Db)
    }

    /// Lists distinct `(device_profile, stage, variant)` medians from `stage_stats` for diagnostics.
    pub fn list_stage_stat_medians(&self) -> Result<Vec<StageStatMedian>, StoreError> {
        let keys: Vec<(String, String, String)> = {
            let conn = self.lock_conn()?;
            let mut stmt = conn.prepare_cached(
                "SELECT DISTINCT device_profile, stage, variant FROM stage_stats
                 WHERE wall_ms > 0
                 ORDER BY stage ASC, variant ASC",
            )?;
            let mapped = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };

        let mut results = Vec::new();
        for (dp, stage, variant) in keys {
            let count: usize = {
                let conn = self.lock_conn()?;
                conn.query_row(
                    "SELECT COUNT(*) FROM stage_stats
                     WHERE device_profile = ?1 AND stage = ?2 AND variant = ?3 AND wall_ms > 0",
                    params![dp, stage, variant],
                    |r| r.get::<_, i64>(0),
                )? as usize
            };
            if let Some(rate_per_ms) = self.get_median_rate(&dp, &stage, &variant, 20)? {
                results.push(StageStatMedian {
                    device_profile: dp,
                    stage,
                    variant,
                    sample_count: count,
                    median_rate_units_per_sec: rate_per_ms * 1000.0,
                });
            }
        }
        Ok(results)
    }

    /// Return the database file path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Accepts only strict 11-character YouTube video IDs (`[A-Za-z0-9_-]{11}`); never URLs/query strings.
pub fn sanitize_youtube_video_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.len() == 11
        && trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        Some(trimmed.to_string())
    } else {
        None
    }
}

// ── Migration SQL ───────────────────────────────────────────────────────

/// V1: Initial schema per DATA_MODEL §9.
const MIGRATION_V1: &str = "
CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY NOT NULL,
    title TEXT,
    source_provider TEXT,
    source_video_id TEXT,
    source_url TEXT,
    source_title TEXT,
    range_start_ms INTEGER,
    range_end_ms INTEGER,
    template_id TEXT,
    composition_json TEXT,
    composition_hash TEXT,
    status TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_export_at INTEGER
);

CREATE TABLE IF NOT EXISTS jobs (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('generate','captions','export','model_download','component_update','app_update')),
    status TEXT NOT NULL CHECK (status IN ('queued','running','paused','succeeded','failed','cancelled')),
    stage TEXT,
    progress_json TEXT,
    error_code TEXT,
    error_detail TEXT,
    idempotency_key TEXT,
    attempt INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    started_at INTEGER,
    finished_at INTEGER
);

CREATE TABLE IF NOT EXISTS artifacts (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT,
    kind TEXT NOT NULL CHECK (kind IN ('source','audio','transcript','proxy','waveform','output')),
    cache_key TEXT UNIQUE,
    path TEXT NOT NULL,
    size_bytes INTEGER,
    sha256 TEXT,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER,
    pinned INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS stage_stats (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    device_profile TEXT NOT NULL,
    stage TEXT NOT NULL,
    variant TEXT NOT NULL,
    work_units REAL NOT NULL,
    wall_ms INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS templates_local (
    id TEXT PRIMARY KEY NOT NULL,
    remote_id TEXT,
    name TEXT,
    schema_version INTEGER,
    body_json TEXT,
    body_hash TEXT,
    updated_at INTEGER,
    dirty INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS outbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_retry_at INTEGER
);

CREATE TABLE IF NOT EXISTS account_snapshot (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    plan_code TEXT,
    daily_credits INTEGER,
    used_today INTEGER,
    resets_at INTEGER,
    pass_ends_at INTEGER,
    fetched_at INTEGER
);

-- Indexes for common query patterns
CREATE INDEX IF NOT EXISTS idx_jobs_project ON jobs(project_id);
CREATE INDEX IF NOT EXISTS idx_jobs_status ON jobs(status) WHERE status IN ('queued','running','paused');
CREATE INDEX IF NOT EXISTS idx_artifacts_project ON artifacts(project_id);
CREATE INDEX IF NOT EXISTS idx_artifacts_cache_key ON artifacts(cache_key);
CREATE INDEX IF NOT EXISTS idx_artifacts_last_used ON artifacts(last_used_at) WHERE pinned = 0;
CREATE INDEX IF NOT EXISTS idx_stage_stats_lookup ON stage_stats(device_profile, stage, variant, created_at);
CREATE INDEX IF NOT EXISTS idx_outbox_retry ON outbox(next_retry_at) WHERE attempts < 10;
";

/// V2: Versioned components, quarantine, capability reports, and deferred lock-retry deletes.
const MIGRATION_V2: &str = "
CREATE TABLE IF NOT EXISTS components (
    component TEXT NOT NULL,
    version TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('bin', 'model')),
    install_rel_path TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('installed', 'quarantined')),
    is_current INTEGER NOT NULL DEFAULT 0 CHECK (is_current IN (0, 1)),
    is_previous INTEGER NOT NULL DEFAULT 0 CHECK (is_previous IN (0, 1)),
    manifest_sequence INTEGER NOT NULL DEFAULT 0,
    files_json TEXT NOT NULL,
    capability_report_json TEXT,
    quarantine_reason TEXT,
    installed_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (component, version)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_components_unique_current
    ON components(component) WHERE is_current = 1;
CREATE UNIQUE INDEX IF NOT EXISTS idx_components_unique_previous
    ON components(component) WHERE is_previous = 1;
CREATE INDEX IF NOT EXISTS idx_components_lookup
    ON components(component, status, installed_at DESC);

CREATE TABLE IF NOT EXISTS deferred_deletes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT
);
";

/// V3: Local typed error store (`error_events`) with indexed lookup & retention.
const MIGRATION_V3: &str = "
CREATE TABLE IF NOT EXISTS error_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts INTEGER NOT NULL,
    code TEXT NOT NULL,
    stage TEXT NOT NULL,
    job_id TEXT,
    message_key TEXT NOT NULL,
    detail_redacted TEXT NOT NULL,
    stderr_tail_redacted TEXT NOT NULL DEFAULT '',
    app_version TEXT NOT NULL,
    component_versions_json TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_error_events_ts ON error_events(ts DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_error_events_code ON error_events(code, ts DESC);
";

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn temp_db() -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("create temp dir");
        let path = dir.path().join("test.db");
        (dir, path)
    }

    #[test]
    fn test_open_creates_fresh_database() {
        let (_dir, path) = temp_db();
        let store = LocalStore::open(&path).expect("open fresh db");
        let version = store.schema_version().expect("get version");
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn test_migration_idempotency() {
        let (_dir, path) = temp_db();
        // Open twice — migrations should not fail on second open
        let store1 = LocalStore::open(&path).expect("first open");
        assert_eq!(store1.schema_version().expect("v1"), CURRENT_SCHEMA_VERSION);
        drop(store1);

        let store2 = LocalStore::open(&path).expect("second open");
        assert_eq!(store2.schema_version().expect("v2"), CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn test_corruption_recovery() {
        let (_dir, path) = temp_db();
        // Write garbage to the file
        fs::write(&path, b"this is not a sqlite database").expect("write garbage");

        // Open should detect corruption and recover
        let result = LocalStore::open(&path);
        match result {
            Err(StoreError::CorruptRecovered(name)) => {
                assert!(name.contains("corrupt"), "got: {}", name);
                // The original path should now be a fresh database
                assert!(path.exists(), "fresh db should exist");
                // Open the fresh database to verify it works
                let store = LocalStore::open(&path).expect("open recovered db");
                assert_eq!(
                    store.schema_version().expect("version"),
                    CURRENT_SCHEMA_VERSION
                );
            }
            Ok(_) => panic!("Expected CorruptRecovered error, got Ok"),
            Err(e) => panic!("Expected CorruptRecovered error, got: {:?}", e),
        }
    }

    #[test]
    fn test_settings_crud() {
        let (_dir, path) = temp_db();
        let store = LocalStore::open(&path).expect("open");

        // Read nonexistent key
        assert_eq!(store.get_setting("lang").expect("get"), None);

        // Set and read
        store.set_setting("lang", "id").expect("set");
        assert_eq!(
            store.get_setting("lang").expect("get"),
            Some("id".to_string())
        );

        // Update
        store.set_setting("lang", "en").expect("update");
        assert_eq!(
            store.get_setting("lang").expect("get after update"),
            Some("en".to_string())
        );

        // Delete
        assert!(store.delete_setting("lang").expect("delete"));
        assert_eq!(store.get_setting("lang").expect("get after delete"), None);

        // Delete nonexistent
        assert!(!store.delete_setting("nope").expect("delete nope"));
    }

    #[test]
    fn test_stage_stats_insert_and_median() {
        let (_dir, path) = temp_db();
        let store = LocalStore::open(&path).expect("open");

        // No data → None
        assert_eq!(
            store
                .get_median_rate("mid_laptop", "download", "default", 10)
                .expect("empty"),
            None
        );

        // Insert 5 data points with known rates:
        // rate = work_units / wall_ms
        // 100/50=2, 200/100=2, 300/100=3, 400/200=2, 500/100=5
        // sorted rates: [2, 2, 2, 3, 5] → median = 2
        let data = [
            (100.0_f64, 50_i64),
            (200.0, 100),
            (300.0, 100),
            (400.0, 200),
            (500.0, 100),
        ];
        for (wu, ms) in &data {
            store
                .insert_stage_stat("mid_laptop", "download", "default", *wu, *ms)
                .expect("insert");
        }

        let median = store
            .get_median_rate("mid_laptop", "download", "default", 10)
            .expect("median")
            .expect("should have data");
        assert!(
            (median - 2.0).abs() < 1e-10,
            "expected median ~2.0, got {}",
            median
        );

        // last_n = 3: takes the 3 most recent → rates [3, 2, 5] → sorted [2, 3, 5] → median = 3
        let median3 = store
            .get_median_rate("mid_laptop", "download", "default", 3)
            .expect("median3")
            .expect("should have data");
        assert!(
            (median3 - 3.0).abs() < 1e-10,
            "expected median ~3.0, got {}",
            median3
        );
    }

    #[test]
    fn test_stage_stats_different_profiles() {
        let (_dir, path) = temp_db();
        let store = LocalStore::open(&path).expect("open");

        store
            .insert_stage_stat("low_end", "transcribe", "small-q5_1", 60.0, 30000)
            .expect("insert low");
        store
            .insert_stage_stat("high_end", "transcribe", "small-q5_1", 60.0, 15000)
            .expect("insert high");

        // Querying for "low_end" should only get that profile's data
        let rate = store
            .get_median_rate("low_end", "transcribe", "small-q5_1", 10)
            .expect("get")
            .expect("should exist");
        assert!(
            (rate - 60.0 / 30000.0).abs() < 1e-10,
            "wrong rate: {}",
            rate
        );
    }

    #[test]
    fn test_concurrent_access() {
        let (_dir, path) = temp_db();
        let store = LocalStore::open(&path).expect("open");

        let handles: Vec<_> = (0..8)
            .map(|i| {
                let s = store.clone();
                std::thread::spawn(move || {
                    let key = format!("key_{}", i);
                    let val = format!("val_{}", i);
                    s.set_setting(&key, &val).expect("set");
                    let got = s.get_setting(&key).expect("get");
                    assert_eq!(got, Some(val));
                })
            })
            .collect();

        for h in handles {
            h.join().expect("thread panicked");
        }

        // Verify all 8 keys exist
        for i in 0..8 {
            let key = format!("key_{}", i);
            let val = format!("val_{}", i);
            assert_eq!(store.get_setting(&key).expect("final get"), Some(val));
        }
    }

    #[test]
    fn test_tables_exist() {
        let (_dir, path) = temp_db();
        let store = LocalStore::open(&path).expect("open");
        let conn = store.conn.lock().expect("lock");

        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .expect("prepare")
            .query_map([], |r| r.get(0))
            .expect("query")
            .filter_map(|r| r.ok())
            .collect();

        let expected = [
            "account_snapshot",
            "artifacts",
            "components",
            "deferred_deletes",
            "jobs",
            "outbox",
            "projects",
            "settings",
            "stage_stats",
            "templates_local",
        ];
        for name in &expected {
            assert!(
                tables.contains(&name.to_string()),
                "Missing table: {}. Found: {:?}",
                name,
                tables
            );
        }
    }

    #[test]
    fn test_wal_mode_enabled() {
        let (_dir, path) = temp_db();
        let store = LocalStore::open(&path).expect("open");
        let conn = store.conn.lock().expect("lock");
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .expect("pragma");
        assert_eq!(mode, "wal");
    }

    #[test]
    fn test_job_check_constraints() {
        let (_dir, path) = temp_db();
        let store = LocalStore::open(&path).expect("open");
        let conn = store.conn.lock().expect("lock");

        // Valid insert
        let res = conn.execute(
            "INSERT INTO jobs (id, project_id, kind, status, created_at)
             VALUES ('j1', 'p1', 'generate', 'queued', 1000)",
            [],
        );
        assert!(res.is_ok(), "valid insert should succeed");

        // Invalid kind
        let res = conn.execute(
            "INSERT INTO jobs (id, project_id, kind, status, created_at)
             VALUES ('j2', 'p1', 'invalid_kind', 'queued', 1000)",
            [],
        );
        assert!(res.is_err(), "invalid kind should fail CHECK");

        // Invalid status
        let res = conn.execute(
            "INSERT INTO jobs (id, project_id, kind, status, created_at)
             VALUES ('j3', 'p1', 'export', 'invalid_status', 1000)",
            [],
        );
        assert!(res.is_err(), "invalid status should fail CHECK");
    }

    #[test]
    fn test_artifact_check_constraints() {
        let (_dir, path) = temp_db();
        let store = LocalStore::open(&path).expect("open");
        let conn = store.conn.lock().expect("lock");

        // Valid insert
        let res = conn.execute(
            "INSERT INTO artifacts (id, project_id, kind, path, created_at)
             VALUES ('a1', 'p1', 'source', 'cache/video.mp4', 1000)",
            [],
        );
        assert!(res.is_ok(), "valid artifact insert should succeed");

        // Invalid kind
        let res = conn.execute(
            "INSERT INTO artifacts (id, project_id, kind, path, created_at)
             VALUES ('a2', 'p1', 'invalid_kind', 'cache/x.mp4', 1000)",
            [],
        );
        assert!(res.is_err(), "invalid artifact kind should fail CHECK");

        // Duplicate cache_key
        conn.execute(
            "INSERT INTO artifacts (id, project_id, kind, cache_key, path, created_at)
             VALUES ('a3', 'p1', 'audio', 'ck1', 'cache/a.wav', 1000)",
            [],
        )
        .expect("first cache_key insert");
        let res = conn.execute(
            "INSERT INTO artifacts (id, project_id, kind, cache_key, path, created_at)
             VALUES ('a4', 'p1', 'audio', 'ck1', 'cache/b.wav', 2000)",
            [],
        );
        assert!(res.is_err(), "duplicate cache_key should fail UNIQUE");
    }

    #[test]
    fn test_account_snapshot_singleton() {
        let (_dir, path) = temp_db();
        let store = LocalStore::open(&path).expect("open");
        let conn = store.conn.lock().expect("lock");

        // Insert with id=1 is valid
        conn.execute(
            "INSERT INTO account_snapshot (id, plan_code, daily_credits, used_today)
             VALUES (1, 'free', 3, 0)",
            [],
        )
        .expect("insert id=1");

        // Insert with id=2 violates CHECK (id = 1)
        let res = conn.execute(
            "INSERT INTO account_snapshot (id, plan_code, daily_credits, used_today)
             VALUES (2, 'free', 3, 0)",
            [],
        );
        assert!(res.is_err(), "id=2 should fail CHECK (id = 1)");

        // Update id=1 works (upsert pattern)
        conn.execute(
            "INSERT OR REPLACE INTO account_snapshot (id, plan_code, daily_credits, used_today)
             VALUES (1, 'pass_30', 20, 5)",
            [],
        )
        .expect("upsert id=1");

        let plan: String = conn
            .query_row(
                "SELECT plan_code FROM account_snapshot WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .expect("read back");
        assert_eq!(plan, "pass_30");
    }

    #[test]
    fn test_v1_to_v2_forward_migration_preserves_data() {
        let (_dir, path) = temp_db();

        // Create an explicit v1 database first
        {
            let conn = Connection::open(&path).expect("open v1 conn");
            conn.execute_batch(MIGRATION_V1).expect("apply v1");
            conn.pragma_update(None, "user_version", 1)
                .expect("set v1 user_version");
            conn.execute(
                "INSERT INTO settings (key, value) VALUES ('install_id', 'existing-id-123')",
                [],
            )
            .expect("seed v1 setting");
        }

        // Open through LocalStore -> should migrate automatically v1 -> current (v3)
        let store = LocalStore::open(&path).expect("open & migrate v1->current");
        assert_eq!(store.schema_version().expect("version"), CURRENT_SCHEMA_VERSION);
        assert_eq!(
            store.get_setting("install_id").expect("preserved setting"),
            Some("existing-id-123".to_string())
        );

        // Verify v2 tables work
        store
            .set_component_installed_current(
                "ffmpeg",
                "n9.0.2",
                "bin",
                "bin/ffmpeg/n9.0.2",
                1,
                "[]",
                Some("{\"status\":\"pass\"}"),
                1000,
            )
            .expect("insert component v2");
        let cur = store
            .get_current_component("ffmpeg")
            .expect("query current")
            .expect("must exist");
        assert_eq!(cur.version, "n9.0.2");
        assert!(cur.is_current);
    }

    #[test]
    fn test_components_promotion_rollback_and_quarantine() {
        let (_dir, path) = temp_db();
        let store = LocalStore::open(&path).expect("open v2");

        // Install v1 -> current
        store
            .set_component_installed_current(
                "ffmpeg",
                "1.0.0",
                "bin",
                "bin/ffmpeg/1.0.0",
                1,
                "[]",
                None,
                100,
            )
            .unwrap();
        // Install v2 -> current, v1 -> previous
        store
            .set_component_installed_current(
                "ffmpeg",
                "2.0.0",
                "bin",
                "bin/ffmpeg/2.0.0",
                2,
                "[]",
                None,
                200,
            )
            .unwrap();

        assert_eq!(
            store
                .get_current_component("ffmpeg")
                .unwrap()
                .unwrap()
                .version,
            "2.0.0"
        );
        assert_eq!(
            store
                .get_previous_component("ffmpeg")
                .unwrap()
                .unwrap()
                .version,
            "1.0.0"
        );

        // Install bad v3, then quarantine & roll back to 2.0.0
        store
            .set_component_installed_current(
                "ffmpeg",
                "3.0.0",
                "bin",
                "bin/ffmpeg/3.0.0",
                3,
                "[]",
                None,
                300,
            )
            .unwrap();
        let restored = store
            .quarantine_and_rollback_component(
                "ffmpeg",
                "3.0.0",
                "bin",
                "bin/ffmpeg/3.0.0",
                3,
                "[]",
                300,
                "--enable-gpl detected",
                Some("{\"status\":\"fail\"}"),
            )
            .unwrap();
        assert_eq!(restored, Some("2.0.0".to_string()));
        assert_eq!(
            store
                .get_current_component("ffmpeg")
                .unwrap()
                .unwrap()
                .version,
            "2.0.0"
        );
        let q = store
            .get_component_version("ffmpeg", "3.0.0")
            .unwrap()
            .unwrap();
        assert_eq!(q.status, "quarantined");
        assert!(!q.is_current);
        assert_eq!(
            q.quarantine_reason.as_deref(),
            Some("--enable-gpl detected")
        );

        // Switch active version manually backward (to 1.0.0) and forward (to 2.0.0)
        store
            .switch_active_component_version("ffmpeg", "1.0.0")
            .unwrap();
        assert_eq!(
            store
                .get_current_component("ffmpeg")
                .unwrap()
                .unwrap()
                .version,
            "1.0.0"
        );
        assert_eq!(
            store
                .get_previous_component("ffmpeg")
                .unwrap()
                .unwrap()
                .version,
            "2.0.0"
        );

        // Quarantined version must refuse activation
        assert!(store
            .switch_active_component_version("ffmpeg", "3.0.0")
            .is_err());

        // Roll forward to 2.0.0
        store
            .switch_active_component_version("ffmpeg", "2.0.0")
            .unwrap();
        assert_eq!(
            store
                .get_current_component("ffmpeg")
                .unwrap()
                .unwrap()
                .version,
            "2.0.0"
        );
    }

    #[test]
    fn test_deferred_deletes_lifecycle() {
        let (_dir, path) = temp_db();
        let store = LocalStore::open(&path).expect("open");

        store
            .add_deferred_delete(r"C:\tmp\locked\v1", "sharing violation")
            .unwrap();
        store
            .add_deferred_delete(r"C:\tmp\locked\v1", "still locked")
            .unwrap();

        let list = store.list_deferred_deletes().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].attempts, 2);
        assert_eq!(list[0].last_error.as_deref(), Some("still locked"));

        assert!(store.remove_deferred_delete(r"C:\tmp\locked\v1").unwrap());
        assert!(store.list_deferred_deletes().unwrap().is_empty());
    }

    #[test]
    fn test_error_events_redaction_filter_and_retention() {
        let (_dir, path) = temp_db();
        let store = LocalStore::open(&path).expect("open");
        assert_eq!(store.schema_version().unwrap(), 3);

        let now = 1_800_000_000u64;
        // 1. Expired row (> 30 days old) gets pruned when recording a newer row
        store
            .record_error_event_at(
                NewErrorEvent {
                    code: "E_NET_OFFLINE".into(),
                    stage: "manifest".into(),
                    job_id: None,
                    message_key: "err.E_NET_OFFLINE.body".into(),
                    detail: "old error".into(),
                    stderr_tail: String::new(),
                    app_version: "0.1.0".into(),
                    component_versions_json: "{}".into(),
                },
                now - (31 * 86_400),
            )
            .unwrap();

        // 2. Record dirty row with PII/token -> redacted before insert
        store
            .record_error_event_at(
                NewErrorEvent {
                    code: "E_YT_EXTRACT_FAILED".into(),
                    stage: "download".into(),
                    job_id: Some("job-1".into()),
                    message_key: "err.E_YT_EXTRACT_FAILED.body".into(),
                    detail: r"failed reading C:\Users\Budi\x.mp4 token=secret99".into(),
                    stderr_tail: "stderr email=budi@example.com".into(),
                    app_version: "0.1.0".into(),
                    component_versions_json: r#"{"yt-dlp":"2026.10.26"}"#.into(),
                },
                now,
            )
            .unwrap();

        let rows = store.list_error_events(None, 50).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].code, "E_YT_EXTRACT_FAILED");
        assert!(!rows[0].detail_redacted.contains("Budi"));
        assert!(!rows[0].detail_redacted.contains("secret99"));
        assert!(!rows[0].stderr_tail_redacted.contains('@'));

        // 3. Enforce max 500-row retention cap
        for i in 0..520u64 {
            store
                .record_error_event_at(
                    NewErrorEvent {
                        code: if i % 2 == 0 {
                            "E_VERIFY_FAILED".into()
                        } else {
                            "E_RENDER_FAILED".into()
                        },
                        stage: "render".into(),
                        job_id: None,
                        message_key: "err.E_RENDER_FAILED.body".into(),
                        detail: format!("err #{}", i),
                        stderr_tail: String::new(),
                        app_version: "0.1.0".into(),
                        component_versions_json: "{}".into(),
                    },
                    now + i + 1,
                )
                .unwrap();
        }
        let all_after = store.list_error_events(None, 600).unwrap();
        assert_eq!(all_after.len(), ERROR_EVENT_MAX_ROWS);

        let filtered = store
            .list_error_events(Some("E_VERIFY_FAILED"), 50)
            .unwrap();
        assert_eq!(filtered.len(), 50);
        assert!(filtered.iter().all(|r| r.code == "E_VERIFY_FAILED"));

        // 4. Settings allowlist excludes unknown/sensitive keys
        store.set_setting("language", "id").unwrap();
        store.set_setting("speech_model", "small-q5_1").unwrap();
        store
            .set_setting("supabase_service_role_key", "SHOULD_NEVER_EXPORT")
            .unwrap();
        store.set_setting("random_token", "SECRET_VALUE").unwrap();
        let safe_settings = store.export_safe_settings_allowlist().unwrap();
        assert_eq!(
            safe_settings.get("language").map(String::as_str),
            Some("id")
        );
        assert_eq!(
            safe_settings.get("speech_model").map(String::as_str),
            Some("small-q5_1")
        );
        assert!(!safe_settings.contains_key("supabase_service_role_key"));
        assert!(!safe_settings.contains_key("random_token"));
    }

    #[test]
    fn test_v2_to_v3_forward_migration_preserves_data() {
        let (_dir, path) = temp_db();
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATION_V1).unwrap();
            conn.execute_batch(MIGRATION_V2).unwrap();
            conn.pragma_update(None, "user_version", 2).unwrap();
            conn.execute(
                "INSERT INTO settings (key, value) VALUES ('preferred_encoder', 'h264_nvenc')",
                [],
            )
            .unwrap();
        }
        let store = LocalStore::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), 3);
        assert_eq!(
            store.get_setting("preferred_encoder").unwrap().as_deref(),
            Some("h264_nvenc")
        );
        assert!(store.list_error_events(None, 10).unwrap().is_empty());
    }
}
