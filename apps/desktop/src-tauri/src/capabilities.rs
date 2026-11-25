//! Sidecar capability verification executed via the production `SidecarSupervisor`
//! (ARCHITECTURE §15; SECURITY §9; docs/compliance/FFMPEG.md; docs/benchmarks/S5-ffmpeg.md).
//!
//! Spawns sidecar binaries only through `SidecarSupervisor` with strict timeouts and checks:
//! - `ffmpeg` / `ffprobe`: version, build configuration (`--enable-gpl` / `--enable-nonfree`
//!   forbidden in Strict mode), forbidden encoders (`libx264`, `libx265`, `libfdk_aac`),
//!   required encoders (`aac`, `h264_mf`), hardware encoders inventory (`h264_nvenc`,
//!   `h264_qsv`, `h264_amf`, `h264_mf`), required decoders (`h264`, `hevc`, `vp9`, `av1`,
//!   `aac`, `opus`), required filters + absence of all filters in `docs/benchmarks/gpl-filters-denylist.txt`,
//!   and required protocols (`https`, `file`) and formats (`mp4`, `mov`, `matroska`, `webm`).
//! - `yt-dlp`: `--version` matches manifest version.
//! - `qjs`: evaluates trivial arithmetic expression script.
//! - `whisper-cli`: runs and reports version.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use specta::Type;
use thiserror::Error;

use crate::sidecars::{SidecarError, SidecarSpec, SidecarSupervisor};

/// Canonical GPL-only FFmpeg filters denylist embedded at compile time from `docs/benchmarks/gpl-filters-denylist.txt`.
const GPL_FILTER_DENYLIST_RAW: &str =
    include_str!("../../../../docs/benchmarks/gpl-filters-denylist.txt");

/// Timeout applied to every capability probe invocation inside `SidecarSupervisor`.
pub const CAPABILITY_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyMode {
    /// Used for manifest-distributed managed components (GPL/missing caps -> fail + quarantine).
    Strict,
    /// Used for LGPLv3 §6 user custom FFmpeg override (broken binary fails; license/missing caps warn).
    Warn,
}

impl VerifyMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            VerifyMode::Strict => "strict",
            VerifyMode::Warn => "warn",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FfmpegCapabilities {
    pub ffmpeg_version: String,
    pub ffprobe_version: String,
    pub has_gpl: bool,
    pub has_nonfree: bool,
    pub has_version3: bool,
    pub effective_license: String,
    pub forbidden_libs_present: Vec<String>,
    pub forbidden_encoders_present: Vec<String>,
    pub gpl_filters_present: Vec<String>,
    pub hardware_encoders_available: Vec<String>,
    pub required_encoders_present: Vec<String>,
    pub missing_encoders: Vec<String>,
    pub missing_decoders: Vec<String>,
    pub missing_filters: Vec<String>,
    pub missing_protocols: Vec<String>,
    pub missing_formats: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityReport {
    pub component: String,
    pub version: String,
    pub mode: String,
    pub status: CapabilityStatus,
    pub checked_at: u64,
    pub reported_version: Option<String>,
    pub reasons: Vec<String>,
    pub warnings: Vec<String>,
    pub ffmpeg: Option<FfmpegCapabilities>,
}

/// UI/IPC-safe capability report projection avoiding raw `u64` BigInt in TypeScript bindings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct CapabilityReportView {
    pub component: String,
    pub version: String,
    pub mode: String,
    pub status: CapabilityStatus,
    pub checked_at: f64,
    pub reported_version: Option<String>,
    pub reasons: Vec<String>,
    pub warnings: Vec<String>,
    pub ffmpeg: Option<FfmpegCapabilities>,
}

impl From<&CapabilityReport> for CapabilityReportView {
    fn from(r: &CapabilityReport) -> Self {
        Self {
            component: r.component.clone(),
            version: r.version.clone(),
            mode: r.mode.clone(),
            status: r.status,
            checked_at: r.checked_at as f64,
            reported_version: r.reported_version.clone(),
            reasons: r.reasons.clone(),
            warnings: r.warnings.clone(),
            ffmpeg: r.ffmpeg.clone(),
        }
    }
}

#[derive(Error, Debug)]
pub enum CapabilityError {
    #[error("Component executable missing: {0}")]
    BinaryMissing(String),

    #[error("Failed to run capability check via supervisor: {0}")]
    ExecutionFailed(String),

    #[error("Capability check timed out after {0:?}")]
    Timeout(Duration),

    #[error("Capability verification failed: {reason}")]
    CapabilityCheckFailed {
        reason: String,
        report: Box<CapabilityReport>,
    },
}

impl CapabilityError {
    pub fn error_code(&self) -> &'static str {
        match self {
            CapabilityError::BinaryMissing(_) => "E_COMPONENT_MISSING",
            _ => "E_VERIFY_FAILED",
        }
    }

    pub fn report(&self) -> Option<&CapabilityReport> {
        match self {
            CapabilityError::CapabilityCheckFailed { report, .. } => Some(report),
            _ => None,
        }
    }
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn map_sidecar_err(err: SidecarError, bin: &Path) -> CapabilityError {
    match err {
        SidecarError::BinaryNotFound(_) => {
            CapabilityError::BinaryMissing(bin.display().to_string())
        }
        SidecarError::Timeout(d) => CapabilityError::Timeout(d),
        other => CapabilityError::ExecutionFailed(other.to_string()),
    }
}

