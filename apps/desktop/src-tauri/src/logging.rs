//! Rotating redacted file logger and log reader (ARCHITECTURE §14; SECURITY §11–§12).
//!
//! Writes redacted tracing events into `<workspace>/logs/` with automatic file rotation
//! and strict retention limits (max 7 days age, max 20 MB total across all log files).

use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tracing::field::{Field, Visit};
use tracing::Subscriber;
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

use crate::security;

pub const LOG_RETENTION_DAYS: u64 = 7;
pub const LOG_RETENTION_SECS: u64 = LOG_RETENTION_DAYS * 86_400;
pub const LOG_MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024; // 20 MB
pub const LOG_MAX_SINGLE_FILE_BYTES: u64 = 4 * 1024 * 1024; // Rotate every 4 MB

static LOG_WRITER_STATE: OnceLock<Mutex<Option<RotatingFileSink>>> = OnceLock::new();

fn sink_cell() -> &'static Mutex<Option<RotatingFileSink>> {
    LOG_WRITER_STATE.get_or_init(|| Mutex::new(None))
}

#[derive(Debug)]
pub struct RotatingFileSink {
    logs_dir: PathBuf,
    current_path: PathBuf,
    current_size: u64,
    writer: BufWriter<File>,
    rotation_seq: u64,
    max_file_bytes: u64,
    max_total_bytes: u64,
    max_age_secs: u64,
}

impl RotatingFileSink {
    pub fn open(logs_dir: &Path) -> Result<Self, std::io::Error> {
        Self::open_with_limits(
            logs_dir,
            LOG_MAX_SINGLE_FILE_BYTES,
            LOG_MAX_TOTAL_BYTES,
            LOG_RETENTION_SECS,
        )
    }

    pub fn open_with_limits(
        logs_dir: &Path,
        max_file_bytes: u64,
        max_total_bytes: u64,
        max_age_secs: u64,
    ) -> Result<Self, std::io::Error> {
        fs::create_dir_all(logs_dir)?;
        prune_old_logs(logs_dir, max_total_bytes, max_age_secs);

        let current_path = logs_dir.join("clipper.log");
        let existing_len = fs::metadata(&current_path).map(|m| m.len()).unwrap_or(0);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&current_path)?;

        Ok(Self {
            logs_dir: logs_dir.to_path_buf(),
            current_path,
            current_size: existing_len,
            writer: BufWriter::new(file),
            rotation_seq: 0,
            max_file_bytes,
            max_total_bytes,
            max_age_secs,
        })
    }

    pub fn write_line(&mut self, raw_line: &str) -> Result<(), std::io::Error> {
        let redacted = security::redact_sensitive_text(raw_line);
        let line_bytes = redacted.len() as u64 + 1;

        if self.current_size > 0
            && self.current_size.saturating_add(line_bytes) > self.max_file_bytes
        {
            self.rotate()?;
        }

        writeln!(self.writer, "{}", redacted)?;
        self.writer.flush()?;
        self.current_size = self.current_size.saturating_add(line_bytes);
        Ok(())
    }

    fn rotate(&mut self) -> Result<(), std::io::Error> {
        let _ = self.writer.flush();
        self.rotation_seq = self.rotation_seq.saturating_add(1);
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let archived = self
            .logs_dir
            .join(format!("clipper-{}-{}.log", ts, self.rotation_seq));
        if self.current_path.exists() {
            fs::rename(&self.current_path, &archived)?;
        }

        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.current_path)?;
        self.writer = BufWriter::new(file);
        self.current_size = 0;

        prune_old_logs(&self.logs_dir, self.max_total_bytes, self.max_age_secs);
        Ok(())
    }
}

/// Removes log files older than `max_age_secs` and trims oldest `.log` files until
/// total directory log size is at or below `max_total_bytes`.
pub fn prune_old_logs(logs_dir: &Path, max_total_bytes: u64, max_age_secs: u64) {
    let Ok(entries) = fs::read_dir(logs_dir) else {
        return;
    };

    let now = SystemTime::now();
    let cutoff = now.checked_sub(Duration::from_secs(max_age_secs));

    let mut remaining: Vec<(PathBuf, SystemTime, u64)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_log = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("log"));
        if !is_log {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let modified = meta.modified().unwrap_or(now);
        let len = meta.len();

        let is_active = path.file_name().and_then(|n| n.to_str()) == Some("clipper.log");
        if !is_active {
            if let Some(cut) = cutoff {
                if modified < cut {
                    let _ = fs::remove_file(&path);
                    continue;
                }
            }
        }
        remaining.push((path, modified, len));
    }

    // Oldest first so we drop stale logs before recent ones.
    remaining.sort_by_key(|(_, modified, _)| *modified);
    let mut total_size: u64 = remaining.iter().map(|(_, _, sz)| *sz).sum();

    for (path, _, sz) in remaining {
        if total_size <= max_total_bytes {
            break;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("clipper.log") {
            continue;
        }
        if fs::remove_file(&path).is_ok() {
            total_size = total_size.saturating_sub(sz);
        }
    }
}

/// Activates rotating file logging once the verified workspace `logs/` directory is ready.
pub fn activate_file_logging(logs_dir: &Path) -> Result<(), std::io::Error> {
    let sink = RotatingFileSink::open(logs_dir)?;
    if let Ok(mut guard) = sink_cell().lock() {
        *guard = Some(sink);
    }
    Ok(())
}

/// Flushes pending buffered log lines to disk (called prior to diagnostics read or shutdown).
pub fn flush_file_logging() {
    if let Ok(mut guard) = sink_cell().lock() {
        if let Some(sink) = guard.as_mut() {
            let _ = sink.writer.flush();
        }
    }
}

