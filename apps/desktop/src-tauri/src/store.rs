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
use thiserror::Error;

/// Current schema version. Bump this when adding new migrations.
const CURRENT_SCHEMA_VERSION: u32 = 1;

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

        // Future migrations go here:
        // if version < 2 { ... }

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

    /// Return the database file path.
    pub fn path(&self) -> &Path {
        &self.path
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
}