async fn run_probe(
    supervisor: &SidecarSupervisor,
    binary: &Path,
    args: &[&str],
) -> Result<String, CapabilityError> {
    let spec = SidecarSpec::new(binary, args.iter().copied()).timeout(CAPABILITY_PROBE_TIMEOUT);
    let res = supervisor
        .run_to_completion(&spec, None)
        .await
        .map_err(|e| map_sidecar_err(e, binary))?;
    Ok(String::from_utf8_lossy(&res.stdout)
        .trim_start_matches('\u{feff}')
        .to_string())
}

/// Parses filter names from `docs/benchmarks/gpl-filters-denylist.txt`.
pub fn load_gpl_denylist_filters() -> BTreeSet<String> {
    parse_gpl_denylist_filters(GPL_FILTER_DENYLIST_RAW)
}

/// Parses GPL denylist text lines, skipping `#` comments and empty lines.
pub fn parse_gpl_denylist_filters(raw: &str) -> BTreeSet<String> {
    raw.lines()
        .map(|l| l.trim_start_matches('\u{feff}').trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

fn parse_ff_version_line(output: &str, tool_prefix: &str) -> Option<String> {
    let cleaned = output.trim_start_matches('\u{feff}');
    for line in cleaned.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(tool_prefix) {
            if let Some(ver) = rest.split_whitespace().next() {
                if !ver.is_empty() {
                    return Some(ver.to_string());
                }
            }
        }
    }
    None
}

fn parse_column_list_table(output: &str, expected_flag_len: usize) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut past_separator = false;
    for raw_line in output.trim_start_matches('\u{feff}').lines() {
        let trimmed = raw_line.trim();
        if !past_separator {
            if trimmed.starts_with("---") {
                past_separator = true;
            }
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let Some(flags) = parts.next() else {
            continue;
        };
        if flags.len() != expected_flag_len {
            continue;
        }
        if let Some(name) = parts.next() {
            names.insert(name.to_string());
        }
    }
    names
}

fn parse_ff_encoders_or_decoders(output: &str) -> BTreeSet<String> {
    // Format: " V....D libx264   ..." -> 6 capability flag chars followed by name.
    parse_column_list_table(output, 6)
}

fn parse_ff_filters(output: &str) -> BTreeSet<String> {
    // Format: " TS gblur   V->V  ..." -> 3 timeline/slice/command flag chars + filter name.
    // Older/newer builds use 2 or 3 flag chars before filter name.
    let mut names = BTreeSet::new();
    let mut past_separator = false;
    for raw_line in output.trim_start_matches('\u{feff}').lines() {
        let trimmed = raw_line.trim();
        if !past_separator {
            if trimmed.starts_with("---") {
                past_separator = true;
            }
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let Some(flags) = parts.next() else {
            continue;
        };
        if !(2..=3).contains(&flags.len()) {
            continue;
        }
        if let Some(name) = parts.next() {
            names.insert(name.to_string());
        }
    }
    names
}

fn parse_ff_protocols(output: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut input = BTreeSet::new();
    let mut output_protos = BTreeSet::new();
    enum Section {
        None,
        Input,
        Output,
    }
    let mut section = Section::None;

    for raw_line in output.trim_start_matches('\u{feff}').lines() {
        let trimmed = raw_line.trim();
        if trimmed == "Input:" {
            section = Section::Input;
            continue;
        }
        if trimmed == "Output:" {
            section = Section::Output;
            continue;
        }
        if trimmed.is_empty() || trimmed.ends_with(':') {
            continue;
        }
        if let Some(proto) = trimmed.split_whitespace().next() {
            match section {
                Section::Input => {
                    input.insert(proto.to_string());
                }
                Section::Output => {
                    output_protos.insert(proto.to_string());
                }
                Section::None => {}
            }
        }
    }
    (input, output_protos)
}

fn parse_ff_formats(output: &str) -> BTreeSet<String> {
    // Format: " D   mov,mp4,m4a,3gp,3g2,mj2 QuickTime / MOV"
    // or      "  E  mp4             MP4"
    // Line can have leading spaces and combinations of D / E / d flags; parse after `---` divider.
    let mut formats = BTreeSet::new();
    let mut past_separator = false;
    for raw_line in output.trim_start_matches('\u{feff}').lines() {
        let trimmed = raw_line.trim();
        if !past_separator {
            if trimmed.starts_with("---") {
                past_separator = true;
            }
            continue;
        }
        // First 4 chars of line are format flags (e.g. " D  ", "  E ", " DE "), then format names token.
        if raw_line.len() <= 4 {
            continue;
        }
        let after_flags = raw_line[4..].trim_start();
        if let Some(names_token) = after_flags.split_whitespace().next() {
            for alias in names_token.split(',') {
                if !alias.is_empty() {
                    formats.insert(alias.to_string());
                }
            }
        }
    }
    formats
}

fn buildconf_flags(combined: &str) -> BTreeSet<String> {
    combined
        .split_whitespace()
        .filter(|tok| tok.starts_with("--"))
        .map(|tok| {
            tok.trim_end_matches(|c: char| {
                !c.is_ascii_alphanumeric() && c != '-' && c != '_' && c != '='
            })
        })
        .map(str::to_string)
        .collect()
}

/// Evaluates captured FFmpeg and ffprobe command outputs against compliance and capability rules.
#[allow(clippy::too_many_arguments)]
pub fn verify_ffmpeg_outputs(
    version_out: &str,
    buildconf_out: &str,
    encoders_out: &str,
    decoders_out: &str,
    filters_out: &str,
    protocols_out: &str,
    formats_out: &str,
    ffprobe_version_out: &str,
    version_label: &str,
    mode: VerifyMode,
) -> CapabilityReport {
    let ffmpeg_version = parse_ff_version_line(version_out, "ffmpeg version ")
        .unwrap_or_else(|| "unknown".to_string());
    let ffprobe_version = parse_ff_version_line(ffprobe_version_out, "ffprobe version ")
        .unwrap_or_else(|| "unknown".to_string());

    let combined_conf = format!("{}\n{}", version_out, buildconf_out);
    let flags = buildconf_flags(&combined_conf);
    let has_gpl = flags.contains("--enable-gpl");
    let has_nonfree = flags.contains("--enable-nonfree");
    let has_version3 = flags.contains("--enable-version3");

    let effective_license = if has_nonfree {
        "Non-free".to_string()
    } else if has_gpl && has_version3 {
        "GPLv3".to_string()
    } else if has_gpl {
        "GPL".to_string()
    } else if has_version3 {
        "LGPLv3".to_string()
    } else {
        "LGPLv2.1".to_string()
    };

    let mut findings: Vec<String> = Vec::new();
    if ffmpeg_version == "unknown" {
        findings.push("Could not parse `ffmpeg -version` banner".to_string());
    }
    if ffprobe_version == "unknown" {
        findings.push("Could not parse `ffprobe -version` banner".to_string());
    }
    if has_gpl {
        findings.push("Forbidden build flag --enable-gpl detected".to_string());
    }
    if has_nonfree {
        findings.push("Forbidden build flag --enable-nonfree detected".to_string());
    }

    let forbidden_lib_names = ["libx264", "libx265", "libfdk_aac"];
    let forbidden_libs_present: Vec<String> = forbidden_lib_names
        .iter()
        .filter(|lib| flags.contains(&format!("--enable-{}", lib)))
        .map(|s| s.to_string())
        .collect();
    if !forbidden_libs_present.is_empty() {
        findings.push(format!(
            "Forbidden GPL/nonfree libraries enabled: {}",
            forbidden_libs_present.join(", ")
        ));
    }

    let encoders = parse_ff_encoders_or_decoders(encoders_out);
    let forbidden_encoder_names = ["libx264", "libx264rgb", "libx265", "libfdk_aac"];
    let forbidden_encoders_present: Vec<String> = forbidden_encoder_names
        .iter()
        .filter(|e| encoders.contains(**e))
        .map(|s| s.to_string())
        .collect();
    if !forbidden_encoders_present.is_empty() {
        findings.push(format!(
            "Forbidden GPL/nonfree encoders compiled in build: {}",
            forbidden_encoders_present.join(", ")
        ));
    }

    let required_encoders = ["aac", "h264_mf"];
    let mut required_encoders_present = Vec::new();
    let mut missing_encoders = Vec::new();
    for enc in required_encoders {
        if encoders.contains(enc) {
            required_encoders_present.push(enc.to_string());
        } else {
            missing_encoders.push(enc.to_string());
        }
    }
    if !missing_encoders.is_empty() {
        findings.push(format!(
            "Missing required encoders: {}",
            missing_encoders.join(", ")
        ));
    }

    let hardware_candidates = ["h264_nvenc", "h264_qsv", "h264_amf", "h264_mf"];
    let hardware_encoders_available: Vec<String> = hardware_candidates
        .iter()
        .filter(|enc| encoders.contains(**enc))
        .map(|s| s.to_string())
        .collect();

    let decoders = parse_ff_encoders_or_decoders(decoders_out);
    let required_decoders = ["h264", "hevc", "vp9", "av1", "aac", "opus"];
    let missing_decoders: Vec<String> = required_decoders
        .iter()
        .filter(|d| !decoders.contains(**d))
        .map(|s| s.to_string())
        .collect();
    if !missing_decoders.is_empty() {
        findings.push(format!(
            "Missing required decoders: {}",
            missing_decoders.join(", ")
        ));
    }

    let filters = parse_ff_filters(filters_out);
    let required_filters = [
        "ass",
        "subtitles",
        "crop",
        "scale",
        "overlay",
        "split",
        "vstack",
        "hstack",
        "loudnorm",
        "ssim",
        "psnr",
        "drawtext",
    ];
    let mut missing_filters: Vec<String> = required_filters
        .iter()
        .filter(|f| !filters.contains(**f))
        .map(|s| s.to_string())
        .collect();
    if !filters.contains("gblur") && !filters.contains("avgblur") {
        missing_filters.push("gblur/avgblur".to_string());
    }
    if !missing_filters.is_empty() {
        findings.push(format!(
            "Missing required filters: {}",
            missing_filters.join(", ")
        ));
    }

    let denylist = load_gpl_denylist_filters();
    let gpl_filters_present: Vec<String> = denylist
        .iter()
        .filter(|f| filters.contains(*f))
        .cloned()
        .collect();
    if !gpl_filters_present.is_empty() {
        let preview: Vec<&str> = gpl_filters_present
            .iter()
            .take(8)
            .map(String::as_str)
            .collect();
        let suffix = if gpl_filters_present.len() > 8 {
            format!(" (+{} more)", gpl_filters_present.len() - 8)
        } else {
            String::new()
        };
        findings.push(format!(
            "GPL-only filters from denylist detected: {}{}",
            preview.join(", "),
            suffix
        ));
    }

    let (input_protos, _) = parse_ff_protocols(protocols_out);
    let required_protocols = ["https", "file"];
    let missing_protocols: Vec<String> = required_protocols
        .iter()
        .filter(|p| !input_protos.contains(**p))
        .map(|s| s.to_string())
        .collect();
    if !missing_protocols.is_empty() {
        findings.push(format!(
            "Missing required protocols: {}",
            missing_protocols.join(", ")
        ));
    }

    let formats = parse_ff_formats(formats_out);
    let required_formats = ["mp4", "mov", "matroska", "webm"];
    let missing_formats: Vec<String> = required_formats
        .iter()
        .filter(|f| !formats.contains(**f))
        .map(|s| s.to_string())
        .collect();
    if !missing_formats.is_empty() {
        findings.push(format!(
            "Missing required formats: {}",
            missing_formats.join(", ")
        ));
    }

    let (status, reasons, warnings) = if findings.is_empty() {
        (CapabilityStatus::Pass, Vec::new(), Vec::new())
    } else {
        match mode {
            VerifyMode::Strict => (CapabilityStatus::Fail, findings, Vec::new()),
            VerifyMode::Warn => (CapabilityStatus::Warn, Vec::new(), findings),
        }
    };

    CapabilityReport {
        component: "ffmpeg".to_string(),
        version: version_label.to_string(),
        mode: mode.as_str().to_string(),
        status,
        checked_at: unix_now_secs(),
        reported_version: Some(ffmpeg_version.clone()),
        reasons,
        warnings,
        ffmpeg: Some(FfmpegCapabilities {
            ffmpeg_version,
            ffprobe_version,
            has_gpl,
            has_nonfree,
            has_version3,
            effective_license,
            forbidden_libs_present,
            forbidden_encoders_present,
            gpl_filters_present,
            hardware_encoders_available,
            required_encoders_present,
            missing_encoders,
            missing_decoders,
            missing_filters,
            missing_protocols,
            missing_formats,
        }),
    }
}

/// Executes `ffmpeg` and `ffprobe` capability probes via the production `SidecarSupervisor`.
pub async fn verify_ffmpeg_capabilities(
    ffmpeg_path: &Path,
    ffprobe_path: &Path,
    version_label: &str,
    mode: VerifyMode,
    supervisor: &SidecarSupervisor,
) -> Result<CapabilityReport, CapabilityError> {
    if !ffmpeg_path.is_file() {
        return Err(CapabilityError::BinaryMissing(
            ffmpeg_path.display().to_string(),
        ));
    }
    if !ffprobe_path.is_file() {
        return Err(CapabilityError::BinaryMissing(
            ffprobe_path.display().to_string(),
        ));
    }

    let version_out = run_probe(supervisor, ffmpeg_path, &["-version"]).await?;
    let ffprobe_out = run_probe(supervisor, ffprobe_path, &["-version"]).await?;
    let buildconf_out = run_probe(supervisor, ffmpeg_path, &["-buildconf"]).await?;
    let encoders_out = run_probe(supervisor, ffmpeg_path, &["-v", "error", "-encoders"]).await?;
    let decoders_out = run_probe(supervisor, ffmpeg_path, &["-v", "error", "-decoders"]).await?;
    let filters_out = run_probe(supervisor, ffmpeg_path, &["-v", "error", "-filters"]).await?;
    let protocols_out = run_probe(supervisor, ffmpeg_path, &["-v", "error", "-protocols"]).await?;
    let formats_out = run_probe(supervisor, ffmpeg_path, &["-v", "error", "-formats"]).await?;

    let report = verify_ffmpeg_outputs(
        &version_out,
        &buildconf_out,
        &encoders_out,
        &decoders_out,
        &filters_out,
        &protocols_out,
        &formats_out,
        &ffprobe_out,
        version_label,
        mode,
    );

    if report.status == CapabilityStatus::Fail {
        return Err(CapabilityError::CapabilityCheckFailed {
            reason: report.reasons.join("; "),
            report: Box::new(report),
        });
    }

    // In Warn mode, require valid ffmpeg & ffprobe banners so arbitrary renamed EXEs are rejected.
    if let Some(ref ff) = report.ffmpeg {
        if ff.ffmpeg_version == "unknown" || ff.ffprobe_version == "unknown" {
            return Err(CapabilityError::CapabilityCheckFailed {
                reason: "Executable did not report valid ffmpeg/ffprobe version banners"
                    .to_string(),
                report: Box::new(report),
            });
        }
    }

    Ok(report)
}

/// Evaluates `yt-dlp --version` stdout against the manifest version.
pub fn verify_yt_dlp_output(
    stdout: &str,
    expected_version: &str,
    mode: VerifyMode,
) -> CapabilityReport {
    let reported = stdout
        .trim_start_matches('\u{feff}')
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();

    let expected_norm = expected_version.trim();
    let mut findings = Vec::new();
    if reported.is_empty() {
        findings.push("yt-dlp --version returned empty output".to_string());
    } else if reported != expected_norm {
        findings.push(format!(
            "yt-dlp reported version '{}' does not equal manifest version '{}'",
            reported, expected_norm
        ));
    }

    let (status, reasons, warnings) = if findings.is_empty() {
        (CapabilityStatus::Pass, Vec::new(), Vec::new())
    } else if mode == VerifyMode::Warn {
        (CapabilityStatus::Warn, Vec::new(), findings)
    } else {
        (CapabilityStatus::Fail, findings, Vec::new())
    };

    CapabilityReport {
        component: "yt-dlp".to_string(),
        version: expected_version.to_string(),
        mode: mode.as_str().to_string(),
        status,
        checked_at: unix_now_secs(),
        reported_version: if reported.is_empty() {
            None
        } else {
            Some(reported)
        },
        reasons,
        warnings,
        ffmpeg: None,
    }
}

/// Evaluates QuickJS script output from `qjs -e "print('clipper-qjs-ok', 1 + 2)"`.
pub fn verify_qjs_output(
    eval_stdout: &str,
    version_stdout: Option<&str>,
    expected_version: &str,
    mode: VerifyMode,
) -> CapabilityReport {
    let eval_clean = eval_stdout.trim_start_matches('\u{feff}').trim();
    let reported_version = version_stdout.and_then(|v| {
        let trimmed = v.trim_start_matches('\u{feff}').trim();
        if trimmed.is_empty() {
            None
        } else {
            trimmed
                .lines()
                .next()
                .map(|l| l.trim().to_string())
                .filter(|s| !s.is_empty())
        }
    });

    let mut findings = Vec::new();
    if eval_clean != "clipper-qjs-ok 3" {
        findings.push(format!(
            "qjs trivial script evaluation returned '{}', expected 'clipper-qjs-ok 3'",
            eval_clean
        ));
    }

    let (status, reasons, warnings) = if findings.is_empty() {
        (CapabilityStatus::Pass, Vec::new(), Vec::new())
    } else if mode == VerifyMode::Warn {
        (CapabilityStatus::Warn, Vec::new(), findings)
    } else {
        (CapabilityStatus::Fail, findings, Vec::new())
    };

    CapabilityReport {
        component: "qjs".to_string(),
        version: expected_version.to_string(),
        mode: mode.as_str().to_string(),
        status,
        checked_at: unix_now_secs(),
        reported_version,
        reasons,
        warnings,
        ffmpeg: None,
    }
}

/// Evaluates `whisper-cli --version` output.
pub fn verify_whisper_output(
    stdout: &str,
    expected_version: &str,
    mode: VerifyMode,
) -> CapabilityReport {
    let cleaned = stdout.trim_start_matches('\u{feff}');
    let mut parsed_version: Option<String> = None;

    for line in cleaned.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("whisper.cpp version:") {
            let v = rest.trim();
            if !v.is_empty() {
                parsed_version = Some(v.to_string());
                break;
            }
        }
    }

    if parsed_version.is_none() {
        if let Some(first) = cleaned.lines().find(|l| !l.trim().is_empty()) {
            let lower = first.to_lowercase();
            if lower.contains("whisper") && lower.contains("version") {
                if let Some(tok) = first.split_whitespace().last() {
                    parsed_version = Some(tok.trim_start_matches('v').to_string());
                }
            }
        }
    }

    let mut findings = Vec::new();
    match &parsed_version {
        None => {
            findings
                .push("whisper-cli --version did not report a recognizable version".to_string());
        }
        Some(ver) => {
            let exp_norm = expected_version.trim().trim_start_matches('v');
            let rep_norm = ver.trim().trim_start_matches('v');
            if !exp_norm.is_empty()
                && !exp_norm.eq_ignore_ascii_case("custom")
                && rep_norm != exp_norm
            {
                findings.push(format!(
                    "whisper-cli reported version '{}' does not match expected '{}'",
                    rep_norm, exp_norm
                ));
            }
        }
    }

    let (status, reasons, warnings) = if findings.is_empty() {
        (CapabilityStatus::Pass, Vec::new(), Vec::new())
    } else if mode == VerifyMode::Warn {
        (CapabilityStatus::Warn, Vec::new(), findings)
    } else {
        (CapabilityStatus::Fail, findings, Vec::new())
    };

    CapabilityReport {
        component: "whisper-cli".to_string(),
        version: expected_version.to_string(),
        mode: mode.as_str().to_string(),
        status,
        checked_at: unix_now_secs(),
        reported_version: parsed_version,
        reasons,
        warnings,
        ffmpeg: None,
    }
}

/// Verifies capabilities of an installed component version directory using `SidecarSupervisor`.
pub async fn verify_installed_component_capabilities(
    component: &str,
    version_dir: &Path,
    expected_version: &str,
    mode: VerifyMode,
    supervisor: &SidecarSupervisor,
) -> Result<CapabilityReport, CapabilityError> {
    match component {
        "ffmpeg" => {
            let ffmpeg = version_dir.join("ffmpeg.exe");
            let ffprobe = version_dir.join("ffprobe.exe");
            verify_ffmpeg_capabilities(&ffmpeg, &ffprobe, expected_version, mode, supervisor).await
        }
        "yt-dlp" => {
            let bin = version_dir.join("yt-dlp.exe");
            if !bin.is_file() {
                return Err(CapabilityError::BinaryMissing(bin.display().to_string()));
            }
            let out = run_probe(supervisor, &bin, &["--version"]).await?;
            let report = verify_yt_dlp_output(&out, expected_version, mode);
            if report.status == CapabilityStatus::Fail {
                return Err(CapabilityError::CapabilityCheckFailed {
                    reason: report.reasons.join("; "),
                    report: Box::new(report),
                });
            }
            Ok(report)
        }
        "qjs" | "quickjs" => {
            let bin = version_dir.join("qjs.exe");
            if !bin.is_file() {
                return Err(CapabilityError::BinaryMissing(bin.display().to_string()));
            }
            let eval_out =
                run_probe(supervisor, &bin, &["-e", "print('clipper-qjs-ok', 1 + 2)"]).await?;
            let ver_out = run_probe(supervisor, &bin, &["--version"]).await.ok();
            let report = verify_qjs_output(&eval_out, ver_out.as_deref(), expected_version, mode);
            if report.status == CapabilityStatus::Fail {
                return Err(CapabilityError::CapabilityCheckFailed {
                    reason: report.reasons.join("; "),
                    report: Box::new(report),
                });
            }
            Ok(report)
        }
        "whisper-cli" | "whisper" => {
            let bin = version_dir.join("whisper-cli.exe");
            if !bin.is_file() {
                return Err(CapabilityError::BinaryMissing(bin.display().to_string()));
            }
            let out = run_probe(supervisor, &bin, &["--version"]).await?;
            let report = verify_whisper_output(&out, expected_version, mode);
            if report.status == CapabilityStatus::Fail {
                return Err(CapabilityError::CapabilityCheckFailed {
                    reason: report.reasons.join("; "),
                    report: Box::new(report),
                });
            }
            Ok(report)
        }
        #[cfg(any(debug_assertions, test))]
        "fake-sidecar" => {
            let bin = version_dir.join("fake-sidecar.exe");
            if !bin.is_file() {
                return Err(CapabilityError::BinaryMissing(bin.display().to_string()));
            }
            let out = run_probe(supervisor, &bin, &["--mode", "normal"]).await?;
            let ok = out.contains("hello");
            let report = CapabilityReport {
                component: "fake-sidecar".to_string(),
                version: expected_version.to_string(),
                mode: mode.as_str().to_string(),
                status: if ok {
                    CapabilityStatus::Pass
                } else {
                    CapabilityStatus::Fail
                },
                checked_at: unix_now_secs(),
                reported_version: Some(expected_version.to_string()),
                reasons: if ok {
                    vec![]
                } else {
                    vec!["fake-sidecar did not emit 'hello'".to_string()]
                },
                warnings: vec![],
                ffmpeg: None,
            };
            if report.status == CapabilityStatus::Fail {
                return Err(CapabilityError::CapabilityCheckFailed {
                    reason: report.reasons.join("; "),
                    report: Box::new(report),
                });
            }
            Ok(report)
        }
        other => Err(CapabilityError::ExecutionFailed(format!(
            "Unsupported component for capability verification: {}",
            other
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LGPL_VERSION: &str =
        include_str!("../tests/fixtures/capabilities/ffmpeg_lgpl_version.txt");
    const LGPL_BUILDCONF: &str =
        include_str!("../tests/fixtures/capabilities/ffmpeg_lgpl_buildconf.txt");
    const LGPL_ENCODERS: &str =
        include_str!("../tests/fixtures/capabilities/ffmpeg_lgpl_encoders.txt");
    const LGPL_DECODERS: &str =
        include_str!("../tests/fixtures/capabilities/ffmpeg_lgpl_decoders.txt");
    const LGPL_FILTERS: &str =
        include_str!("../tests/fixtures/capabilities/ffmpeg_lgpl_filters.txt");
    const LGPL_PROTOCOLS: &str =
        include_str!("../tests/fixtures/capabilities/ffmpeg_lgpl_protocols.txt");
    const LGPL_FORMATS: &str =
        include_str!("../tests/fixtures/capabilities/ffmpeg_lgpl_formats.txt");
    const LGPL_FFPROBE: &str =
        include_str!("../tests/fixtures/capabilities/ffprobe_lgpl_version.txt");

    const GPL_VERSION: &str = include_str!("../tests/fixtures/capabilities/ffmpeg_gpl_version.txt");
    const GPL_BUILDCONF: &str =
        include_str!("../tests/fixtures/capabilities/ffmpeg_gpl_buildconf.txt");
    const GPL_ENCODERS: &str =
        include_str!("../tests/fixtures/capabilities/ffmpeg_gpl_encoders.txt");
    const GPL_DECODERS: &str =
        include_str!("../tests/fixtures/capabilities/ffmpeg_gpl_decoders.txt");
    const GPL_FILTERS: &str = include_str!("../tests/fixtures/capabilities/ffmpeg_gpl_filters.txt");
    const GPL_PROTOCOLS: &str =
        include_str!("../tests/fixtures/capabilities/ffmpeg_gpl_protocols.txt");
    const GPL_FORMATS: &str = include_str!("../tests/fixtures/capabilities/ffmpeg_gpl_formats.txt");
    const GPL_FFPROBE: &str =
        include_str!("../tests/fixtures/capabilities/ffprobe_gpl_version.txt");

    const YTDLP_VERSION_OUT: &str =
        include_str!("../tests/fixtures/capabilities/yt_dlp_version.txt");
    const QJS_EVAL_OUT: &str = include_str!("../tests/fixtures/capabilities/qjs_eval.txt");
    const WHISPER_VERSION_OUT: &str =
        include_str!("../tests/fixtures/capabilities/whisper_cli_version.txt");

    #[test]
    fn test_gpl_denylist_loaded_from_benchmarks() {
        let denylist = load_gpl_denylist_filters();
        assert_eq!(denylist.len(), 36);
        assert!(denylist.contains("boxblur"));
        assert!(denylist.contains("eq"));
        assert!(denylist.contains("vidstabdetect"));
        assert!(denylist.contains("rubberband"));
        assert!(!denylist.contains("gblur"));
    }

    #[test]
    fn test_golden_lgpl_ffmpeg_strict_passes() {
        let report = verify_ffmpeg_outputs(
            LGPL_VERSION,
            LGPL_BUILDCONF,
            LGPL_ENCODERS,
            LGPL_DECODERS,
            LGPL_FILTERS,
            LGPL_PROTOCOLS,
            LGPL_FORMATS,
            LGPL_FFPROBE,
            "n9.0.2-58-g8c7ef49002-20261119",
            VerifyMode::Strict,
        );

        assert_eq!(report.status, CapabilityStatus::Pass, "{:?}", report);
        assert!(report.reasons.is_empty());
        assert!(report.warnings.is_empty());

        let ff = report.ffmpeg.expect("ffmpeg caps");
        assert_eq!(ff.ffmpeg_version, "n9.0.2-58-g8c7ef49002-20261119");
        assert_eq!(ff.ffprobe_version, "n9.0.2-58-g8c7ef49002-20261119");
        assert!(!ff.has_gpl);
        assert!(!ff.has_nonfree);
        assert!(ff.has_version3);
        assert_eq!(ff.effective_license, "LGPLv3");
        assert!(ff.forbidden_libs_present.is_empty());
        assert!(ff.forbidden_encoders_present.is_empty());
        assert!(ff.gpl_filters_present.is_empty());
        assert!(ff.missing_encoders.is_empty());
        assert!(ff.missing_decoders.is_empty());
        assert!(ff.missing_filters.is_empty());
        assert!(ff.missing_protocols.is_empty());
        assert!(ff.missing_formats.is_empty());
        assert!(ff
            .hardware_encoders_available
            .contains(&"h264_mf".to_string()));
        assert!(ff
            .hardware_encoders_available
            .contains(&"h264_nvenc".to_string()));
        assert!(ff
            .hardware_encoders_available
            .contains(&"h264_qsv".to_string()));
        assert!(ff
            .hardware_encoders_available
            .contains(&"h264_amf".to_string()));
    }

    #[test]
    fn test_golden_gpl_ffmpeg_rejected_in_strict_and_flagged_in_warn() {
        let strict = verify_ffmpeg_outputs(
            GPL_VERSION,
            GPL_BUILDCONF,
            GPL_ENCODERS,
            GPL_DECODERS,
            GPL_FILTERS,
            GPL_PROTOCOLS,
            GPL_FORMATS,
            GPL_FFPROBE,
            "9.0.2-essentials",
            VerifyMode::Strict,
        );
        assert_eq!(strict.status, CapabilityStatus::Fail);
        let ff = strict.ffmpeg.as_ref().expect("ffmpeg details");
        assert!(ff.has_gpl);
        assert_eq!(ff.effective_license, "GPLv3");
        assert!(ff.forbidden_libs_present.contains(&"libx264".to_string()));
        assert!(ff.forbidden_libs_present.contains(&"libx265".to_string()));
        assert!(ff
            .forbidden_encoders_present
            .contains(&"libx264".to_string()));
        assert!(ff.gpl_filters_present.contains(&"boxblur".to_string()));
        assert!(ff.gpl_filters_present.contains(&"rubberband".to_string()));
        assert!(!strict.reasons.is_empty());

        let warn = verify_ffmpeg_outputs(
            GPL_VERSION,
            GPL_BUILDCONF,
            GPL_ENCODERS,
            GPL_DECODERS,
            GPL_FILTERS,
            GPL_PROTOCOLS,
            GPL_FORMATS,
            GPL_FFPROBE,
            "custom",
            VerifyMode::Warn,
        );
        assert_eq!(warn.status, CapabilityStatus::Warn);
        assert!(warn.reasons.is_empty());
        assert!(!warn.warnings.is_empty());
    }

    #[test]
    fn test_ffmpeg_detects_missing_caps_and_nonfree() {
        // Strip --enable-version3, add --enable-nonfree, remove https protocol and h264 decoder
        let bad_ver = LGPL_VERSION.replace("--enable-version3", "--enable-nonfree");
        let bad_bc = LGPL_BUILDCONF.replace("--enable-version3", "--enable-nonfree");
        let bad_enc = LGPL_ENCODERS.replace("h264_mf", "removed_mf");
        let bad_dec = LGPL_DECODERS.replace("h264                 ", "removed_dec          ");
        let bad_flt = LGPL_FILTERS.replace(" ass ", " removed_ass ");
        let bad_prt = LGPL_PROTOCOLS.replace("  https\n", "  ftp_only\n");

        let report = verify_ffmpeg_outputs(
            &bad_ver,
            &bad_bc,
            &bad_enc,
            &bad_dec,
            &bad_flt,
            &bad_prt,
            LGPL_FORMATS,
            LGPL_FFPROBE,
            "broken",
            VerifyMode::Strict,
        );

        assert_eq!(report.status, CapabilityStatus::Fail);
        let ff = report.ffmpeg.unwrap();
        assert!(ff.has_nonfree);
        assert_eq!(ff.effective_license, "Non-free");
        assert!(ff.missing_encoders.contains(&"h264_mf".to_string()));
        assert!(ff.missing_decoders.contains(&"h264".to_string()));
        assert!(ff.missing_filters.contains(&"ass".to_string()));
        assert!(ff.missing_protocols.contains(&"https".to_string()));
    }

    #[test]
    fn test_golden_yt_dlp_qjs_and_whisper_checks() {
        let yt_ok = verify_yt_dlp_output(YTDLP_VERSION_OUT, "2026.10.26", VerifyMode::Strict);
        assert_eq!(yt_ok.status, CapabilityStatus::Pass);
        assert_eq!(yt_ok.reported_version.as_deref(), Some("2026.10.26"));

        let yt_mismatch = verify_yt_dlp_output(YTDLP_VERSION_OUT, "2026.11.01", VerifyMode::Strict);
        assert_eq!(yt_mismatch.status, CapabilityStatus::Fail);

        let qjs_ok = verify_qjs_output(QJS_EVAL_OUT, Some("0.18.0"), "v0.18.0", VerifyMode::Strict);
        assert_eq!(qjs_ok.status, CapabilityStatus::Pass);
        assert_eq!(qjs_ok.reported_version.as_deref(), Some("0.18.0"));

        let qjs_bad = verify_qjs_output("ReferenceError: x", None, "v0.18.0", VerifyMode::Strict);
        assert_eq!(qjs_bad.status, CapabilityStatus::Fail);

        let whisper_ok = verify_whisper_output(WHISPER_VERSION_OUT, "v1.9.5", VerifyMode::Strict);
        assert_eq!(whisper_ok.status, CapabilityStatus::Pass);
        assert_eq!(whisper_ok.reported_version.as_deref(), Some("1.9.5"));

        let whisper_empty = verify_whisper_output("", "1.9.5", VerifyMode::Strict);
        assert_eq!(whisper_empty.status, CapabilityStatus::Fail);
    }

    #[tokio::test]
    async fn test_supervisor_runs_fake_sidecar_capability_and_missing_binary() {
        let supervisor = SidecarSupervisor::new();
        let temp = tempfile::TempDir::new().unwrap();
        let res = verify_installed_component_capabilities(
            "ffmpeg",
            temp.path(),
            "1.0.0",
            VerifyMode::Strict,
            &supervisor,
        )
        .await;
        assert!(matches!(res, Err(CapabilityError::BinaryMissing(_))));
        assert_eq!(res.unwrap_err().error_code(), "E_COMPONENT_MISSING");
    }
}
