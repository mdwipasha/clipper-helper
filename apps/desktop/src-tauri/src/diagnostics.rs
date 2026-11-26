//! Offline redacted diagnostics report builder (SECURITY §11–§12; M1.1d-lite Part 4d–e).
//!
//! Produces a chat-friendly plain-text / Markdown report in two strict size budgets:
//! - Clipboard preview: `<= 100 KB` (drops oldest detail/log lines first with an explicit notice).
//! - File export (`.txt`): `<= 1 MB`.
//!
//! Zero network calls are ever made; nothing leaves the machine unless the user explicitly copies
//! or saves the report.

use std::fmt::Write as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::crash::CrashRecord;
use crate::encoder::EncoderStatusSnapshot;
use crate::hardware::HardwareProfile;
use crate::logging;
use crate::resolver::ComponentDiagnosticsSnapshot;
use crate::security;
use crate::speech::SpeechModelState;
use crate::store::{ErrorEventRow, LocalStore, SafeJobSummary, StageStatMedian};

pub const RINGKASAN_MAX_BYTES: usize = 4096;
pub const CLIPBOARD_REPORT_MAX_BYTES: usize = 100 * 1024;
pub const FILE_REPORT_MAX_BYTES: usize = 1024 * 1024;
pub const REQUIRED_DISK_MIN_BUFFER_BYTES: u64 = 1024 * 1024 * 1024; // 1 GB

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct DiagnosticsReportBundle {
    pub report_id: String,
    pub ringkasan_bytes: u32,
    pub preview_text: String,
    pub preview_bytes: u32,
    pub preview_truncated: bool,
    pub full_text: String,
    pub full_bytes: u32,
    pub full_truncated: bool,
    pub error_count: u32,
}

#[derive(Debug, Clone)]
pub struct DiagnosticsReportOptions {
    pub user_note: String,
    pub include_video_ids: bool,
    pub app_version: String,
    pub crash_record: Option<CrashRecord>,
}

