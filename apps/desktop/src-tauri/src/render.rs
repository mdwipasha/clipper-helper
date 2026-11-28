//! Composition rendering: composition → FFmpeg args (ARCHITECTURE §7; docs/benchmarks/S4-render.md).
//!
//! Pure functions that translate a canonical `CompositionV1` JSON into an FFmpeg
//! filtergraph + argument array for export. This module is the SINGLE SOURCE OF TRUTH
//! for the render pipeline — the editor's CSS transform (`cropMath.ts`) must produce
//! visually identical results.
//!
//! Layout `fill` only in M1.7 (blur_fit / split deferred to M2).
//! Captions OFF in M1.7 (deferred to M1.6).
//! Audio normalization OFF in M1.7 (gain-only via `volume` filter).

use std::path::Path;

use serde::{Deserialize, Serialize};
use specta::Type;
use thiserror::Error;

use crate::composition::{
    CompositionLayout, CompositionSource, CompositionV1, Rect, TimelineSegment,
};
use crate::encoder::VideoEncoder;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const OUTPUT_WIDTH: u32 = 1080;
pub const OUTPUT_HEIGHT: u32 = 1920;
pub const DEFAULT_FPS: u32 = 30;
pub const DEFAULT_AUDIO_KBPS: u32 = 160;
pub const DEFAULT_AUDIO_SAMPLE_RATE: u32 = 48_000;

/// GOP length in frames (2-second keyframe interval at 30 fps).
pub const GOP_LENGTH: u32 = 60;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
pub struct RenderPlan {
    /// Complete FFmpeg argument array (excluding the binary path itself).
    pub args: Vec<String>,
    /// The video filtergraph string passed to `-filter_complex` or `-vf`.
    pub video_filter_graph: String,
    /// The audio filter string passed to `-af`.
    pub audio_filter: String,
    /// Expected output duration in milliseconds.
    pub expected_duration_ms: u64,
    /// Expected output frame count.
    pub expected_output_frames: u64,
    /// Encoder used.
    pub encoder: String,
    /// Preset used.
    pub preset: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ExportPreset {
    Standard,
    SmallFile,
}

impl ExportPreset {
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "small_file" => ExportPreset::SmallFile,
            _ => ExportPreset::Standard,
        }
    }
}

#[derive(Error, Debug)]
pub enum RenderBuildError {
    #[error("Unsupported layout mode for M1.7: {0}")]
    UnsupportedLayout(String),
    #[error("No timeline segments")]
    EmptyTimeline,
    #[error("Source not found: {0}")]
    SourceNotFound(String),
    #[error("Invalid crop rect")]
    InvalidCrop,
    #[error("Invalid composition: {0}")]
    InvalidComposition(String),
}

/// Computed pixel crop from normalized rect + source dimensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelCrop {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

// ---------------------------------------------------------------------------
// Crop math (parity with cropMath.ts)
// ---------------------------------------------------------------------------

/// Convert normalized crop rect `{x, y, w, h}` in 0..1 of the source frame
/// to integer pixel values, ensuring even dimensions for yuv420p.
pub fn crop_rect_to_pixels(rect: &Rect, source_width: u32, source_height: u32) -> PixelCrop {
    let sw = source_width.max(2) as f64;
    let sh = source_height.max(2) as f64;

    // Round width/height to nearest even pixel (critical when e.g. 0.316406 * 1920 = 607.4995 -> 608)
    let even_max_w = source_width & !1;
    let even_max_h = source_height & !1;
    let w = ((((rect.w * sw) * 0.5).round() as u32).saturating_mul(2))
        .max(2)
        .min(even_max_w.max(2));
    let h = ((((rect.h * sh) * 0.5).round() as u32).saturating_mul(2))
        .max(2)
        .min(even_max_h.max(2));

    let raw_x = (rect.x * sw).round() as u32;
    let raw_y = (rect.y * sh).round() as u32;

    // Clamp position so crop stays inside the frame
    let x = raw_x.min(source_width.saturating_sub(w));
    let y = raw_y.min(source_height.saturating_sub(h));

    PixelCrop { x, y, w, h }
}

// ---------------------------------------------------------------------------
// Encoder presets (S4-render.md §4)
// ---------------------------------------------------------------------------