/// Formats an epoch-seconds timestamp compactly in UTC ISO-like form without pulling chrono.
fn format_utc_timestamp(epoch_secs: u64) -> String {
    format!("ts={}", epoch_secs)
}

/// `tracing` layer that formats events cleanly and writes redacted lines into `logs/`.
pub struct RotatingFileLogLayer;

impl<S> Layer<S> for RotatingFileLogLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        struct LineVisitor {
            message: String,
            fields: String,
        }
        impl Visit for LineVisitor {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                let name = field.name();
                if is_sensitive_field(name) {
                    if !self.fields.is_empty() {
                        self.fields.push(' ');
                    }
                    let _ = write!(self.fields, "{}=[REDACTED]", name);
                    return;
                }
                let raw = format!("{:?}", value);
                if name == "message" {
                    self.message = raw;
                } else {
                    if !self.fields.is_empty() {
                        self.fields.push(' ');
                    }
                    let _ = write!(self.fields, "{}={}", name, raw);
                }
            }

            fn record_str(&mut self, field: &Field, value: &str) {
                let name = field.name();
                if is_sensitive_field(name) {
                    if !self.fields.is_empty() {
                        self.fields.push(' ');
                    }
                    let _ = write!(self.fields, "{}=[REDACTED]", name);
                    return;
                }
                if name == "message" {
                    self.message = value.to_string();
                } else {
                    if !self.fields.is_empty() {
                        self.fields.push(' ');
                    }
                    let _ = write!(self.fields, "{}={}", name, value);
                }
            }
        }

        let mut visitor = LineVisitor {
            message: String::new(),
            fields: String::new(),
        };
        event.record(&mut visitor);

        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let meta = event.metadata();
        let line = if visitor.fields.is_empty() {
            format!(
                "[{}] {} {}: {}",
                format_utc_timestamp(now_secs),
                meta.level(),
                meta.target(),
                visitor.message
            )
        } else {
            format!(
                "[{}] {} {}: {} ({})",
                format_utc_timestamp(now_secs),
                meta.level(),
                meta.target(),
                visitor.message,
                visitor.fields
            )
        };

        if let Ok(mut guard) = sink_cell().lock() {
            if let Some(sink) = guard.as_mut() {
                let _ = sink.write_line(&line);
            }
        }
    }
}

fn is_sensitive_field(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("token")
        || lower.contains("key")
        || lower.contains("secret")
        || lower.contains("jwt")
        || lower.contains("password")
        || lower.contains("authorization")
        || lower.contains("cookie")
        || lower.contains("license")
}

/// Reads up to `max_lines` and `max_bytes` of the most recent log lines from `<logs_dir>/*.log`,
/// guaranteeing every line is passed through `security::redact_sensitive_text`.
pub fn read_recent_redacted_log_lines(
    logs_dir: &Path,
    max_lines: usize,
    max_bytes: usize,
) -> Vec<String> {
    flush_file_logging();
    let Ok(entries) = fs::read_dir(logs_dir) else {
        return Vec::new();
    };

    let mut files: Vec<(PathBuf, SystemTime)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_log = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("log"));
        if !is_log {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        files.push((path, modified));
    }

    // Read chronologically so newest lines come last.
    files.sort_by_key(|(_, modified)| *modified);

    let mut all_lines: Vec<String> = Vec::new();
    for (path, _) in files {
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        for line in content.lines() {
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                continue;
            }
            all_lines.push(security::redact_sensitive_text(trimmed));
        }
    }

    if all_lines.len() > max_lines {
        let start = all_lines.len() - max_lines;
        all_lines = all_lines.split_off(start);
    }

    // Trim oldest lines if total byte budget is exceeded.
    let mut selected: Vec<String> = Vec::new();
    let mut used = 0usize;
    for line in all_lines.into_iter().rev() {
        let cost = line.len() + 1;
        if !selected.is_empty() && used.saturating_add(cost) > max_bytes {
            break;
        }
        used = used.saturating_add(cost);
        selected.push(line);
    }
    selected.reverse();
    selected
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_rotating_sink_redacts_secrets_and_prunes_size_budget() {
        let tmp = TempDir::new().unwrap();
        let logs_dir = tmp.path().join("logs");

        // Tight limits: 200 bytes/file, 450 bytes total
        let mut sink = RotatingFileSink::open_with_limits(&logs_dir, 200, 450, 7 * 86_400).unwrap();
        for i in 0..12 {
            sink.write_line(&format!(
                "line={} user=C:\\Users\\Alice\\test.mp4 url=https://a.test/x?token=secret{} email=a{}@b.com",
                i, i, i
            ))
            .unwrap();
        }
        drop(sink);

        let mut total_size = 0u64;
        let mut file_count = 0usize;
        for entry in fs::read_dir(&logs_dir).unwrap().flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("log") {
                file_count += 1;
                let meta = entry.metadata().unwrap();
                total_size += meta.len();
                let content = fs::read_to_string(&p).unwrap();
                assert!(!content.contains("Alice"));
                assert!(!content.contains("secret"));
                assert!(!content.contains('@'));
            }
        }
        assert!(file_count >= 1);
        assert!(
            total_size <= 700,
            "Rotated archives should remain bounded (got {})",
            total_size
        );

        let tail = read_recent_redacted_log_lines(&logs_dir, 5, 4096);
        assert!(!tail.is_empty());
        assert!(tail.len() <= 5);
    }
}
