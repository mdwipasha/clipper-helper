"""
Post-Render Verification Module for Clipper Helper (Spike S4)
Implements ffprobe-based and container checks per PRD FR-CHK-2 and ARCHITECTURE §7.
Verifies duration, resolution (1080x1920), fps, audio stream, moov at start (+faststart), and color tags.
"""

import json
import os
import struct
import subprocess
from typing import Any, Dict, List, Tuple


def check_faststart_moov_at_start(file_path: str) -> Tuple[bool, str]:
    """
    Inspects MP4 top-level box/atom sequence.
    Returns (True, 'moov precedes mdat') if +faststart is properly applied.
    """
    if not os.path.exists(file_path) or os.path.getsize(file_path) < 32:
        return False, "File too small or does not exist"

    with open(file_path, "rb") as f:
        atoms: List[str] = []
        file_size = os.path.getsize(file_path)
        offset = 0

        while offset < file_size:
            f.seek(offset)
            header = f.read(8)
            if len(header) < 8:
                break
            size, atom_type = struct.unpack(">I4s", header)
            atom_str = atom_type.decode("latin1", errors="ignore")
            atoms.append(atom_str)

            if size == 1:
                # 64-bit size
                ext_header = f.read(8)
                if len(ext_header) < 8:
                    break
                size = struct.unpack(">Q", ext_header)[0]
            elif size == 0:
                # Atom extends to end of file
                break

            if "moov" in atoms and "mdat" not in atoms:
                return True, "moov precedes mdat (faststart confirmed)"
            if "mdat" in atoms and "moov" not in atoms:
                return False, "mdat precedes moov (faststart NOT applied)"

            offset += size
            if size <= 0:
                break

    if "moov" in atoms and "mdat" in atoms:
        moov_idx = atoms.index("moov")
        mdat_idx = atoms.index("mdat")
        if moov_idx < mdat_idx:
            return True, f"moov at index {moov_idx} precedes mdat at index {mdat_idx}"
        else:
            return False, f"mdat at index {mdat_idx} precedes moov at index {moov_idx}"

    return False, f"Missing moov or mdat atoms in top-level stream: {atoms}"


def verify_rendered_export(
    file_path: str,
    expected_duration_s: float,
    ffprobe_exe: str,
    expected_width: int = 1080,
    expected_height: int = 1920,
    expected_fps: float = 30.0,
    duration_tolerance_s: float = 0.5
) -> Dict[str, Any]:
    """
    Runs complete verification checklist on exported MP4 file.
    Returns structured results for checklist rule validation.
    """
    report: Dict[str, Any] = {
        "file_exists": False,
        "overall_pass": False,
        "checks": {},
        "errors": []
    }

    if not os.path.exists(file_path) or os.path.getsize(file_path) == 0:
        report["errors"].append("Exported file does not exist or is 0 bytes")
        return report

    report["file_exists"] = True
    report["file_size_bytes"] = os.path.getsize(file_path)
    report["file_size_mb"] = round(report["file_size_bytes"] / (1024.0 * 1024.0), 2)

    # 1. Faststart atom order check
    faststart_ok, faststart_reason = check_faststart_moov_at_start(file_path)
    report["checks"]["faststart_moov_first"] = {
        "pass": faststart_ok,
        "detail": faststart_reason
    }
    if not faststart_ok:
        report["errors"].append(f"Faststart check failed: {faststart_reason}")

    # 2. ffprobe metadata probe
    cmd = [
        ffprobe_exe, "-v", "quiet", "-print_format", "json",
        "-show_streams", "-show_format", file_path
    ]
    res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if res.returncode != 0:
        report["errors"].append(f"ffprobe execution failed: {res.stderr}")
        return report

    meta = json.loads(res.stdout)
    v_streams = [s for s in meta.get("streams", []) if s.get("codec_type") == "video"]
    a_streams = [s for s in meta.get("streams", []) if s.get("codec_type") == "audio"]

    if not v_streams:
        report["errors"].append("Missing video stream in exported file")
        return report
    if not a_streams:
        report["errors"].append("Missing audio stream in exported file")
        return report

    v_stream = v_streams[0]
    a_stream = a_streams[0]

    # Video codec check
    v_codec = v_stream.get("codec_name")
    v_codec_ok = (v_codec == "h264")
    report["checks"]["video_codec"] = {"pass": v_codec_ok, "codec": v_codec}
    if not v_codec_ok:
        report["errors"].append(f"Expected H.264 video codec, found {v_codec}")

    # Resolution check
    w = v_stream.get("width")
    h = v_stream.get("height")
    res_ok = (w == expected_width and h == expected_height)
    report["checks"]["resolution"] = {"pass": res_ok, "width": w, "height": h}
    if not res_ok:
        report["errors"].append(f"Expected resolution {expected_width}x{expected_height}, found {w}x{h}")

    # Framerate check
    fps_raw = v_stream.get("r_frame_rate", "0/1")
    try:
        num, den = fps_raw.split("/")
        actual_fps = float(num) / float(den)
    except Exception:
        actual_fps = 0.0
    fps_ok = abs(actual_fps - expected_fps) <= 1.0
    report["checks"]["framerate"] = {"pass": fps_ok, "fps": round(actual_fps, 2)}
    if not fps_ok:
        report["errors"].append(f"Expected ~{expected_fps} fps, found {actual_fps}")

    # Duration check
    fmt_dur = float(meta.get("format", {}).get("duration", 0.0))
    dur_ok = abs(fmt_dur - expected_duration_s) <= duration_tolerance_s
    report["checks"]["duration"] = {"pass": dur_ok, "duration_s": round(fmt_dur, 2)}
    if not dur_ok:
        report["errors"].append(f"Duration {fmt_dur:.2f}s is out of tolerance (expected {expected_duration_s:.2f}s ± {duration_tolerance_s}s)")

    # Audio check
    a_codec = a_stream.get("codec_name")
    a_sr = int(a_stream.get("sample_rate", 0))
    a_codec_ok = (a_codec == "aac" and a_sr in (44100, 48000))
    report["checks"]["audio"] = {"pass": a_codec_ok, "codec": a_codec, "sample_rate": a_sr}
    if not a_codec_ok:
        report["errors"].append(f"Audio check failed (expected aac @ 44.1/48kHz, got {a_codec} @ {a_sr}Hz)")

    # Color tags check
    color_space = v_stream.get("color_space")
    color_range = v_stream.get("color_range")
    color_ok = (color_space == "bt709" and color_range == "tv")
    report["checks"]["color_tags"] = {"pass": color_ok, "color_space": color_space, "color_range": color_range}

    report["overall_pass"] = (len(report["errors"]) == 0)
    return report
