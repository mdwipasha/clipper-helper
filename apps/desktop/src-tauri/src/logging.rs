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
use tracing::level_filters::LevelFilter;
use tracing::Subscriber;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

use crate::security;

pub const LOG_RETENTION_DAYS: u64 = 7;
pub const LOG_RETENTION_SECS: u64 = LOG_RETENTION_DAYS * 86_400;
pub const LOG_MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024; // 20 MB
pub const LOG_MAX_SINGLE_FILE_BYTES: u64 = 4 * 1024 * 1024; // Rotate every 4 MB
pub const REPORT_MAX_LOG_LINES: usize = 80;
pub const DEBUG_LOG_FILTER_ENV: &str = "CLIPPER_LOG";

fn parse_level_filter(raw: &str) -> Option<LevelFilter> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "trace" => Some(LevelFilter::TRACE),
        "debug" => Some(LevelFilter::DEBUG),
        "info" => Some(LevelFilter::INFO),
        "warn" | "warning" => Some(LevelFilter::WARN),
        "error" => Some(LevelFilter::ERROR),
        "off" => Some(LevelFilter::OFF),
        _ => None,
    }
}

/// Builds the default structured tracing filter:
/// - `info` for our crates (`clipper_helper_desktop`, `clipper_helper_desktop_lib`)
/// - `warn` for dependencies (`hyper`, `hyper_util`, `reqwest`, `tao`, `wry`, `tokio`, default)
/// - `TRACE` / `DEBUG` enabled ONLY via `CLIPPER_LOG` env var in debug builds.
pub fn build_log_targets(debug_override_env: Option<&str>) -> Targets {
    let mut targets = Targets::new()
        .with_default(LevelFilter::WARN)
        .with_target("hyper", LevelFilter::WARN)
        .with_target("hyper_util", LevelFilter::WARN)
        .with_target("reqwest", LevelFilter::WARN)
        .with_target("tao", LevelFilter::WARN)
        .with_target("wry", LevelFilter::WARN)
        .with_target("tokio", LevelFilter::WARN)
        .with_target("clipper_helper_desktop_lib", LevelFilter::INFO)
        .with_target("clipper_helper_desktop", LevelFilter::INFO);

    #[cfg(debug_assertions)]
    if let Some(raw) = debug_override_env {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            if let Some(lvl) = parse_level_filter(trimmed) {
                targets = targets
                    .with_target("clipper_helper_desktop_lib", lvl)
                    .with_target("clipper_helper_desktop", lvl);
            } else {
                for part in trimmed.split(',') {
                    let p = part.trim();
                    if let Some((k, v)) = p.split_once('=') {
                        if let Some(lvl) = parse_level_filter(v) {
                            targets = targets.with_target(k.trim().to_string(), lvl);
                        }
                    } else if let Some(lvl) = parse_level_filter(p) {
                        targets = targets.with_default(lvl);
                    }
                }
            }
        }
    }

    #[cfg(not(debug_assertions))]
    let _ = debug_override_env;

    targets
}

/// Convenience wrapper reading `CLIPPER_LOG` in debug builds only.
pub fn build_runtime_log_targets() -> Targets {
    #[cfg(debug_assertions)]
    {
        let env_val = std::env::var(DEBUG_LOG_FILTER_ENV).ok();
        build_log_targets(env_val.as_deref())
    }
    #[cfg(not(debug_assertions))]
    {
        build_log_targets(None)
    }
}

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

/// Formats Unix epoch seconds as strict ISO-8601 UTC (`YYYY-MM-DDTHH:MM:SSZ`).
pub fn format_utc_iso8601(epoch_secs: u64) -> String {
    let days_since_epoch = (epoch_secs / 86_400) as i64;
    let rem = epoch_secs % 86_400;
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;

    // Howard Hinnant civil-from-days algorithm (Euclidean calendar conversion).
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + if month <= 2 { 1 } else { 0 };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, minute, second
    )
}