/// Returns encoder-specific arguments for the given encoder × preset combination.
/// Exact flags from docs/benchmarks/S4-render.md §4.
fn encoder_args(encoder: VideoEncoder, preset: ExportPreset) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    // Common output container flags
    args.extend([
        "-color_primaries".into(),
        "bt709".into(),
        "-color_trc".into(),
        "bt709".into(),
        "-colorspace".into(),
        "bt709".into(),
        "-color_range".into(),
        "tv".into(),
    ]);

    match (encoder, preset) {
        // ---- h264_amf ----
        (VideoEncoder::H264Amf, ExportPreset::Standard) => {
            args.extend([
                "-c:v".into(),
                "h264_amf".into(),
                "-usage".into(),
                "transcoding".into(),
                "-rc".into(),
                "vbr_peak".into(),
                "-b:v".into(),
                "4500k".into(),
                "-maxrate".into(),
                "6000k".into(),
                "-bufsize".into(),
                "9000k".into(),
                "-quality".into(),
                "quality".into(),
                "-profile:v".into(),
                "high".into(),
                "-level".into(),
                "4.2".into(),
                "-g".into(),
                GOP_LENGTH.to_string(),
            ]);
        }
        (VideoEncoder::H264Amf, ExportPreset::SmallFile) => {
            args.extend([
                "-c:v".into(),
                "h264_amf".into(),
                "-usage".into(),
                "transcoding".into(),
                "-rc".into(),
                "cbr".into(),
                "-b:v".into(),
                "2500k".into(),
                "-maxrate".into(),
                "2500k".into(),
                "-bufsize".into(),
                "5000k".into(),
                "-profile:v".into(),
                "high".into(),
                "-level".into(),
                "4.2".into(),
                "-g".into(),
                GOP_LENGTH.to_string(),
            ]);
        }
        // ---- h264_mf (CRITICAL: no -profile:v) ----
        (VideoEncoder::H264Mf, ExportPreset::Standard) => {
            args.extend([
                "-c:v".into(),
                "h264_mf".into(),
                "-b:v".into(),
                "4500k".into(),
                "-rate_control".into(),
                "pc_vbr".into(),
                "-g".into(),
                GOP_LENGTH.to_string(),
            ]);
        }
        (VideoEncoder::H264Mf, ExportPreset::SmallFile) => {
            args.extend([
                "-c:v".into(),
                "h264_mf".into(),
                "-b:v".into(),
                "2500k".into(),
                "-rate_control".into(),
                "cbr".into(),
                "-g".into(),
                GOP_LENGTH.to_string(),
            ]);
        }
        // ---- h264_nvenc (UNTESTED / PLACEHOLDER) ----
        (VideoEncoder::H264Nvenc, ExportPreset::Standard) => {
            args.extend([
                "-c:v".into(),
                "h264_nvenc".into(),
                "-rc".into(),
                "vbr".into(),
                "-cq".into(),
                "23".into(),
                "-b:v".into(),
                "4500k".into(),
                "-maxrate".into(),
                "6000k".into(),
                "-bufsize".into(),
                "9000k".into(),
                "-profile:v".into(),
                "high".into(),
                "-g".into(),
                GOP_LENGTH.to_string(),
            ]);
        }
        (VideoEncoder::H264Nvenc, ExportPreset::SmallFile) => {
            args.extend([
                "-c:v".into(),
                "h264_nvenc".into(),
                "-rc".into(),
                "cbr".into(),
                "-b:v".into(),
                "2500k".into(),
                "-maxrate".into(),
                "2500k".into(),
                "-bufsize".into(),
                "5000k".into(),
                "-profile:v".into(),
                "high".into(),
                "-g".into(),
                GOP_LENGTH.to_string(),
            ]);
        }
        // ---- h264_qsv (UNTESTED / PLACEHOLDER) ----
        (VideoEncoder::H264Qsv, ExportPreset::Standard) => {
            args.extend([
                "-c:v".into(),
                "h264_qsv".into(),
                "-global_quality".into(),
                "23".into(),
                "-b:v".into(),
                "4500k".into(),
                "-maxrate".into(),
                "6000k".into(),
                "-profile:v".into(),
                "high".into(),
                "-g".into(),
                GOP_LENGTH.to_string(),
            ]);
        }
        (VideoEncoder::H264Qsv, ExportPreset::SmallFile) => {
            args.extend([
                "-c:v".into(),
                "h264_qsv".into(),
                "-b:v".into(),
                "2500k".into(),
                "-maxrate".into(),
                "2500k".into(),
                "-bufsize".into(),
                "5000k".into(),
                "-profile:v".into(),
                "high".into(),
                "-g".into(),
                GOP_LENGTH.to_string(),
            ]);
        }
    }

    args
}

// ---------------------------------------------------------------------------
// Windows filter path escaping (S4 §2.4)
// ---------------------------------------------------------------------------

/// Escapes a Windows absolute path for use in FFmpeg filtergraph expressions.
/// Replaces `\` with `/` and escapes `:` as `\:`.
pub fn escape_filter_path(path: &str) -> String {
    path.replace('\\', "/").replace(':', r"\:")
}

// ---------------------------------------------------------------------------
// Filter graph builder
// ---------------------------------------------------------------------------

/// Resolves the effective export preset. If `max_file_size_mb` is provided (from template config)
/// and the estimated size at `Standard` exceeds that limit, automatically downgrades to `SmallFile` (2.5 Mbps CBR).
pub fn resolve_export_preset(
    requested: ExportPreset,
    duration_ms: u64,
    audio_kbps: u32,
    max_file_size_mb: Option<f64>,
) -> ExportPreset {
    if requested == ExportPreset::SmallFile {
        return ExportPreset::SmallFile;
    }
    if let Some(limit_mb) = max_file_size_mb {
        if limit_mb.is_finite() && limit_mb > 0.0 {
            let max_bytes = (limit_mb * 1024.0 * 1024.0) as u64;
            let est_standard =
                estimate_output_size_bytes(duration_ms, ExportPreset::Standard, audio_kbps);
            if est_standard > max_bytes {
                return ExportPreset::SmallFile;
            }
        }
    }
    requested
}