/// Generates a unique human-readable diagnostics identifier formatted as `CH-XXXX-XXXX`.
pub fn generate_report_id() -> String {
    let raw = uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .to_ascii_uppercase();
    format!("CH-{}-{}", &raw[0..4], &raw[4..8])
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn format_bytes_gb(bytes: u64) -> String {
    let gb = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    format!("{:.2} GB", gb)
}

fn format_ram(ram_bytes: Option<u64>) -> String {
    match ram_bytes {
        Some(b) if b > 0 => format_bytes_gb(b),
        _ => "Tidak diketahui".to_string(),
    }
}

fn truncate_utf8_bytes(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// Builds Section 1 (`## RINGKASAN`), strictly enforced to `<= 4096 bytes`.
#[allow(clippy::too_many_arguments)]
fn build_ringkasan_section(
    report_id: &str,
    ts_secs: u64,
    ws_root: &Path,
    free_disk_bytes: Option<u64>,
    hw: &HardwareProfile,
    snapshot: &ComponentDiagnosticsSnapshot,
    speech_state: &SpeechModelState,
    encoder_status: &EncoderStatusSnapshot,
    recent_errors: &[ErrorEventRow],
    opts: &DiagnosticsReportOptions,
) -> String {
    let clean_note = security::redact_sensitive_text(opts.user_note.trim());
    let clean_root = security::redact_sensitive_text(&ws_root.to_string_lossy());
    let ram_str = format_ram(hw.ram_bytes_u64());
    let disk_str = match free_disk_bytes {
        Some(b) => format!("{} bebas ({})", format_bytes_gb(b), clean_root),
        None => format!("Tidak diketahui ({})", clean_root),
    };
    let ac_str = match hw.ac_power {
        Some(true) => "AC (Tersambung daya)",
        Some(false) => "Baterai",
        None => "Tidak diketahui",
    };

    let gpu_list = if hw.gpus.is_empty() {
        "Tidak terdeteksi".to_string()
    } else {
        hw.gpus
            .iter()
            .map(|g| {
                format!(
                    "{} (Driver: {})",
                    g.name,
                    g.driver_version.as_deref().unwrap_or("unknown")
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    };

    let comp_summary = snapshot
        .components
        .iter()
        .map(|c| {
            let quarantined_count = c
                .versions
                .iter()
                .filter(|v| v.status == "quarantined")
                .count();
            let ver = c.current_version.as_deref().unwrap_or("BELUM PASANG");
            let custom_tag = if c.component == "ffmpeg" && c.active_source == "custom" {
                " [Custom]"
            } else {
                ""
            };
            if quarantined_count > 0 {
                format!(
                    "{}: {}{} ({} karantina)",
                    c.component, ver, custom_tag, quarantined_count
                )
            } else {
                format!("{}: {}{}", c.component, ver, custom_tag)
            }
        })
        .collect::<Vec<_>>()
        .join(" | ");

    let encoder_label = encoder_status
        .cached_encoder
        .as_deref()
        .unwrap_or("belum diuji (lazy)");

    // Start with generous note & error detail caps, shrink only if > 4096 bytes.
    for (note_cap, err_cap) in [(600usize, 180usize), (250, 96), (80, 48)] {
        let mut s = String::new();
        let _ = writeln!(s, "# ClipperHelper Diagnostics Report");
        let _ = writeln!(s, "## RINGKASAN");
        let _ = writeln!(s, "- ID Laporan: {}", report_id);
        let _ = writeln!(s, "- Waktu (UTC epoch): {}", ts_secs);
        let _ = writeln!(s, "- Versi App: v{}", opts.app_version);

        if !clean_note.is_empty() {
            let note_single_line = clean_note.replace('\r', "").replace('\n', " ");
            let _ = writeln!(
                s,
                "- Catatan Pengguna: {}",
                truncate_utf8_bytes(&note_single_line, note_cap)
            );
        } else {
            let _ = writeln!(s, "- Catatan Pengguna: (tidak diisi)");
        }

        let _ = writeln!(
            s,
            "- Windows: {} {} (Build {}) | Edisi N/KN: {}",
            hw.windows_product,
            hw.windows_display_version.as_deref().unwrap_or("-"),
            hw.windows_build,
            if hw.is_windows_n_kn { "Ya" } else { "Tidak" }
        );
        let _ = writeln!(
            s,
            "- CPU: {} ({}C/{}T) | RAM: {}",
            hw.cpu_model, hw.physical_cores, hw.logical_cores, ram_str
        );
        let _ = writeln!(s, "- GPU: {}", truncate_utf8_bytes(&gpu_list, 280));
        let _ = writeln!(s, "- Disk DataRoot: {}", disk_str);
        let _ = writeln!(s, "- Daya: {} | Plan: {}", ac_str, hw.power_plan);
        let _ = writeln!(s, "- Komponen: {}", truncate_utf8_bytes(&comp_summary, 420));
        let _ = writeln!(
            s,
            "- Model & Encoder: {} ({}) | Encoder: {} ({} fallback)",
            speech_state.active_model.as_str(),
            if speech_state.is_user_override {
                "pilihan"
            } else {
                "otomatis"
            },
            encoder_label,
            encoder_status.fallback_history.len()
        );

        let top_errors: Vec<&ErrorEventRow> = recent_errors.iter().take(5).collect();
        if top_errors.is_empty() {
            let _ = writeln!(s, "- 5 Error Terakhir: tidak ada");
        } else {
            let _ = writeln!(s, "- 5 Error Terakhir ({} tercatat):", recent_errors.len());
            for err in top_errors {
                let short_detail = truncate_utf8_bytes(
                    &security::redact_sensitive_text(&err.detail_redacted)
                        .replace(['\r', '\n'], " "),
                    err_cap,
                );
                let _ = writeln!(
                    s,
                    "  - [{}] ts={} stage={} | {}",
                    err.code, err.ts as u64, err.stage, short_detail
                );
            }
        }

        if s.len() <= RINGKASAN_MAX_BYTES {
            return security::redact_report_envelope(&s);
        }
    }

    // Extreme fallback truncation should fixed fields themselves exceed 4096 bytes.
    let hard = truncate_utf8_bytes(
        &format!(
            "# ClipperHelper Diagnostics Report\n## RINGKASAN\n- ID Laporan: {}\n- Versi App: v{}\n- Catatan: {}\n",
            report_id,
            opts.app_version,
            truncate_utf8_bytes(&clean_note.replace('\n', " "), 200)
        ),
        RINGKASAN_MAX_BYTES,
    );
    security::redact_report_envelope(&hard)
}

#[allow(clippy::too_many_arguments)]
fn assemble_detail_section(
    crash_record: Option<&CrashRecord>,
    snapshot: &ComponentDiagnosticsSnapshot,
    safe_settings: &std::collections::BTreeMap<String, String>,
    stage_medians: &[StageStatMedian],
    jobs: &[SafeJobSummary],
    errors: &[ErrorEventRow],
    log_lines: &[String],
    include_stderr_tails: bool,
    truncation_notice: Option<&str>,
) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "\n## DETAIL");
    if let Some(notice) = truncation_notice {
        let _ = writeln!(s, "> {}", notice);
    }

    if let Some(crash) = crash_record {
        let _ = writeln!(s, "\n### Crash / Unclean Shutdown Terakhir");
        let _ = writeln!(
            s,
            "- Waktu: {} | Versi: {} | Lokasi: {}",
            crash.timestamp as u64,
            crash.app_version,
            crash.location.as_deref().unwrap_or("unknown")
        );
        let _ = writeln!(s, "- Pesan: {}", crash.message_redacted);
        if !crash.backtrace_redacted.trim().is_empty() {
            let _ = writeln!(s, "```text\n{}\n```", crash.backtrace_redacted.trim());
        }
    }

    let _ = writeln!(s, "\n### Laporan Kapabilitas Komponen");
    for comp in &snapshot.components {
        let _ = writeln!(
            s,
            "- {} (aktif: {}, sumber: {})",
            comp.component,
            comp.current_version.as_deref().unwrap_or("tidak ada"),
            comp.active_source
        );
        for v in &comp.versions {
            let cap_status = v
                .capability_report
                .as_ref()
                .map(|r| match r.status {
                    crate::capabilities::CapabilityStatus::Pass => "pass",
                    crate::capabilities::CapabilityStatus::Warn => "warn",
                    crate::capabilities::CapabilityStatus::Fail => "fail",
                })
                .unwrap_or("unchecked");
            let q_note = v
                .quarantine_reason
                .as_deref()
                .map(|r| format!(" | karantina: {}", security::redact_sensitive_text(r)))
                .unwrap_or_default();
            let _ = writeln!(
                s,
                "  - ver={} status={} cap={}{}",
                v.version, v.status, cap_status, q_note
            );
            if let Some(rep) = &v.capability_report {
                if let Some(ff) = &rep.ffmpeg {
                    let _ = writeln!(
                        s,
                        "    ffmpeg: license={} ffprobe={} encoders=[{}]",
                        ff.effective_license,
                        ff.ffprobe_version,
                        ff.hardware_encoders_available.join(", ")
                    );
                }
                if !rep.reasons.is_empty() {
                    let _ = writeln!(
                        s,
                        "    reasons: {}",
                        security::redact_sensitive_text(&rep.reasons.join("; "))
                    );
                }
            }
        }
    }
    if snapshot.custom_ffmpeg.enabled {
        let _ = writeln!(
            s,
            "- Custom FFmpeg aktif: {} (fallback={})",
            snapshot
                .custom_ffmpeg
                .redacted_path
                .as_deref()
                .unwrap_or("(redacted)"),
            snapshot.custom_ffmpeg.fallback_active
        );
    }

    let _ = writeln!(s, "\n### Pengaturan (Allowlist Non-Sensitif)");
    if safe_settings.is_empty() {
        let _ = writeln!(s, "(belum ada pengaturan tersimpan)");
    } else {
        for (k, v) in safe_settings {
            let _ = writeln!(s, "- {} = {}", k, security::redact_report_envelope(v));
        }
    }

    let _ = writeln!(s, "\n### Median Statistik Tahap (stage_stats)");
    if stage_medians.is_empty() {
        let _ = writeln!(
            s,
            "(belum ada riwayat stage_stats; menggunakan ETA prior konservatif)"
        );
    } else {
        for m in stage_medians {
            let _ = writeln!(
                s,
                "- {}.{} [{}]: median {:.3} unit/s ({} sampel)",
                m.stage, m.variant, m.device_profile, m.median_rate_units_per_sec, m.sample_count
            );
        }
    }

    let _ = writeln!(s, "\n### Ringkasan Job Terakhir ({})", jobs.len());
    if jobs.is_empty() {
        let _ = writeln!(s, "(belum ada job)");
    } else {
        for j in jobs {
            let vid = j.video_id.as_deref().unwrap_or("-");
            let err_tag = j.error_code.as_deref().unwrap_or("-");
            let _ = writeln!(
                s,
                "- id={} kind={} status={} stage={} video_id={} err={}",
                j.id,
                j.kind,
                j.status,
                j.stage.as_deref().unwrap_or("-"),
                vid,
                err_tag
            );
        }
    }

    let _ = writeln!(s, "\n### Riwayat Error ({})", errors.len());
    if errors.is_empty() {
        let _ = writeln!(s, "(tidak ada error tercatat)");
    } else {
        for err in errors {
            let _ = writeln!(
                s,
                "- [{}] ts={} stage={} key={}: {}",
                err.code,
                err.ts as u64,
                err.stage,
                err.message_key,
                security::redact_sensitive_text(&err.detail_redacted)
            );
            if include_stderr_tails && !err.stderr_tail_redacted.trim().is_empty() {
                let tail = security::redact_sensitive_text(&err.stderr_tail_redacted);
                let _ = writeln!(s, "  stderr: {}", tail.replace('\n', " | "));
            }
        }
    }

    let _ = writeln!(s, "\n### Log Teredaksi Terbaru ({} baris)", log_lines.len());
    if log_lines.is_empty() {
        let _ = writeln!(s, "(belum ada baris log)");
    } else {
        let _ = writeln!(s, "```text");
        for line in log_lines {
            let _ = writeln!(s, "{}", security::redact_sensitive_text(line));
        }
        let _ = writeln!(s, "```");
    }

    s
}

#[allow(clippy::too_many_arguments)]
fn fit_report_to_budget(
    ringkasan: &str,
    crash_record: Option<&CrashRecord>,
    snapshot: &ComponentDiagnosticsSnapshot,
    safe_settings: &std::collections::BTreeMap<String, String>,
    stage_medians: &[StageStatMedian],
    all_jobs: &[SafeJobSummary],
    all_errors: &[ErrorEventRow],
    all_logs: &[String],
    max_bytes: usize,
) -> (String, bool) {
    let notice_msg = format!(
        "DIPOTONG OTOMATIS: detail & log terlama dihapus agar muat batas ~{} KB.",
        max_bytes / 1024
    );

    let full_detail = assemble_detail_section(
        crash_record,
        snapshot,
        safe_settings,
        stage_medians,
        all_jobs,
        all_errors,
        all_logs,
        true,
        None,
    );
    let initial = security::redact_report_envelope(&format!("{}{}", ringkasan, full_detail));
    if initial.len() <= max_bytes {
        return (initial, false);
    }

    // 1. Progressively drop oldest log lines first (logs are ordered chronological oldest -> newest).
    let mut log_slice = all_logs;
    let mut err_slice = all_errors;
    let mut job_slice = all_jobs;
    let mut include_stderr = true;

    let total_logs = log_slice.len();
    for keep_logs in [
        total_logs / 2,
        total_logs / 4,
        40.min(total_logs),
        10.min(total_logs),
        0,
    ] {
        log_slice = &all_logs[all_logs.len().saturating_sub(keep_logs)..];
        let cand_detail = assemble_detail_section(
            crash_record,
            snapshot,
            safe_settings,
            stage_medians,
            job_slice,
            err_slice,
            log_slice,
            include_stderr,
            Some(&notice_msg),
        );
        let cand = security::redact_report_envelope(&format!("{}{}", ringkasan, cand_detail));
        if cand.len() <= max_bytes {
            return (cand, true);
        }
    }

    // 2. Drop stderr tails, then older errors (errors are newest-first), then older jobs.
    include_stderr = false;
    for err_cap in [all_errors.len(), 15, 5, 1, 0] {
        err_slice = &all_errors[..all_errors.len().min(err_cap)];
        for job_cap in [all_jobs.len(), 5, 0] {
            job_slice = &all_jobs[..all_jobs.len().min(job_cap)];
            let cand_detail = assemble_detail_section(
                crash_record,
                snapshot,
                safe_settings,
                stage_medians,
                job_slice,
                err_slice,
                &[],
                include_stderr,
                Some(&notice_msg),
            );
            let cand = security::redact_report_envelope(&format!("{}{}", ringkasan, cand_detail));
            if cand.len() <= max_bytes {
                return (cand, true);
            }
        }
    }

    // 3. Last resort: drop crash backtrace / trim final string cleanly within max_bytes.
    let minimal_detail = assemble_detail_section(
        None,
        snapshot,
        safe_settings,
        stage_medians,
        &[],
        &[],
        &[],
        false,
        Some(&notice_msg),
    );
    let combined = security::redact_report_envelope(&format!("{}{}", ringkasan, minimal_detail));
    if combined.len() <= max_bytes {
        return (combined, true);
    }

    let suffix = format!("\n\n> [{}]\n", notice_msg);
    let head_budget = max_bytes.saturating_sub(suffix.len());
    let mut truncated = truncate_utf8_bytes(&combined, head_budget);
    // Avoid `…` pushing slightly over max_bytes if right at limit:
    while truncated.len().saturating_add(suffix.len()) > max_bytes && !truncated.is_empty() {
        truncated.pop();
    }
    truncated.push_str(&suffix);
    (truncated, true)
}

/// Builds both the clipboard (`<= 100 KB`) and file (`<= 1 MB`) redacted diagnostics report.
#[allow(clippy::too_many_arguments)]
pub fn build_diagnostics_report(
    ws_root: &Path,
    store: &LocalStore,
    hw: &HardwareProfile,
    snapshot: &ComponentDiagnosticsSnapshot,
    speech_state: &SpeechModelState,
    encoder_status: &EncoderStatusSnapshot,
    opts: &DiagnosticsReportOptions,
) -> DiagnosticsReportBundle {
    let report_id = generate_report_id();
    let ts_secs = unix_now_secs();
    let free_disk = security::get_free_disk_space(ws_root).ok();

    let all_errors_file = store.list_error_events(None, 200).unwrap_or_default();
    let all_errors_clip: Vec<ErrorEventRow> = all_errors_file.iter().take(30).cloned().collect();
    let safe_settings = store.export_safe_settings_allowlist().unwrap_or_default();
    let stage_medians = store.list_stage_stat_medians().unwrap_or_default();
    let jobs = store
        .list_recent_job_summaries(20, opts.include_video_ids)
        .unwrap_or_default();
    let logs_dir = ws_root.join("logs");
    let logs_file = logging::read_recent_redacted_log_lines(&logs_dir, 800, 512 * 1024);
    let logs_clip: Vec<String> = logs_file
        .iter()
        .skip(logs_file.len().saturating_sub(150))
        .cloned()
        .collect();

    let ringkasan = build_ringkasan_section(
        &report_id,
        ts_secs,
        ws_root,
        free_disk,
        hw,
        snapshot,
        speech_state,
        encoder_status,
        &all_errors_file,
        opts,
    );

    let (preview_text, preview_truncated) = fit_report_to_budget(
        &ringkasan,
        opts.crash_record.as_ref(),
        snapshot,
        &safe_settings,
        &stage_medians,
        &jobs,
        &all_errors_clip,
        &logs_clip,
        CLIPBOARD_REPORT_MAX_BYTES,
    );

    let (full_text, full_truncated) = fit_report_to_budget(
        &ringkasan,
        opts.crash_record.as_ref(),
        snapshot,
        &safe_settings,
        &stage_medians,
        &jobs,
        &all_errors_file,
        &logs_file,
        FILE_REPORT_MAX_BYTES,
    );

    DiagnosticsReportBundle {
        report_id,
        ringkasan_bytes: ringkasan.len() as u32,
        preview_bytes: preview_text.len() as u32,
        preview_text,
        preview_truncated,
        full_bytes: full_text.len() as u32,
        full_text,
        full_truncated,
        error_count: all_errors_file.len() as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::get_encoder_status;
    use crate::hardware::GpuAdapterInfo;
    use crate::installer::InUseRegistry;
    use crate::resolver::ComponentResolver;
    use crate::speech::resolve_speech_model_state;
    use crate::store::NewErrorEvent;
    use tempfile::TempDir;

    fn mock_hw() -> HardwareProfile {
        let gpus = vec![GpuAdapterInfo {
            name: "NVIDIA GeForce RTX 3060".into(),
            driver_version: Some("32.0.15.6636".into()),
            vendor: Some("NVIDIA".into()),
        }];
        HardwareProfile {
            cpu_model: "AMD Ryzen 5 5600H".into(),
            physical_cores: 6,
            logical_cores: 12,
            ram_bytes: Some((16u64 * 1024 * 1024 * 1024) as f64),
            windows_product: "Windows 11 Home".into(),
            windows_display_version: Some("24H2".into()),
            windows_build: "26100.123".into(),
            is_windows_n_kn: false,
            ac_power: Some(true),
            power_plan: "Balanced".into(),
            fingerprint: crate::hardware::compute_hardware_fingerprint(&gpus, "26100.123"),
            gpus,
        }
    }

    #[test]
    fn test_report_enforces_4kb_ringkasan_100kb_clipboard_and_1mb_file_caps_with_redaction_scan() {
        let tmp = TempDir::new().unwrap();
        let ws = security::ensure_workspace_with_root(tmp.path()).unwrap();
        let store = LocalStore::open(&ws.join("clipper.db")).unwrap();

        // Populate realistic dirty entries: secret token, email, username in path, job with URL vs 11-char ID
        store.set_setting("language", "id").unwrap();
        store
            .set_setting("supabase_service_role_key", "NEVER_SHOULD_APPEAR_SECRET")
            .unwrap();

        // Write oversized log lines (> 150 KB) to exercise clipboard ~100 KB truncation + notice
        let big_log_line = "INFO stage=transcribe progress=45% buffer=0123456789abcdef0123456789abcdef user=C:\\Users\\RahmatClipper\\clip.mp4 email=rahmat@domain.id token=tok_secret_99 Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxIn0.sig1234567890 ".repeat(10);
        let mut big_log_content = String::new();
        for i in 0..140 {
            big_log_content.push_str(&format!("line={} {}\n", i, big_log_line));
        }
        std::fs::write(ws.join("logs/clipper.log"), &big_log_content).unwrap();

        store
            .record_error_event(NewErrorEvent {
                code: "E_YT_EXTRACT_FAILED".into(),
                stage: "download".into(),
                job_id: Some("job-99".into()),
                message_key: "err.E_YT_EXTRACT_FAILED.body".into(),
                detail: "Failed fetching https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=12s#frag for C:\\Users\\RahmatClipper\\out.mp4".into(),
                stderr_tail: "ERROR: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxIn0.sig1234567890 rahmat@domain.id".into(),
                app_version: "0.1.0".into(),
                component_versions_json: "{}".into(),
            })
            .unwrap();

        let hw = mock_hw();
        let resolver = ComponentResolver::new(ws.clone(), store.clone(), InUseRegistry::new());
        let snapshot = resolver.diagnostics_snapshot().unwrap();
        let speech_state = resolve_speech_model_state(&store, hw.ram_bytes_u64()).unwrap();
        let encoder_status = get_encoder_status(&store, &hw).unwrap();

        let bundle = build_diagnostics_report(
            &ws,
            &store,
            &hw,
            &snapshot,
            &speech_state,
            &encoder_status,
            &DiagnosticsReportOptions {
                user_note: "Sedang mencoba render klip email=rahmat@domain.id token=abc12345"
                    .into(),
                include_video_ids: true,
                app_version: "0.1.0".into(),
                crash_record: None,
            },
        );

        assert!(bundle.report_id.starts_with("CH-"));
        assert!(
            bundle.ringkasan_bytes as usize <= RINGKASAN_MAX_BYTES,
            "RINGKASAN exceeded 4 KB: {}",
            bundle.ringkasan_bytes
        );
        assert!(
            bundle.preview_bytes as usize <= CLIPBOARD_REPORT_MAX_BYTES,
            "Clipboard preview exceeded 100 KB: {}",
            bundle.preview_bytes
        );
        assert!(bundle.preview_truncated);
        assert!(bundle.preview_text.contains("DIPOTONG OTOMATIS"));
        assert!(bundle.full_bytes as usize <= FILE_REPORT_MAX_BYTES);

        // GPU driver version must remain legible in Section 1
        assert!(bundle.preview_text.contains("32.0.15.6636"));

        // Scan entire report for forbidden secret/PII patterns!
        for combined in [&bundle.preview_text, &bundle.full_text] {
            for forbidden in [
                "RahmatClipper",
                "@",
                "Bearer ",
                "token=",
                "NEVER_SHOULD_APPEAR_SECRET",
                "eyJhbGci",
                "t=12s",
                "#frag",
            ] {
                assert!(
                    !combined.contains(forbidden),
                    "Diagnostics report leaked forbidden pattern '{}'",
                    forbidden
                );
            }
        }
    }

    #[test]
    fn test_video_id_toggle_controls_11_char_id_visibility() {
        let tmp = TempDir::new().unwrap();
        let ws = security::ensure_workspace_with_root(tmp.path()).unwrap();
        let store = LocalStore::open(&ws.join("clipper.db")).unwrap();

        // Seed a project + job with an 11-char video ID via SQLite
        {
            let conn = rusqlite::Connection::open(store.path()).unwrap();
            conn.execute(
                "INSERT INTO projects (id, source_video_id, created_at, updated_at) VALUES ('p1', 'dQw4w9WgXcQ', 100, 100)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO jobs (id, project_id, kind, status, created_at) VALUES ('j1', 'p1', 'generate', 'succeeded', 100)",
                [],
            )
            .unwrap();
        }

        let hw = mock_hw();
        let resolver = ComponentResolver::new(ws.clone(), store.clone(), InUseRegistry::new());
        let snapshot = resolver.diagnostics_snapshot().unwrap();
        let speech_state = resolve_speech_model_state(&store, hw.ram_bytes_u64()).unwrap();
        let encoder_status = get_encoder_status(&store, &hw).unwrap();

        let with_id = build_diagnostics_report(
            &ws,
            &store,
            &hw,
            &snapshot,
            &speech_state,
            &encoder_status,
            &DiagnosticsReportOptions {
                user_note: String::new(),
                include_video_ids: true,
                app_version: "0.1.0".into(),
                crash_record: None,
            },
        );
        assert!(with_id.preview_text.contains("video_id=dQw4w9WgXcQ"));

        let without_id = build_diagnostics_report(
            &ws,
            &store,
            &hw,
            &snapshot,
            &speech_state,
            &encoder_status,
            &DiagnosticsReportOptions {
                user_note: String::new(),
                include_video_ids: false,
                app_version: "0.1.0".into(),
                crash_record: None,
            },
        );
        assert!(!without_id.preview_text.contains("dQw4w9WgXcQ"));
    }
}
