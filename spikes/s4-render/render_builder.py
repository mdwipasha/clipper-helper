"""
Reference Render Builder for Clipper Helper (Spike S4)
Converts a Composition JSON (DATA_MODEL §7 v1) into FFmpeg CLI arguments.
Strictly targets the pinned LGPLv3 FFmpeg build and adheres to AGENTS.md.
"""

import os
from typing import Any, Dict, List, Optional


def escape_filter_path(p: str) -> str:
    """
    Escapes a Windows filesystem path for use inside FFmpeg filtergraphs.
    Replaces backslashes with forward slashes and escapes colons (e.g. drive letters).
    """
    posix_path = p.replace("\\", "/")
    return posix_path.replace(":", r"\:")


def calculate_anchor_position(anchor: str, margin_x: int, margin_y: int) -> tuple[str, str]:
    """Returns FFmpeg overlay x, y coordinate expressions for given anchor and margins."""
    if anchor == "top_left":
        return f"{margin_x}", f"{margin_y}"
    elif anchor == "top_right":
        return f"W-w-{margin_x}", f"{margin_y}"
    elif anchor == "bottom_left":
        return f"{margin_x}", f"H-h-{margin_y}"
    elif anchor == "bottom_right":
        return f"W-w-{margin_x}", f"H-h-{margin_y}"
    elif anchor == "top_center":
        return "(W-w)/2", f"{margin_y}"
    elif anchor == "bottom_center":
        return "(W-w)/2", f"H-h-{margin_y}"
    elif anchor == "center":
        return "(W-w)/2", "(H-h)/2"
    else:
        # Default to bottom_center
        return "(W-w)/2", f"H-h-{margin_y}"