/// Builds the video+audio filtergraph for a `fill` layout composition.
/// Audio gain/resample is kept inside `-filter_complex` because FFmpeg rejects
/// combining `-filter_complex` and `-af` on the same output stream.
fn build_fill_filter_graph(
    source: &CompositionSource,
    segments: &[&TimelineSegment],
    crop: &Rect,
    output_fps: u32,
    audio_chain: &str,
    source_has_audio: bool,
    total_duration_ms: u64,
) -> Result<String, RenderBuildError> {
    let pc = crop_rect_to_pixels(crop, source.width, source.height);

    let mut filters = Vec::new();

    if segments.len() == 1 {
        // Single segment: simple trim → crop → scale → fps → format pipeline
        let seg = segments[0];
        let in_s = seg.in_ms as f64 / 1000.0;
        let out_s = seg.out_ms as f64 / 1000.0;

        let v_chain = format!(
            "[0:v]trim=start={in_s:.3}:end={out_s:.3},setpts=PTS-STARTPTS,\
             crop={cw}:{ch}:{cx}:{cy},\
             scale={ow}:{oh}:flags=lanczos,setsar=1,\
             fps={fps},format=yuv420p[vout]",
            in_s = in_s,
            out_s = out_s,
            cw = pc.w,
            ch = pc.h,
            cx = pc.x,
            cy = pc.y,
            ow = OUTPUT_WIDTH,
            oh = OUTPUT_HEIGHT,
            fps = output_fps,
        );
        filters.push(v_chain);

        if source_has_audio {
            let a_chain = format!(
                "[0:a]atrim=start={in_s:.3}:end={out_s:.3},asetpts=PTS-STARTPTS,{audio_chain}[aout]",
                in_s = in_s, out_s = out_s, audio_chain = audio_chain,
            );
            filters.push(a_chain);
        } else {
            let total_s = total_duration_ms as f64 / 1000.0;
            let a_chain = format!(
                "[1:a]atrim=start=0.000:end={total_s:.3},asetpts=PTS-STARTPTS[aout]",
                total_s = total_s,
            );
            filters.push(a_chain);
        }
    } else if source_has_audio {
        // Multi-segment with source audio: trim each, then concat video+audio
        let n = segments.len();
        let mut concat_inputs = String::new();

        for (i, seg) in segments.iter().enumerate() {
            let in_s = seg.in_ms as f64 / 1000.0;
            let out_s = seg.out_ms as f64 / 1000.0;

            let v_label = format!("v_seg_{}", i);
            let a_label = format!("a_seg_{}", i);

            filters.push(format!(
                "[0:v]trim=start={in_s:.3}:end={out_s:.3},setpts=PTS-STARTPTS[{label}]",
                in_s = in_s,
                out_s = out_s,
                label = v_label,
            ));
            filters.push(format!(
                "[0:a]atrim=start={in_s:.3}:end={out_s:.3},asetpts=PTS-STARTPTS[{label}]",
                in_s = in_s,
                out_s = out_s,
                label = a_label,
            ));

            concat_inputs.push_str(&format!("[{}][{}]", v_label, a_label));
        }

        filters.push(format!(
            "{inputs}concat=n={n}:v=1:a=1[v_timeline][a_timeline]",
            inputs = concat_inputs,
            n = n,
        ));

        filters.push(format!(
            "[v_timeline]crop={cw}:{ch}:{cx}:{cy},\
             scale={ow}:{oh}:flags=lanczos,setsar=1,\
             fps={fps},format=yuv420p[vout]",
            cw = pc.w,
            ch = pc.h,
            cx = pc.x,
            cy = pc.y,
            ow = OUTPUT_WIDTH,
            oh = OUTPUT_HEIGHT,
            fps = output_fps,
        ));
        filters.push(format!("[a_timeline]{}[aout]", audio_chain));
    } else {
        // Multi-segment without source audio: concat video only + silent track
        let n = segments.len();
        let mut concat_v_inputs = String::new();

        for (i, seg) in segments.iter().enumerate() {
            let in_s = seg.in_ms as f64 / 1000.0;
            let out_s = seg.out_ms as f64 / 1000.0;
            let v_label = format!("v_seg_{}", i);

            filters.push(format!(
                "[0:v]trim=start={in_s:.3}:end={out_s:.3},setpts=PTS-STARTPTS[{label}]",
                in_s = in_s,
                out_s = out_s,
                label = v_label,
            ));
            concat_v_inputs.push_str(&format!("[{}]", v_label));
        }

        filters.push(format!(
            "{inputs}concat=n={n}:v=1:a=0[v_timeline]",
            inputs = concat_v_inputs,
            n = n,
        ));
        filters.push(format!(
            "[v_timeline]crop={cw}:{ch}:{cx}:{cy},\
             scale={ow}:{oh}:flags=lanczos,setsar=1,\
             fps={fps},format=yuv420p[vout]",
            cw = pc.w,
            ch = pc.h,
            cx = pc.x,
            cy = pc.y,
            ow = OUTPUT_WIDTH,
            oh = OUTPUT_HEIGHT,
            fps = output_fps,
        ));
        let total_s = total_duration_ms as f64 / 1000.0;
        filters.push(format!(
            "[1:a]atrim=start=0.000:end={total_s:.3},asetpts=PTS-STARTPTS[aout]",
            total_s = total_s,
        ));
    }

    Ok(filters.join(";\n"))
}

