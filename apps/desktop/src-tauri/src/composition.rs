//! Initial Composition JSON v1 builder, canonical hash calculator, and schema validator
//! matching `packages/shared/src/composition.ts` & `DATA_MODEL §7`.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::download::DownloadStageOutcome;
use crate::security;
use crate::store::ProjectRecord;

pub const MAX_COMPOSITION_JSON_BYTES: usize = 1_048_576;

#[derive(Error, Debug)]
pub enum CompositionError {
    #[error("Invalid composition schema field: {0}")]
    ValidationFailed(String),

    #[error("Failed to serialize or persist composition: {0}")]
    Io(String),
}

#[derive(Error, Debug)]
pub enum CompositionSaveError {
    #[error("{message_id}")]
    InvalidSchema {
        code: &'static str,
        message_id: String,
        detail: String,
    },
    #[error("{message_id}")]
    InvalidRange {
        code: &'static str,
        message_id: String,
        detail: String,
    },
    #[error("IO error saving composition: {0}")]
    Io(String),
}

impl CompositionSaveError {
    pub fn code(&self) -> &'static str {
        match self {
            CompositionSaveError::InvalidSchema { code, .. }
            | CompositionSaveError::InvalidRange { code, .. } => code,
            CompositionSaveError::Io(_) => "E_UNKNOWN",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            CompositionSaveError::InvalidSchema { detail, .. }
            | CompositionSaveError::InvalidRange { detail, .. } => detail.clone(),
            CompositionSaveError::Io(s) => s.clone(),
        }
    }
}