def build_ffmpeg_args(
    composition: Dict[str, Any],
    encoder: str,
    output_path: str,
    ffmpeg_exe: Optional[str] = None,
    caption_ass_path: Optional[str] = None,
    fonts_dir: Optional[str] = None,
    logo_path: Optional[str] = None,
    hwaccel: Optional[str] = None,
    progress_pipe: bool = True
) -> List[str]:
    """
    Pure builder function generating full FFmpeg CLI argument list from a Composition JSON.
    """
    if ffmpeg_exe is None:
        ffmpeg_exe = "ffmpeg"

    cmd: List[str] = [ffmpeg_exe, "-y", "-hide_banner"]

    # 1. Hardware decode acceleration if specified
    if hwaccel:
        cmd.extend(["-hwaccel", hwaccel])

    # 2. Input sources mapping
    # Primary video source is input 0
    sources = composition.get("sources", [])
    if not sources:
        raise ValueError("Composition must contain at least one source in 'sources'")

    main_source = sources[0]
    cmd.extend(["-i", main_source["path"]])

    # If logo overlay exists and a file path is provided, add as input 1
    logo_input_idx = -1
    overlays = composition.get("overlays", [])
    has_logo = False
    for ov in overlays:
        if ov.get("type") == "logo" and logo_path and os.path.exists(logo_path):
            logo_input_idx = 1
            has_logo = True
            cmd.extend(["-i", logo_path])
            break

    # 3. Canvas & Filtergraph construction
    canvas = composition.get("canvas", {"width": 1080, "height": 1920, "fps": 30})
    out_w = canvas.get("width", 1080)
    out_h = canvas.get("height", 1920)
    fps = canvas.get("fps", 30)

    timeline = composition.get("timeline", [])
    if not timeline:
        # Fallback to full duration if no segments
        timeline = [{"id": "t1", "sourceId": main_source["id"], "inMs": 0, "outMs": main_source.get("durationMs", 60000)}]

    filter_chains: List[str] = []

    # A. Multi-segment timeline trimming & concat
    num_segs = len(timeline)
    for i, seg in enumerate(timeline):
        in_sec = seg["inMs"] / 1000.0
        out_sec = seg["outMs"] / 1000.0
        filter_chains.append(f"[0:v]trim=start={in_sec:.3f}:end={out_sec:.3f},setpts=PTS-STARTPTS[v_seg_{i}]")
        filter_chains.append(f"[0:a]atrim=start={in_sec:.3f}:end={out_sec:.3f},asetpts=PTS-STARTPTS[a_seg_{i}]")

    if num_segs > 1:
        cat_inputs = "".join(f"[v_seg_{i}][a_seg_{i}]" for i in range(num_segs))
        filter_chains.append(f"{cat_inputs}concat=n={num_segs}:v=1:a=1[v_timeline][a_timeline]")
        curr_v = "[v_timeline]"
        curr_a = "[a_timeline]"
    else:
        curr_v = "[v_seg_0]"
        curr_a = "[a_seg_0]"

    # B. Video Layout Mode
    layout = composition.get("layout", {"mode": "fill", "crop": {"x": 0.25, "y": 0.0, "w": 0.5, "h": 1.0}})
    mode = layout.get("mode", "fill")

    if mode == "fill":
        crop = layout.get("crop", {"x": 0.0, "y": 0.0, "w": 1.0, "h": 1.0})
        cx = crop.get("x", 0.0)
        cy = crop.get("y", 0.0)
        cw = crop.get("w", 1.0)
        ch = crop.get("h", 1.0)
        filter_chains.append(
            f"{curr_v}crop=w=iw*{cw:.4f}:h=ih*{ch:.4f}:x=iw*{cx:.4f}:y=ih*{cy:.4f},"
            f"scale={out_w}:{out_h}[v_layout]"
        )
        curr_v = "[v_layout]"

    elif mode == "blur_fit":
        # LGPL compliance: gblur (or avgblur) instead of GPL-only boxblur
        blur_sigma = int(layout.get("blur", 3.0) * 10)
        if blur_sigma < 5:
            blur_sigma = 25
        filter_chains.append(f"{curr_v}split=2[v_bg_in][v_fg_in]")
        filter_chains.append(
            f"[v_bg_in]scale={out_w}:{out_h}:force_original_aspect_ratio=increase,"
            f"crop={out_w}:{out_h},gblur=sigma={blur_sigma}:steps=2[v_bg]"
        )
        filter_chains.append(
            f"[v_fg_in]scale={out_w}:{out_h}:force_original_aspect_ratio=decrease[v_fg]"
        )
        filter_chains.append(f"[v_bg][v_fg]overlay=(W-w)/2:(H-h)/2[v_layout]")
        curr_v = "[v_layout]"

    elif mode == "split":
        top = layout.get("top", {"x": 0.0, "y": 0.0, "w": 1.0, "h": 0.5})
        bottom = layout.get("bottom", {"x": 0.0, "y": 0.5, "w": 1.0, "h": 0.5})
        ratio = layout.get("ratio", 0.5)
        top_h = int(out_h * ratio)
        # Ensure even pixel height
        if top_h % 2 != 0:
            top_h += 1
        bottom_h = out_h - top_h

        filter_chains.append(f"{curr_v}split=2[v_split_top_in][v_split_bot_in]")
        filter_chains.append(
            f"[v_split_top_in]crop=w=iw*{top.get('w', 1.0):.4f}:h=ih*{top.get('h', 0.5):.4f}:"
            f"x=iw*{top.get('x', 0.0):.4f}:y=ih*{top.get('y', 0.0):.4f},"
            f"scale={out_w}:{top_h}[v_top]"
        )
        filter_chains.append(
            f"[v_split_bot_in]crop=w=iw*{bottom.get('w', 1.0):.4f}:h=ih*{bottom.get('h', 0.5):.4f}:"
            f"x=iw*{bottom.get('x', 0.0):.4f}:y=ih*{bottom.get('y', 0.5):.4f},"
            f"scale={out_w}:{bottom_h}[v_bottom]"
        )
        filter_chains.append(f"[v_top][v_bottom]vstack[v_layout]")
        curr_v = "[v_layout]"

    # C. Overlay: Logo
    if has_logo and logo_input_idx >= 0:
        for ov in overlays:
            if ov.get("type") == "logo":
                anchor = ov.get("anchor", "top_right")
                margin_pct = ov.get("marginPct", 0.05)
                width_pct = ov.get("widthPct", 0.20)
                opacity = ov.get("opacity", 1.0)

                logo_w = int(out_w * width_pct)
                margin_x = int(out_w * margin_pct)
                margin_y = int(out_h * margin_pct)
                pos_x, pos_y = calculate_anchor_position(anchor, margin_x, margin_y)

                filter_chains.append(
                    f"[{logo_input_idx}:v]scale={logo_w}:-1,format=rgba,"
                    f"colorchannelmixer=aa={opacity:.2f}[logo_scaled]"
                )
                filter_chains.append(f"{curr_v}[logo_scaled]overlay={pos_x}:{pos_y}[v_logo]")
                curr_v = "[v_logo]"
                break

    # D. Overlay: Text Overlays (if not ASS)
    for i, ov in enumerate(overlays):
        if ov.get("type") == "text":
            text_str = ov.get("text", "")
            if text_str:
                anchor = ov.get("anchor", "top_center")
                margin_pct = ov.get("marginPct", 0.08)
                margin_y = int(out_h * margin_pct)
                margin_x = int(out_w * margin_pct)
                pos_x, pos_y = calculate_anchor_position(anchor, margin_x, margin_y)
                color = ov.get("color", "white")
                size = int(out_h * ov.get("sizePct", 0.03))

                # Simple drawtext with bundled font if available
                font_opt = ""
                if fonts_dir:
                    montserrat_font = os.path.join(fonts_dir, "Montserrat-Bold.ttf")
                    if os.path.exists(montserrat_font):
                        font_esc = escape_filter_path(montserrat_font)
                        font_opt = f":fontfile='{font_esc}'"

                filter_chains.append(
                    f"{curr_v}drawtext=text='{text_str}'{font_opt}:fontsize={size}:"
                    f"fontcolor={color}:x={pos_x}:y={pos_y}[v_text_{i}]"
                )
                curr_v = f"[v_text_{i}]"

    # E. Captions Burn-In via libass
    captions = composition.get("captions", {})
    if captions.get("enabled", True) and caption_ass_path and os.path.exists(caption_ass_path):
        ass_esc = escape_filter_path(caption_ass_path)
        fontsdir_opt = ""
        if fonts_dir and os.path.exists(fonts_dir):
            fonts_esc = escape_filter_path(fonts_dir)
            fontsdir_opt = f":fontsdir='{fonts_esc}'"

        filter_chains.append(f"{curr_v}ass=filename='{ass_esc}'{fontsdir_opt}[v_caption]")
        curr_v = "[v_caption]"

    # F. Framerate conversion & pixel format alignment
    filter_chains.append(f"{curr_v}fps={fps},format=yuv420p[v_final]")
    curr_v = "[v_final]"

    # G. Audio Processing: Gain & Loudness Normalization
    audio_cfg = composition.get("audio", {"gainDb": 0.0, "normalize": {"enabled": True, "targetLufs": -16.0}})
    gain_db = audio_cfg.get("gainDb", 0.0)
    norm_cfg = audio_cfg.get("normalize", {"enabled": True, "targetLufs": -16.0})

    audio_filters = []
    if abs(gain_db) > 0.01:
        audio_filters.append(f"volume={gain_db:.1f}dB")
    if norm_cfg.get("enabled", True):
        target_lufs = norm_cfg.get("targetLufs", -16.0)
        audio_filters.append(f"loudnorm=I={target_lufs:.1f}:TP=-1.5:LRA=11")
    audio_filters.append("aresample=48000")

    if audio_filters:
        filter_chains.append(f"{curr_a}{','.join(audio_filters)}[a_final]")
        curr_a = "[a_final]"

    # Assemble complete complex filtergraph
    full_filtergraph = ";".join(filter_chains)
    cmd.extend(["-filter_complex", full_filtergraph])
    cmd.extend(["-map", curr_v, "-map", curr_a])

    # 4. Encoder & Rate Control Configuration
    export_cfg = composition.get("export", {"preset": "standard", "audioKbps": 192})
    preset = export_cfg.get("preset", "standard")
    audio_kbps = export_cfg.get("audioKbps", 192)

    if encoder == "h264_amf":
        cmd.extend(["-c:v", "h264_amf"])
        cmd.extend(["-usage", "transcoding"])
        if preset == "small_file":
            cmd.extend(["-rc", "cbr", "-b:v", "2500k", "-maxrate", "2500k", "-bufsize", "5000k"])
        else:  # standard (quality target)
            cmd.extend(["-rc", "vbr_latency", "-b:v", "4500k", "-maxrate", "6000k", "-bufsize", "9000k", "-quality", "quality"])
        cmd.extend(["-profile:v", "high", "-level", "4.2", "-g", str(fps)])

    elif encoder == "h264_mf":
        cmd.extend(["-c:v", "h264_mf"])
        if preset == "small_file":
            cmd.extend(["-b:v", "2500k", "-rate_control", "cbr"])
        else:  # standard
            cmd.extend(["-b:v", "4500k", "-rate_control", "pc_vbr"])
        # Note: h264_mf does not accept -profile:v flag (handled internally by MFT)
        cmd.extend(["-g", str(fps)])

    elif encoder == "libopenh264":
        # Reference software encoder
        cmd.extend(["-c:v", "libopenh264"])
        if preset == "small_file":
            cmd.extend(["-b:v", "2500k"])
        else:
            cmd.extend(["-b:v", "4500k"])
        cmd.extend(["-profile:v", "high", "-g", str(fps)])

    else:
        # Fallback / reference encoder
        cmd.extend(["-c:v", encoder, "-b:v", "4500k", "-g", str(fps)])

    # Color space tags (BT.709 standard for web/mobile vertical video)
    cmd.extend([
        "-color_primaries", "bt709",
        "-color_trc", "bt709",
        "-colorspace", "bt709",
        "-color_range", "tv"
    ])

    # Audio encoding
    cmd.extend(["-c:a", "aac", "-b:a", f"{audio_kbps}k"])

    # Output container optimizations
    cmd.extend(["-movflags", "+faststart"])

    # Progress reporting pipe
    if progress_pipe:
        cmd.extend(["-progress", "pipe:1"])

    cmd.append(output_path)
    return cmd