/// Builds the audio filter chain (gain only in M1.7; normalization deferred).
fn build_audio_filter(gain_db: f64) -> String {
    let mut parts = Vec::new();
    if gain_db.abs() > 0.01 {
        parts.push(format!("volume={:.1}dB", gain_db));
    }
    // Ensure 48 kHz stereo output
    parts.push("aresample=48000".to_string());
    parts.join(",")
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Input-side seek safety margin in seconds.
/// Must be >= keyframe interval to ensure the trim filter
/// has access to the needed reference frames.
const INPUT_SEEK_MARGIN_S: f64 = 10.0;

/// Builds FFmpeg arguments from a validated `CompositionV1`.
///
/// Returns a `RenderPlan` containing the full argument array, filter graphs,
/// expected duration and frame count.
pub fn build_ffmpeg_args(
    comp: &CompositionV1,
    encoder: VideoEncoder,
    preset: ExportPreset,
    input_path: &Path,
    output_path: &Path,
    source_has_audio: bool,
) -> Result<RenderPlan, RenderBuildError> {
    build_ffmpeg_args_with_limit(
        comp,
        encoder,
        preset,
        input_path,
        output_path,
        source_has_audio,
        None,
    )
}

/// Builds FFmpeg arguments with an optional `max_file_size_mb` limit (auto-selecting
/// `small_file` CBR 2.5 Mbps when `standard` estimate would exceed `max_file_size_mb`).
pub fn build_ffmpeg_args_with_limit(
    comp: &CompositionV1,
    encoder: VideoEncoder,
    preset: ExportPreset,
    input_path: &Path,
    output_path: &Path,
    source_has_audio: bool,
    max_file_size_mb: Option<f64>,
) -> Result<RenderPlan, RenderBuildError> {
    if comp.timeline.is_empty() {
        return Err(RenderBuildError::EmptyTimeline);
    }

    let source = comp
        .sources
        .first()
        .ok_or_else(|| RenderBuildError::SourceNotFound("No sources in composition".into()))?;

    let crop = match &comp.layout {
        CompositionLayout::Fill { crop } => crop,
        CompositionLayout::BlurFit { .. } => {
            return Err(RenderBuildError::UnsupportedLayout("blur_fit".into()));
        }
        CompositionLayout::Split { .. } => {
            return Err(RenderBuildError::UnsupportedLayout("split".into()));
        }
    };

    // Validate crop
    if !crop.w.is_finite() || !crop.h.is_finite() || crop.w <= 0.0 || crop.h <= 0.0 {
        return Err(RenderBuildError::InvalidCrop);
    }

    // Gather segments referencing this source
    let segments: Vec<&TimelineSegment> = comp
        .timeline
        .iter()
        .filter(|seg| seg.source_id == source.id)
        .collect();

    if segments.is_empty() {
        return Err(RenderBuildError::EmptyTimeline);
    }

    // Compute expected output duration & M1.7 export frame rate (30 fps)
    let total_duration_ms: u64 = segments
        .iter()
        .map(|seg| (seg.out_ms as u64).saturating_sub(seg.in_ms as u64))
        .sum();

    let output_fps = DEFAULT_FPS;
    let expected_frames = ((total_duration_ms as f64 / 1000.0) * output_fps as f64).ceil() as u64;

    let audio_kbps = comp.export.audio_kbps.max(64);
    let effective_preset =
        resolve_export_preset(preset, total_duration_ms, audio_kbps, max_file_size_mb);

    // Audio filter chain (embedded inside -filter_complex)
    let audio_filter = build_audio_filter(comp.audio.gain_db);

    // Build the combined video+audio filtergraph
    let video_filter = build_fill_filter_graph(
        source,
        &segments,
        crop,
        output_fps,
        &audio_filter,
        source_has_audio,
        total_duration_ms,
    )?;

    // Input-side seek: seek to the earliest segment minus safety margin.
    // `-copyts` preserves source PTS so `trim=start=...:end=...` operates accurately
    // on the source timeline regardless of keyframe alignment.
    let earliest_in_s = segments
        .iter()
        .map(|s| s.in_ms as f64 / 1000.0)
        .fold(f64::MAX, f64::min);
    let seek_s = (earliest_in_s - INPUT_SEEK_MARGIN_S).max(0.0);

    // Build argument array
    let mut args = Vec::new();

    args.extend(["-hide_banner".into(), "-y".into()]);

    // Input-side seek (optimization; `-copyts` keeps original timestamps for filter trim)
    if seek_s > 0.5 {
        args.extend(["-ss".into(), format!("{:.3}", seek_s), "-copyts".into()]);
    }

    args.extend(["-i".into(), input_path.to_string_lossy().to_string()]);

    // If source has no audio, generate a silent audio track
    if !source_has_audio {
        args.extend([
            "-f".into(),
            "lavfi".into(),
            "-i".into(),
            format!(
                "anullsrc=channel_layout=stereo:sample_rate={}",
                DEFAULT_AUDIO_SAMPLE_RATE
            ),
        ]);
    }

    // Filter complex + output maps
    args.extend(["-filter_complex".into(), video_filter.clone()]);
    args.extend(["-map".into(), "[vout]".into()]);
    args.extend(["-map".into(), "[aout]".into()]);

    // Audio codec (mono sources upmixed to stereo via `-ac 2`)
    args.extend([
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        format!("{}k", audio_kbps),
        "-ar".into(),
        DEFAULT_AUDIO_SAMPLE_RATE.to_string(),
        "-ac".into(),
        "2".into(),
    ]);

    // Encoder-specific video args
    args.extend(encoder_args(encoder, effective_preset));

    // Pixel format and faststart
    args.extend([
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-movflags".into(),
        "+faststart".into(),
    ]);

    // Progress output
    args.extend(["-progress".into(), "pipe:1".into()]);

    // Output
    args.push(output_path.to_string_lossy().to_string());

    Ok(RenderPlan {
        args,
        video_filter_graph: video_filter,
        audio_filter,
        expected_duration_ms: total_duration_ms,
        expected_output_frames: expected_frames,
        encoder: encoder.as_ffmpeg_name().to_string(),
        preset: match effective_preset {
            ExportPreset::Standard => "standard".to_string(),
            ExportPreset::SmallFile => "small_file".to_string(),
        },
    })
}

/// Estimates the output file size in bytes for a given duration and preset.
pub fn estimate_output_size_bytes(duration_ms: u64, preset: ExportPreset, audio_kbps: u32) -> u64 {
    let duration_s = duration_ms as f64 / 1000.0;
    let video_kbps: f64 = match preset {
        ExportPreset::Standard => 4500.0,
        ExportPreset::SmallFile => 2500.0,
    };
    let total_kbps = video_kbps + audio_kbps as f64;
    // bits = kbps * 1000 * seconds; bytes = bits / 8
    ((total_kbps * 1000.0 * duration_s) / 8.0) as u64
}

// ---------------------------------------------------------------------------
// Filter name registry (for GPL denylist cross-check test)
// ---------------------------------------------------------------------------

/// Returns all FFmpeg filter names that `build_ffmpeg_args` can emit.
/// Used by unit tests to cross-check against the GPL denylist.
pub fn all_emitted_filter_names() -> Vec<&'static str> {
    vec![
        "trim",
        "atrim",
        "setpts",
        "asetpts",
        "crop",
        "scale",
        "setsar",
        "fps",
        "format",
        "concat",
        "anull",
        "volume",
        "aresample",
        "anullsrc", // lavfi source, not a filter per se
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composition::*;
    use std::collections::HashSet;

    fn make_test_source(w: u32, h: u32, fps: f64, dur_ms: u32) -> CompositionSource {
        CompositionSource {
            id: "src-1".into(),
            provider: "youtube".into(),
            video_id: "test123".into(),
            path: "cache/test.mp4".into(),
            preview_path: None,
            absolute_start_ms: 0,
            duration_ms: dur_ms,
            width: w,
            height: h,
            fps,
        }
    }

    fn make_test_comp(
        source: CompositionSource,
        segments: Vec<TimelineSegment>,
        crop: Rect,
        canvas_fps: u32,
        gain_db: f64,
    ) -> CompositionV1 {
        CompositionV1 {
            schema_version: 1,
            id: uuid::Uuid::new_v4().to_string(),
            canvas: CompositionCanvas {
                width: 1080,
                height: 1920,
                fps: canvas_fps,
            },
            sources: vec![source],
            timeline: segments,
            layout: CompositionLayout::Fill { crop },
            captions: CompositionCaptions {
                enabled: false,
                status: "none".into(),
                language: "id".into(),
                words: Vec::new(),
                edits: Vec::new(),
                style: default_caption_style(),
            },
            overlays: Vec::new(),
            audio: CompositionAudio {
                gain_db,
                normalize: AudioNormalizeConfig {
                    enabled: false,
                    target_lufs: -14.0,
                },
            },
            template: None,
            export: CompositionExportConfig {
                preset: "standard".into(),
                audio_kbps: 160,
            },
        }
    }

    // --- Crop parity table (shared with TS cropMath.ts tests) ---

    #[test]
    fn test_crop_rect_to_pixels_parity_table() {
        // Case 1: 1920×1080, default centered 9:16 crop
        let pc = crop_rect_to_pixels(
            &Rect {
                x: 0.341797,
                y: 0.0,
                w: 0.316406,
                h: 1.0,
            },
            1920,
            1080,
        );
        assert_eq!(pc.w, 608); // round(0.316406 * 1920) = 608, already even
        assert_eq!(pc.h, 1080);
        assert_eq!(pc.x, 656); // round(0.341797 * 1920) = 656
        assert_eq!(pc.y, 0);

        // Case 2: 1920×1080, zoomed 2× (smaller rect)
        let pc2 = crop_rect_to_pixels(
            &Rect {
                x: 0.420898,
                y: 0.25,
                w: 0.158203,
                h: 0.5,
            },
            1920,
            1080,
        );
        assert_eq!(pc2.w, 304); // round(0.158203 * 1920) = 304
        assert_eq!(pc2.h, 540);
        assert!(pc2.x + pc2.w <= 1920);
        assert!(pc2.y + pc2.h <= 1080);

        // Case 3: 1280×720 (exact 9:16 inside 16:9: 404.9997 -> 404 even)
        let pc3 = crop_rect_to_pixels(
            &Rect {
                x: 0.341797,
                y: 0.0,
                w: 0.316406,
                h: 1.0,
            },
            1280,
            720,
        );
        assert_eq!(pc3.w, 404);
        assert_eq!(pc3.h, 720);

        // Case 4: 1080×1920 portrait source (already 9:16)
        let pc4 = crop_rect_to_pixels(
            &Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
            1080,
            1920,
        );
        assert_eq!(pc4.w, 1080);
        assert_eq!(pc4.h, 1920);
        assert_eq!(pc4.x, 0);
        assert_eq!(pc4.y, 0);

        // Case 5: Non-standard 1366×768 (768 * 9/16 / 1366 = 432 / 1366 ≈ 0.316252)
        let pc5 = crop_rect_to_pixels(
            &Rect {
                x: 0.341874,
                y: 0.0,
                w: 0.316252,
                h: 1.0,
            },
            1366,
            768,
        );
        assert_eq!(pc5.w, 432);
        assert_eq!(pc5.h, 768);
        assert!(pc5.w.is_multiple_of(2), "Width must be even");
        assert!(pc5.h.is_multiple_of(2), "Height must be even");
        assert!(pc5.x + pc5.w <= 1366);
        assert!(pc5.y + pc5.h <= 768);
    }

    // --- Golden snapshot: fill default 1920×1080 ---

    #[test]
    fn test_golden_fill_default_1920x1080() {
        let source = make_test_source(1920, 1080, 60.0, 70_000);
        let crop = Rect {
            x: 0.341797,
            y: 0.0,
            w: 0.316406,
            h: 1.0,
        };
        let seg = TimelineSegment {
            id: "s1".into(),
            source_id: "src-1".into(),
            in_ms: 5000,
            out_ms: 35000,
        };
        let comp = make_test_comp(source, vec![seg], crop, 30, 0.0);

        let plan = build_ffmpeg_args(
            &comp,
            VideoEncoder::H264Amf,
            ExportPreset::Standard,
            Path::new("C:\\test\\input.mp4"),
            Path::new("C:\\test\\output.mp4"),
            true,
        )
        .unwrap();

        // Check expected duration
        assert_eq!(plan.expected_duration_ms, 30_000);
        assert_eq!(plan.expected_output_frames, 900); // 30s * 30fps

        // Video filter must contain trim, crop, scale, fps, format
        assert!(plan
            .video_filter_graph
            .contains("trim=start=5.000:end=35.000"));
        assert!(plan.video_filter_graph.contains("crop=608:1080:656:0"));
        assert!(plan
            .video_filter_graph
            .contains("scale=1080:1920:flags=lanczos"));
        assert!(plan.video_filter_graph.contains("fps=30"));
        assert!(plan.video_filter_graph.contains("format=yuv420p"));
        assert!(plan.video_filter_graph.contains("setsar=1"));

        // Check encoder args
        assert!(plan.args.contains(&"h264_amf".to_string()));
        assert!(plan.args.contains(&"vbr_peak".to_string()));
        assert!(plan.args.contains(&"+faststart".to_string()));
        assert!(plan.args.contains(&"bt709".to_string()));

        // Audio codec
        assert!(plan.args.contains(&"aac".to_string()));
        assert!(plan.args.contains(&"48000".to_string()));

        // Input-side seek should be absent (5s - 10s margin = negative → no seek)
        assert!(!plan.args.iter().take(4).any(|a| a == "-ss"));

        // Progress pipe
        assert!(plan.args.contains(&"pipe:1".to_string()));
    }

    // --- Golden: zoomed/panned crop ---

    #[test]
    fn test_golden_zoomed_panned_crop() {
        let source = make_test_source(1920, 1080, 60.0, 120_000);
        let crop = Rect {
            x: 0.1,
            y: 0.2,
            w: 0.158203,
            h: 0.5,
        };
        let seg = TimelineSegment {
            id: "s1".into(),
            source_id: "src-1".into(),
            in_ms: 60000,
            out_ms: 90000,
        };
        let comp = make_test_comp(source, vec![seg], crop, 30, -3.0);

        let plan = build_ffmpeg_args(
            &comp,
            VideoEncoder::H264Mf,
            ExportPreset::Standard,
            Path::new("C:\\test\\input.mp4"),
            Path::new("C:\\test\\output.mp4"),
            true,
        )
        .unwrap();

        // Input-side seek: 60s - 10s = 50s with -copyts
        assert!(plan.args.contains(&"-ss".to_string()));
        let ss_idx = plan.args.iter().position(|a| a == "-ss").unwrap();
        assert_eq!(plan.args[ss_idx + 1], "50.000");
        assert_eq!(plan.args[ss_idx + 2], "-copyts");

        // h264_mf must NOT have -profile:v
        assert!(!plan.args.contains(&"-profile:v".to_string()));
        assert!(plan.args.contains(&"h264_mf".to_string()));
        assert!(plan.args.contains(&"pc_vbr".to_string()));

        // Audio gain inside -filter_complex (never -af alongside -filter_complex)
        assert!(!plan.args.contains(&"-af".to_string()));
        assert!(plan.audio_filter.contains("volume=-3.0dB"));
        assert!(plan
            .video_filter_graph
            .contains("volume=-3.0dB,aresample=48000"));
    }

    // --- Golden: 30 fps source ---

    #[test]
    fn test_golden_30fps_source() {
        let source = make_test_source(1920, 1080, 30.0, 45_000);
        let crop = Rect {
            x: 0.341797,
            y: 0.0,
            w: 0.316406,
            h: 1.0,
        };
        let seg = TimelineSegment {
            id: "s1".into(),
            source_id: "src-1".into(),
            in_ms: 2000,
            out_ms: 22000,
        };
        let comp = make_test_comp(source, vec![seg], crop, 30, 0.0);

        let plan = build_ffmpeg_args(
            &comp,
            VideoEncoder::H264Amf,
            ExportPreset::Standard,
            Path::new("C:\\test\\input30.mp4"),
            Path::new("C:\\test\\out30.mp4"),
            true,
        )
        .unwrap();

        assert_eq!(plan.expected_duration_ms, 20_000);
        assert_eq!(plan.expected_output_frames, 600);
        assert!(plan.video_filter_graph.contains("fps=30"));
        assert!(plan.video_filter_graph.contains("crop=608:1080:656:0"));
    }

    // --- Golden: multi-segment ---

    #[test]
    fn test_golden_multi_segment() {
        let source = make_test_source(1920, 1080, 30.0, 300_000);
        let crop = Rect {
            x: 0.341797,
            y: 0.0,
            w: 0.316406,
            h: 1.0,
        };
        let segs = vec![
            TimelineSegment {
                id: "s1".into(),
                source_id: "src-1".into(),
                in_ms: 10000,
                out_ms: 20000,
            },
            TimelineSegment {
                id: "s2".into(),
                source_id: "src-1".into(),
                in_ms: 100000,
                out_ms: 115000,
            },
        ];
        let comp = make_test_comp(source, segs, crop, 30, 0.0);

        let plan = build_ffmpeg_args(
            &comp,
            VideoEncoder::H264Amf,
            ExportPreset::SmallFile,
            Path::new("C:\\test\\input.mp4"),
            Path::new("C:\\test\\output.mp4"),
            true,
        )
        .unwrap();

        // 10s + 15s = 25s
        assert_eq!(plan.expected_duration_ms, 25_000);
        assert_eq!(plan.expected_output_frames, 750);

        // Filter must have concat
        assert!(plan.video_filter_graph.contains("concat=n=2:v=1:a=1"));
        assert!(plan.video_filter_graph.contains("v_seg_0"));
        assert!(plan.video_filter_graph.contains("v_seg_1"));
        assert!(plan.video_filter_graph.contains("a_seg_0"));
        assert!(plan.video_filter_graph.contains("a_seg_1"));

        // small_file preset
        assert!(plan.args.contains(&"cbr".to_string()));
        assert!(plan.args.contains(&"2500k".to_string()));
    }

    // --- Golden: 1280×720 source ---

    #[test]
    fn test_golden_1280x720_source() {
        let source = make_test_source(1280, 720, 30.0, 60_000);
        let crop = Rect {
            x: 0.341797,
            y: 0.0,
            w: 0.316406,
            h: 1.0,
        };
        let seg = TimelineSegment {
            id: "s1".into(),
            source_id: "src-1".into(),
            in_ms: 0,
            out_ms: 30000,
        };
        let comp = make_test_comp(source, vec![seg], crop, 30, 0.0);

        let plan = build_ffmpeg_args(
            &comp,
            VideoEncoder::H264Amf,
            ExportPreset::Standard,
            Path::new("input.mp4"),
            Path::new("output.mp4"),
            true,
        )
        .unwrap();

        assert!(plan.video_filter_graph.contains("crop=404:720:"));
        assert!(plan.video_filter_graph.contains("scale=1080:1920"));
    }

    // --- Golden: portrait source 1080×1920 ---

    #[test]
    fn test_golden_portrait_source() {
        let source = make_test_source(1080, 1920, 30.0, 60_000);
        let crop = Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        };
        let seg = TimelineSegment {
            id: "s1".into(),
            source_id: "src-1".into(),
            in_ms: 0,
            out_ms: 30000,
        };
        let comp = make_test_comp(source, vec![seg], crop, 30, 0.0);

        let plan = build_ffmpeg_args(
            &comp,
            VideoEncoder::H264Amf,
            ExportPreset::Standard,
            Path::new("input.mp4"),
            Path::new("output.mp4"),
            true,
        )
        .unwrap();

        // Full frame → crop 1080×1920
        assert!(plan.video_filter_graph.contains("crop=1080:1920:0:0"));
    }

    // --- Golden: no-audio source ---

    #[test]
    fn test_golden_no_audio_source() {
        let source = make_test_source(1920, 1080, 30.0, 60_000);
        let crop = Rect {
            x: 0.341797,
            y: 0.0,
            w: 0.316406,
            h: 1.0,
        };
        let seg = TimelineSegment {
            id: "s1".into(),
            source_id: "src-1".into(),
            in_ms: 0,
            out_ms: 30000,
        };
        let comp = make_test_comp(source, vec![seg], crop, 30, 0.0);

        let plan = build_ffmpeg_args(
            &comp,
            VideoEncoder::H264Amf,
            ExportPreset::Standard,
            Path::new("input.mp4"),
            Path::new("output.mp4"),
            false, // no audio
        )
        .unwrap();

        // Should have anullsrc for silent audio and must NOT reference [0:a]
        assert!(plan
            .args
            .contains(&"anullsrc=channel_layout=stereo:sample_rate=48000".to_string()));
        assert!(!plan.video_filter_graph.contains("[0:a]"));
        assert!(plan
            .video_filter_graph
            .contains("[1:a]atrim=start=0.000:end=30.000"));
        assert!(plan.args.contains(&"[aout]".to_string()));
    }

    // --- Filter denylist + LGPL capability fixture cross-check ---

    #[test]
    fn test_no_emitted_filter_in_gpl_denylist_and_all_in_lgpl_capabilities() {
        let denylist_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../docs/benchmarks/gpl-filters-denylist.txt");
        let lgpl_filters_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/capabilities/ffmpeg_lgpl_filters.txt");

        let deny_content = std::fs::read_to_string(&denylist_path)
            .unwrap_or_else(|_| panic!("Cannot read GPL denylist at {:?}", denylist_path));
        let lgpl_content = std::fs::read_to_string(&lgpl_filters_path).unwrap_or_else(|_| {
            panic!(
                "Cannot read LGPL filters fixture at {:?}",
                lgpl_filters_path
            )
        });

        let denied: HashSet<&str> = deny_content
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| l.split_whitespace().next().unwrap_or(""))
            .filter(|s| !s.is_empty())
            .collect();

        // Parse filter lines like ` .. crop              V->V       Crop the input video.`
        let lgpl_filters: HashSet<&str> = lgpl_content
            .lines()
            .filter_map(|l| {
                let trimmed = l.trim_start();
                let mut cols = trimmed.split_whitespace();
                let flags = cols.next()?;
                let name = cols.next()?;
                let io = cols.next()?;
                if flags.len() <= 4 && io.contains("->") {
                    Some(name)
                } else {
                    None
                }
            })
            .collect();

        let emitted = all_emitted_filter_names();
        for f in &emitted {
            assert!(
                !denied.contains(f),
                "Render builder emits GPL-only filter '{}' which is in the denylist",
                f
            );
            assert!(
                lgpl_filters.contains(f),
                "Render builder emits filter '{}' which is NOT in the pinned LGPL capability fixture",
                f
            );
        }
    }

    #[test]
    fn test_resolve_export_preset_auto_small_file() {
        // 60s at Standard (4660 kbps) = 34.95 MB
        let p_fit = resolve_export_preset(ExportPreset::Standard, 60_000, 160, Some(40.0));
        assert_eq!(p_fit, ExportPreset::Standard);

        let p_exceed = resolve_export_preset(ExportPreset::Standard, 60_000, 160, Some(25.0));
        assert_eq!(p_exceed, ExportPreset::SmallFile);
    }

    // --- Windows path escaping ---

    #[test]
    fn test_escape_filter_path() {
        assert_eq!(
            escape_filter_path(r"C:\Users\Test\videos\clip.mp4"),
            r"C\:/Users/Test/videos/clip.mp4"
        );
        assert_eq!(
            escape_filter_path(r"F:\coding\clipper-helper\output.ass"),
            r"F\:/coding/clipper-helper/output.ass"
        );
    }

    // --- Estimate output size ---

    #[test]
    fn test_estimate_output_size() {
        // 30s at standard (4500 + 160 kbps) = 4660 kbps
        // 4660 * 1000 * 30 / 8 = 17_475_000 bytes
        let size = estimate_output_size_bytes(30_000, ExportPreset::Standard, 160);
        assert_eq!(size, 17_475_000);

        // 60s at small_file (2500 + 160 kbps) = 2660 kbps
        // 2660 * 1000 * 60 / 8 = 19_950_000 bytes
        let size2 = estimate_output_size_bytes(60_000, ExportPreset::SmallFile, 160);
        assert_eq!(size2, 19_950_000);
    }
}