fn format_utc_timestamp(epoch_secs: u64) -> String {
    format_utc_iso8601(epoch_secs)
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

/// Parses `[timestamp] LEVEL target: message` into `(level, target, body_without_ts)`.
fn parse_log_parts(line: &str) -> Option<(&str, &str, &str)> {
    let trimmed = line.trim();
    let rest = if let Some(after_open) = trimmed.strip_prefix('[') {
        let close_idx = after_open.find(']')?;
        after_open[close_idx + 1..].trim_start()
    } else {
        trimmed
    };
    let mut split = rest.splitn(3, ' ');
    let level = split.next()?;
    let target_raw = split.next()?;
    let target = target_raw.trim_end_matches(':');
    if !matches!(level, "TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR") {
        return None;
    }
    Some((level, target, rest))
}

/// Returns true iff a log line belongs in Diagnostics reports:
/// all `WARN`/`ERROR` lines, plus `INFO` lines emitted by our crates only.
pub fn is_report_eligible_log_line(line: &str) -> bool {
    let Some((level, target, _)) = parse_log_parts(line) else {
        return false;
    };
    match level {
        "ERROR" | "WARN" => true,
        "INFO" => target.starts_with("clipper_helper_desktop"),
        _ => false,
    }
}

/// Filters log lines to report-eligible entries, collapses consecutive duplicate bodies with `(×N)`,
/// keeps at most `max_lines` newest entries, and trims oldest entries if over `max_bytes`.
pub fn select_report_log_lines(
    raw_lines: &[String],
    max_lines: usize,
    max_bytes: usize,
) -> Vec<String> {
    struct Collapsed {
        rendered: String,
        dedup_key: String,
        count: usize,
    }

    let mut collapsed: Vec<Collapsed> = Vec::new();
    for raw in raw_lines {
        let clean = security::redact_sensitive_text(raw.trim_end());
        if clean.is_empty() || !is_report_eligible_log_line(&clean) {
            continue;
        }
        let dedup_key = parse_log_parts(&clean)
            .map(|(_, _, body)| body.to_string())
            .unwrap_or_else(|| clean.clone());

        if let Some(last) = collapsed.last_mut() {
            if last.dedup_key == dedup_key {
                last.count = last.count.saturating_add(1);
                continue;
            }
        }
        collapsed.push(Collapsed {
            rendered: clean,
            dedup_key,
            count: 1,
        });
    }

    let mut formatted: Vec<String> = collapsed
        .into_iter()
        .map(|entry| {
            if entry.count > 1 {
                format!("{} (×{})", entry.rendered, entry.count)
            } else {
                entry.rendered
            }
        })
        .collect();

    if formatted.len() > max_lines {
        let start = formatted.len() - max_lines;
        formatted = formatted.split_off(start);
    }

    let mut selected: Vec<String> = Vec::new();
    let mut used = 0usize;
    for line in formatted.into_iter().rev() {
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

/// Reads up to `max_lines` and `max_bytes` of the most recent report-eligible redacted log lines
/// from `<logs_dir>/*.log`.
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
            if !trimmed.is_empty() {
                all_lines.push(trimmed.to_string());
            }
        }
    }

    select_report_log_lines(&all_lines, max_lines, max_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_log_filter_allows_our_info_and_blocks_dependency_info() {
        let default_targets = build_log_targets(None);
        assert!(default_targets
            .would_enable("clipper_helper_desktop_lib::probe", &tracing::Level::INFO));
        assert!(default_targets.would_enable("clipper_helper_desktop", &tracing::Level::INFO));
        assert!(!default_targets.would_enable("clipper_helper_desktop_lib", &tracing::Level::DEBUG));
        assert!(!default_targets.would_enable("clipper_helper_desktop_lib", &tracing::Level::TRACE));

        for dep in [
            "hyper::proto::h1",
            "hyper_util::client",
            "reqwest::connect",
            "tao::platform_impl",
            "wry::webview2",
            "tokio::runtime",
            "other_crate",
        ] {
            assert!(
                !default_targets.would_enable(dep, &tracing::Level::INFO),
                "Dependency {} INFO should be suppressed by default",
                dep
            );
            assert!(
                default_targets.would_enable(dep, &tracing::Level::WARN),
                "Dependency {} WARN should be enabled",
                dep
            );
        }

        // In debug builds, CLIPPER_LOG=trace enables TRACE/DEBUG on our crates; ignored in release.
        let with_env = build_log_targets(Some("trace"));
        if cfg!(debug_assertions) {
            assert!(with_env.would_enable("clipper_helper_desktop_lib", &tracing::Level::TRACE));
            assert!(with_env.would_enable("clipper_helper_desktop_lib", &tracing::Level::DEBUG));
        } else {
            assert!(!with_env.would_enable("clipper_helper_desktop_lib", &tracing::Level::TRACE));
            assert!(!with_env.would_enable("clipper_helper_desktop_lib", &tracing::Level::DEBUG));
        }
    }

    #[test]
    fn test_report_log_selection_filters_dep_info_caps_80_and_collapses_duplicates() {
        let mut lines = vec![
            "[2026-11-26T10:00:00Z] INFO hyper::client: noisy info".to_string(),
            "[2026-11-26T10:00:01Z] INFO reqwest::connect: noisy connect".to_string(),
            "[2026-11-26T10:00:02Z] DEBUG clipper_helper_desktop_lib: verbose debug".to_string(),
            "[2026-11-26T10:00:03Z] TRACE tokio::task: verbose trace".to_string(),
            "[2026-11-26T10:00:04Z] WARN reqwest::retry: retrying transfer".to_string(),
            "[2026-11-26T10:00:05Z] WARN reqwest::retry: retrying transfer".to_string(),
            "[2026-11-26T10:00:06Z] WARN reqwest::retry: retrying transfer".to_string(),
        ];
        for i in 0..90 {
            lines.push(format!(
                "[2026-11-26T10:01:{:02}Z] INFO clipper_helper_desktop_lib::app: step {}",
                i % 60,
                i
            ));
        }
        lines.push(
            "[2026-11-26T10:02:00Z] ERROR clipper_helper_desktop_lib::updater: boom".to_string(),
        );

        let selected = select_report_log_lines(&lines, REPORT_MAX_LOG_LINES, 128 * 1024);
        assert_eq!(selected.len(), 80, "must cap at newest 80 lines");
        assert!(!selected.iter().any(|l| l.contains("noisy info")
            || l.contains("verbose debug")
            || l.contains("verbose trace")));
        assert!(selected
            .last()
            .unwrap()
            .contains("ERROR clipper_helper_desktop_lib::updater: boom"));

        // With higher line limit, verify consecutive duplicate collapse
        let collapsed = select_report_log_lines(&lines[..7], REPORT_MAX_LOG_LINES, 64 * 1024);
        assert_eq!(collapsed.len(), 1);
        assert_eq!(
            collapsed[0],
            "[2026-11-26T10:00:04Z] WARN reqwest::retry: retrying transfer (×3)"
        );
    }

    #[test]
    fn test_rotating_sink_redacts_secrets_and_prunes_size_budget() {
        let tmp = TempDir::new().unwrap();
        let logs_dir = tmp.path().join("logs");

        // Tight limits: 200 bytes/file, 450 bytes total
        let mut sink = RotatingFileSink::open_with_limits(&logs_dir, 200, 450, 7 * 86_400).unwrap();
        for i in 0..12 {
            sink.write_line(&format!(
                "[2026-11-26T10:00:00Z] INFO clipper_helper_desktop_lib: line={} user=C:\\Users\\Alice\\test.mp4 url=https://a.test/x?token=secret{} email=a{}@b.com",
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