/// Converts floating-point FPS (e.g. `59.94`, `29.97`, `60.0`) to a canonical rational pair `(num, den)`.
pub fn fps_to_rational(fps: f64) -> (u32, u32) {
    if !fps.is_finite() || fps <= 0.0 {
        return (30, 1);
    }
    if (fps - 59.94005994).abs() < 0.02 {
        return (60_000, 1001);
    }
    if (fps - 29.97002997).abs() < 0.02 {
        return (30_000, 1001);
    }
    if (fps - 23.97602398).abs() < 0.02 {
        return (24_000, 1001);
    }
    let rounded = fps.round();
    if (fps - rounded).abs() < 0.01 && (1.0..=240.0).contains(&rounded) {
        return (rounded as u32, 1);
    }
    let num = (fps * 1000.0).round().clamp(1000.0, 240_000.0) as u32;
    (num, 1000)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CompositionCanvas {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CompositionSource {
    pub id: String,
    pub provider: String,
    pub video_id: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_path: Option<String>,
    pub absolute_start_ms: u32,
    pub duration_ms: u32,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TimelineSegment {
    pub id: String,
    pub source_id: String,
    pub in_ms: u32,
    pub out_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum CompositionLayout {
    Fill { crop: Rect },
    BlurFit { blur: f64 },
    Split { top: Rect, bottom: Rect, ratio: f64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CaptionWord {
    pub id: String,
    pub t0: f64,
    pub t1: f64,
    pub text: String,
    pub timing_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CaptionEdit {
    pub word_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub t0: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub t1: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CaptionStyle {
    pub font_family: String,
    pub font_size_pct: f64,
    pub weight: u32,
    pub color: String,
    pub outline_color: String,
    pub outline_px: f64,
    pub shadow: bool,
    pub position_anchor: String,
    #[serde(rename = "offsetYPct")]
    pub offset_y_pct: f64,
    pub max_words_per_line: u32,
    pub max_lines: u32,
    pub animation: String,
    pub highlight_color: String,
    pub uppercase: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CompositionCaptions {
    pub enabled: bool,
    pub status: String,
    pub language: String,
    pub words: Vec<CaptionWord>,
    pub edits: Vec<CaptionEdit>,
    pub style: CaptionStyle,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioNormalizeConfig {
    pub enabled: bool,
    pub target_lufs: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CompositionAudio {
    pub gain_db: f64,
    pub normalize: AudioNormalizeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TemplateRef {
    pub id: String,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CompositionExportConfig {
    pub preset: String,
    pub audio_kbps: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CompositionV1 {
    pub schema_version: u32,
    pub id: String,
    pub canvas: CompositionCanvas,
    pub sources: Vec<CompositionSource>,
    pub timeline: Vec<TimelineSegment>,
    pub layout: CompositionLayout,
    pub captions: CompositionCaptions,
    pub overlays: Vec<serde_json::Value>,
    pub audio: CompositionAudio,
    pub template: Option<TemplateRef>,
    pub export: CompositionExportConfig,
}

fn round_coord(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}

/// Computes a centered 9:16 normalized crop `Rect` (`0..=1`) for `fill` layout.
pub fn compute_centered_portrait_crop(source_width: u32, source_height: u32) -> Rect {
    let target_aspect = 9.0 / 16.0;
    let w = source_width.max(1) as f64;
    let h = source_height.max(1) as f64;
    let source_aspect = w / h;

    if (source_aspect - target_aspect).abs() < 1e-4 {
        Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        }
    } else if source_aspect > target_aspect {
        let crop_w = (target_aspect / source_aspect).clamp(0.05, 1.0);
        let x = ((1.0 - crop_w) * 0.5).clamp(0.0, 1.0);
        Rect {
            x: round_coord(x),
            y: 0.0,
            w: round_coord(crop_w),
            h: 1.0,
        }
    } else {
        let crop_h = (source_aspect / target_aspect).clamp(0.05, 1.0);
        let y = ((1.0 - crop_h) * 0.5).clamp(0.0, 1.0);
        Rect {
            x: 0.0,
            y: round_coord(y),
            w: 1.0,
            h: round_coord(crop_h),
        }
    }
}

pub fn default_caption_style() -> CaptionStyle {
    CaptionStyle {
        font_family: "Montserrat".to_string(),
        font_size_pct: 4.5,
        weight: 700,
        color: "#FFFFFF".to_string(),
        outline_color: "#000000".to_string(),
        outline_px: 3.0,
        shadow: true,
        position_anchor: "bottom".to_string(),
        offset_y_pct: 12.0,
        max_words_per_line: 4,
        max_lines: 2,
        animation: "phrase".to_string(),
        highlight_color: "#FACC15".to_string(),
        uppercase: false,
    }
}

/// Builds initial `CompositionV1` from `project` + verified download (+ optional preview proxy path).
pub fn build_initial_composition(
    project: &ProjectRecord,
    download: &DownloadStageOutcome,
    preview_rel_path: Option<String>,
) -> Result<CompositionV1, CompositionError> {
    let source_duration_ms = download.media.duration_ms.max(1000);
    let in_ms = download
        .window
        .handle_before_ms
        .min(source_duration_ms.saturating_sub(1));
    let raw_out_ms = download
        .window
        .handle_before_ms
        .saturating_add(download.window.selected_duration_ms());
    let out_ms = raw_out_ms
        .min(source_duration_ms)
        .max(in_ms.saturating_add(1));

    let canvas_fps = if download.media.fps >= 45.0 { 60 } else { 30 };

    let comp = CompositionV1 {
        schema_version: 1,
        id: project.id.clone(),
        canvas: CompositionCanvas {
            width: 1080,
            height: 1920,
            fps: canvas_fps,
        },
        sources: vec![CompositionSource {
            id: "src-1".to_string(),
            provider: project.source_provider.clone(),
            video_id: project.source_video_id.clone(),
            path: download.rel_path.clone(),
            preview_path: preview_rel_path,
            absolute_start_ms: download.window.requested_start_ms,
            duration_ms: source_duration_ms,
            width: download.media.width,
            height: download.media.height,
            fps: download.media.fps,
        }],
        timeline: vec![TimelineSegment {
            id: "seg-1".to_string(),
            source_id: "src-1".to_string(),
            in_ms,
            out_ms,
        }],
        layout: CompositionLayout::Fill {
            crop: compute_centered_portrait_crop(download.media.width, download.media.height),
        },
        captions: CompositionCaptions {
            enabled: false,
            status: "none".to_string(),
            language: "id".to_string(),
            words: Vec::new(),
            edits: Vec::new(),
            style: default_caption_style(),
        },
        overlays: Vec::new(),
        audio: CompositionAudio {
            gain_db: 0.0,
            normalize: AudioNormalizeConfig {
                enabled: false,
                target_lufs: -14.0,
            },
        },
        template: None,
        export: CompositionExportConfig {
            preset: "standard".to_string(),
            audio_kbps: 192,
        },
    };

    validate_composition(&comp)?;
    Ok(comp)
}

fn validate_workspace_rel_path(path: &str) -> bool {
    if !(path.starts_with("cache/") || path.starts_with("projects/")) {
        return false;
    }
    if path.contains("..") {
        return false;
    }
    path.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'))
}

fn validate_rect(r: &Rect) -> bool {
    [r.x, r.y, r.w, r.h]
        .iter()
        .all(|v| v.is_finite() && *v >= 0.0 && *v <= 1.0)
        && r.w > 0.0
        && r.h > 0.0
        && (r.x + r.w) <= 1.0 + 1e-4
        && (r.y + r.h) <= 1.0 + 1e-4
}

fn validate_hex_color(raw: &str) -> bool {
    if !raw.starts_with('#') {
        return false;
    }
    let hex_part = &raw[1..];
    matches!(hex_part.len(), 3 | 6 | 8) && hex_part.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Validates every `CompositionSchema` invariant in Rust before persisting or hashing.
pub fn validate_composition(comp: &CompositionV1) -> Result<(), CompositionError> {
    if comp.schema_version != 1 {
        return Err(CompositionError::ValidationFailed(
            "schemaVersion must equal 1".to_string(),
        ));
    }
    if uuid::Uuid::parse_str(&comp.id).is_err() {
        return Err(CompositionError::ValidationFailed(format!(
            "composition id '{}' must be a valid UUID",
            comp.id
        )));
    }
    if comp.canvas.width != 1080 || comp.canvas.height != 1920 {
        return Err(CompositionError::ValidationFailed(
            "canvas dimensions must be 1080x1920".to_string(),
        ));
    }
    if !matches!(comp.canvas.fps, 30 | 60) {
        return Err(CompositionError::ValidationFailed(
            "canvas.fps must be 30 or 60".to_string(),
        ));
    }
    if comp.sources.is_empty() {
        return Err(CompositionError::ValidationFailed(
            "sources must have at least 1 entry".to_string(),
        ));
    }
    for s in &comp.sources {
        if !matches!(s.provider.as_str(), "youtube" | "local_file") {
            return Err(CompositionError::ValidationFailed(format!(
                "invalid source provider '{}'",
                s.provider
            )));
        }
        if s.video_id.is_empty() {
            return Err(CompositionError::ValidationFailed(
                "source videoId cannot be empty".to_string(),
            ));
        }
        if !validate_workspace_rel_path(&s.path) {
            return Err(CompositionError::ValidationFailed(format!(
                "invalid source workspace path '{}'",
                s.path
            )));
        }
        if let Some(ref p) = s.preview_path {
            if !validate_workspace_rel_path(p) {
                return Err(CompositionError::ValidationFailed(format!(
                    "invalid previewPath '{}'",
                    p
                )));
            }
        }
        if s.duration_ms == 0 || s.width == 0 || s.height == 0 || !s.fps.is_finite() || s.fps <= 0.0
        {
            return Err(CompositionError::ValidationFailed(
                "source media metadata must be strictly positive".to_string(),
            ));
        }
    }

    if comp.timeline.is_empty() || comp.timeline.len() > 20 {
        return Err(CompositionError::ValidationFailed(
            "timeline length must be between 1 and 20".to_string(),
        ));
    }
    for seg in &comp.timeline {
        let Some(src) = comp.sources.iter().find(|s| s.id == seg.source_id) else {
            return Err(CompositionError::ValidationFailed(format!(
                "timeline segment references unknown source '{}'",
                seg.source_id
            )));
        };
        if seg.out_ms <= seg.in_ms || seg.out_ms > src.duration_ms {
            return Err(CompositionError::ValidationFailed(format!(
                "timeline segment bounds [{}, {}] exceed source duration {}",
                seg.in_ms, seg.out_ms, src.duration_ms
            )));
        }
    }

    match &comp.layout {
        CompositionLayout::Fill { crop } => {
            if !validate_rect(crop) {
                return Err(CompositionError::ValidationFailed(
                    "fill crop rect must stay within [0, 1]".to_string(),
                ));
            }
        }
        CompositionLayout::BlurFit { blur } => {
            if !blur.is_finite() || *blur <= 0.0 {
                return Err(CompositionError::ValidationFailed(
                    "blur must be positive".to_string(),
                ));
            }
        }
        CompositionLayout::Split { top, bottom, ratio } => {
            if !validate_rect(top)
                || !validate_rect(bottom)
                || !ratio.is_finite()
                || *ratio < 0.3
                || *ratio > 0.7
            {
                return Err(CompositionError::ValidationFailed(
                    "split layout coordinates or ratio out of bounds".to_string(),
                ));
            }
        }
    }

    if !matches!(
        comp.captions.status.as_str(),
        "none" | "running" | "ready" | "stale"
    ) || !matches!(comp.captions.language.as_str(), "id" | "en" | "auto")
    {
        return Err(CompositionError::ValidationFailed(
            "invalid captions status or language".to_string(),
        ));
    }

    let st = &comp.captions.style;
    if !matches!(
        st.font_family.as_str(),
        "Montserrat" | "Poppins" | "Inter" | "Anton"
    ) || !matches!(st.weight, 400 | 600 | 700 | 800)
        || !matches!(st.max_lines, 1..=3)
        || !validate_hex_color(&st.color)
        || !validate_hex_color(&st.outline_color)
        || !validate_hex_color(&st.highlight_color)
    {
        return Err(CompositionError::ValidationFailed(
            "invalid caption style".to_string(),
        ));
    }

    if !comp.audio.normalize.target_lufs.is_finite()
        || comp.audio.normalize.target_lufs < -70.0
        || comp.audio.normalize.target_lufs > 0.0
    {
        return Err(CompositionError::ValidationFailed(
            "audio.normalize.targetLufs must be in [-70, 0]".to_string(),
        ));
    }

    if !matches!(comp.export.preset.as_str(), "standard" | "small_file")
        || comp.export.audio_kbps == 0
    {
        return Err(CompositionError::ValidationFailed(
            "invalid export config".to_string(),
        ));
    }

    Ok(())
}

/// Validates, produces canonical JSON string and SHA-256 `comp_hash`.
pub fn canonicalize_and_hash(comp: &CompositionV1) -> Result<(String, String), CompositionError> {
    validate_composition(comp)?;
    let canonical_json =
        serde_json::to_string(comp).map_err(|e| CompositionError::Io(e.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(canonical_json.as_bytes());
    let hash = hex::encode(hasher.finalize());
    Ok((canonical_json, hash))
}

/// Writes canonical `projects/<id>/composition.json` atomically via confined workspace path.
pub fn write_project_composition_file(
    ws_root: &Path,
    project_id: &str,
    canonical_json: &str,
) -> Result<String, CompositionError> {
    let rel_path = format!("projects/{}/composition.json", project_id);
    let abs_path = security::resolve_workspace_path_with_root(ws_root, &rel_path)
        .map_err(|e| CompositionError::Io(e.to_string()))?;
    if let Some(parent) = abs_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CompositionError::Io(e.to_string()))?;
    }
    let tmp_path = abs_path.with_extension("json.tmp");
    std::fs::write(&tmp_path, canonical_json.as_bytes())
        .map_err(|e| CompositionError::Io(e.to_string()))?;
    if abs_path.exists() {
        let _ = std::fs::remove_file(&abs_path);
    }
    std::fs::rename(&tmp_path, &abs_path).map_err(|e| CompositionError::Io(e.to_string()))?;
    Ok(rel_path)
}

/// Validates untrusted composition JSON from the editor UI against schema + clip limits,
/// recomputes canonical JSON and SHA-256 `comp_hash` authoritatively.
pub fn parse_and_validate_composition_for_save(
    expected_project_id: &str,
    raw_json: &str,
    max_clip_seconds: u32,
) -> Result<(CompositionV1, String, String), CompositionSaveError> {
    if raw_json.len() > MAX_COMPOSITION_JSON_BYTES {
        return Err(CompositionSaveError::InvalidSchema {
            code: "E_TEMPLATE_INVALID",
            message_id: "Data komposisi melebihi batas ukuran maksimum (1 MB).".to_string(),
            detail: format!(
                "composition JSON exceeds {} bytes",
                MAX_COMPOSITION_JSON_BYTES
            ),
        });
    }

    let comp: CompositionV1 =
        serde_json::from_str(raw_json).map_err(|e| CompositionSaveError::InvalidSchema {
            code: "E_TEMPLATE_INVALID",
            message_id: "Format data komposisi tidak valid.".to_string(),
            detail: format!("Invalid composition JSON: {}", e),
        })?;

    if comp.id != expected_project_id {
        return Err(CompositionSaveError::InvalidSchema {
            code: "E_TEMPLATE_INVALID",
            message_id: "ID proyek pada komposisi tidak sesuai.".to_string(),
            detail: format!(
                "Composition id '{}' does not match project '{}'",
                comp.id, expected_project_id
            ),
        });
    }

    validate_composition(&comp).map_err(|e| match e {
        CompositionError::ValidationFailed(msg) => {
            if msg.contains("timeline segment") {
                CompositionSaveError::InvalidRange {
                    code: "E_RANGE_INVALID",
                    message_id: "Batas rentang trim tidak valid untuk durasi video sumber."
                        .to_string(),
                    detail: msg,
                }
            } else {
                CompositionSaveError::InvalidSchema {
                    code: "E_TEMPLATE_INVALID",
                    message_id: "Pengaturan crop atau skema komposisi tidak valid.".to_string(),
                    detail: msg,
                }
            }
        }
        CompositionError::Io(msg) => CompositionSaveError::Io(msg),
    })?;

    let mut total_ms: u64 = 0;
    for seg in &comp.timeline {
        let src = comp
            .sources
            .iter()
            .find(|s| s.id == seg.source_id)
            .ok_or_else(|| CompositionSaveError::InvalidSchema {
                code: "E_TEMPLATE_INVALID",
                message_id: "Sumber video pada timeline tidak ditemukan.".to_string(),
                detail: format!("Unknown source '{}'", seg.source_id),
            })?;
        let one_frame_ms = (1000.0 / src.fps).floor().max(1.0) as u32;
        let seg_dur_ms = seg.out_ms.saturating_sub(seg.in_ms);
        if seg_dur_ms < one_frame_ms {
            return Err(CompositionSaveError::InvalidRange {
                code: "E_RANGE_INVALID",
                message_id: "Durasi segmen klip minimal 1 frame.".to_string(),
                detail: format!(
                    "Segment duration {} ms < 1 frame ({} ms)",
                    seg_dur_ms, one_frame_ms
                ),
            });
        }
        total_ms = total_ms.saturating_add(seg_dur_ms as u64);
    }

    let max_clip_ms = (max_clip_seconds as u64).saturating_mul(1000);
    if total_ms > max_clip_ms {
        return Err(CompositionSaveError::InvalidRange {
            code: "E_RANGE_TOO_LONG",
            message_id: format!(
                "Durasi total klip ({:.1} detik) melebihi batas maksimum {} detik.",
                total_ms as f64 / 1000.0,
                max_clip_seconds
            ),
            detail: format!("Total duration {} ms > {} ms limit", total_ms, max_clip_ms),
        });
    }

    let (canonical_json, hash) =
        canonicalize_and_hash(&comp).map_err(|e| CompositionSaveError::Io(e.to_string()))?;
    Ok((comp, canonical_json, hash))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::{DownloadStageOutcome, DownloadWindow, VerifiedMediaInfo};

    #[test]
    fn test_build_initial_composition_handles_fill_crop_and_canonical_hash() {
        let proj = ProjectRecord {
            id: uuid::Uuid::new_v4().to_string(),
            title: "VOD".into(),
            channel: "Channel".into(),
            source_provider: "youtube".into(),
            source_video_id: "WufkeDaJisQ".into(),
            source_url: "https://www.youtube.com/watch?v=WufkeDaJisQ".into(),
            duration_seconds: 600,
            live_status: Some("not_live".into()),
            preview_mode: "embed".into(),
            suggested_start_seconds: None,
            range_start_ms: Some(15_000.0),
            range_end_ms: Some(75_000.0),
            status: "ranged".into(),
            composition_hash: None,
            created_at: 1_700_000_000.0,
            updated_at: 1_700_000_000.0,
        };
        let dl = DownloadStageOutcome {
            cache_key: "a".repeat(64),
            rel_path: format!("cache/{}.mp4", "a".repeat(64)),
            abs_path: Path::new("C:\\unused.mp4").to_path_buf(),
            window: DownloadWindow {
                selected_start_ms: 15_000,
                selected_end_ms: 75_000,
                requested_start_ms: 10_000,
                requested_end_ms: 80_000,
                handle_before_ms: 5_000,
                handle_after_ms: 5_000,
            },
            media: VerifiedMediaInfo {
                width: 1920,
                height: 1080,
                fps: 60.0,
                duration_ms: 70_000,
                video_codec: "h264".into(),
                audio_codec: "aac".into(),
                format_name: "mp4".into(),
                size_bytes: 5_000_000,
                is_webview_native: true,
            },
            cache_hit: false,
            wall_ms: 25_000,
            reprobed_after_403: false,
        };

        let comp = build_initial_composition(&proj, &dl, Some("cache/proxies/p1.mp4".to_string()))
            .unwrap();
        assert_eq!(comp.schema_version, 1);
        assert_eq!(comp.canvas.width, 1080);
        assert_eq!(comp.canvas.height, 1920);
        assert_eq!(comp.canvas.fps, 60);
        assert_eq!(comp.sources[0].absolute_start_ms, 10_000);
        assert_eq!(
            comp.sources[0].preview_path.as_deref(),
            Some("cache/proxies/p1.mp4")
        );
        // Selected range inside downloaded file with 5 s handles = [5000 .. 65000]
        assert_eq!(comp.timeline[0].in_ms, 5_000);
        assert_eq!(comp.timeline[0].out_ms, 65_000);
        assert_eq!(comp.captions.status, "none");
        assert!(comp.template.is_none());

        // Centered 9:16 crop on 16:9: w = (9/16) / (16/9) = 81/256 = 0.316406
        match &comp.layout {
            CompositionLayout::Fill { crop } => {
                assert!((crop.w - 0.316406).abs() < 1e-4);
                assert!((crop.x - (1.0 - crop.w) * 0.5).abs() < 1e-4);
                assert_eq!(crop.y, 0.0);
                assert_eq!(crop.h, 1.0);
            }
            _ => panic!("Expected Fill layout"),
        }

        let (json1, hash1) = canonicalize_and_hash(&comp).unwrap();
        let (json2, hash2) = canonicalize_and_hash(&comp).unwrap();
        assert_eq!(json1, json2);
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64);

        // Reject out-of-bounds timeline
        let mut bad = comp.clone();
        bad.timeline[0].out_ms = 90_000;
        assert!(validate_composition(&bad).is_err());

        // Reject unsafe path traversal
        let mut bad_path = comp;
        bad_path.sources[0].path = "cache/../secret.mp4".into();
        assert!(validate_composition(&bad_path).is_err());
    }
}
