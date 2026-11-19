#!/usr/bin/env python3
"""
Spike S1b — test-lgpl-ffmpeg.py
Item 3: Repeat 1-minute copy-mode test with real LGPL-only FFmpeg build.
Also test what happens when --force-keyframes-at-cuts is used with it.
"""
import json
import os
import subprocess
import time

SPIKE_DIR = os.path.dirname(os.path.abspath(__file__))
BIN_DIR = os.path.join(SPIKE_DIR, "bin")
LGPL_FFMPEG_DIR = os.path.join(BIN_DIR, "ffmpeg-lgpl", "bin")
RESULTS_DIR = os.path.join(SPIKE_DIR, "results", "s1b")
os.makedirs(RESULTS_DIR, exist_ok=True)

YTDLP = os.path.join(BIN_DIR, "yt-dlp.exe")
QJS = os.path.join(BIN_DIR, "qjs.exe")

VIDEO_URL = "https://www.youtube.com/watch?v=QOJIILaJKNc"
FORMAT_SELECTOR = "bv*[vcodec^=avc1][height<=1080]+ba[acodec^=mp4a]/b[vcodec^=avc1][height<=1080]/bv*[height<=1080]+ba/b[height<=1080]"

def test_lgpl():
    print("=" * 70)
    print("Spike S1b: LGPL-only FFmpeg Build Tests")
    print("=" * 70)

    ffmpeg_exe = os.path.join(LGPL_FFMPEG_DIR, "ffmpeg.exe")
    ffprobe_exe = os.path.join(LGPL_FFMPEG_DIR, "ffprobe.exe")

    ver_res = subprocess.run([ffmpeg_exe, "-version"], capture_output=True, text=True)
    ver_line = ver_res.stdout.splitlines()[0]
    has_gpl = "--enable-gpl" in ver_res.stdout
    enc_res = subprocess.run([ffmpeg_exe, "-encoders"], capture_output=True, text=True)
    has_libx264 = "libx264" in enc_res.stdout
    has_h264_mf = "h264_mf" in enc_res.stdout

    print(f"Build: {ver_line}")
    print(f"Has GPL: {has_gpl}")
    print(f"Has libx264: {has_libx264}")
    print(f"Has h264_mf: {has_h264_mf}")

    results = {
        "build_version": ver_line,
        "has_gpl": has_gpl,
        "has_libx264": has_libx264,
        "has_h264_mf": has_h264_mf,
    }

    # Test A: Copy-mode download with LGPL FFmpeg
    print("\n--- Test A: 1-min Range in COPY MODE with LGPL FFmpeg ---")
    out_copy = os.path.join(RESULTS_DIR, "lgpl_copy_1min.mp4")
    if os.path.exists(out_copy):
        os.remove(out_copy)

    cmd_copy = [
        YTDLP,
        "--no-playlist", "--no-warnings", "--newline", "--ignore-config",
        "--ffmpeg-location", LGPL_FFMPEG_DIR,
        "--js-runtimes", f"quickjs:{QJS}",
        "-f", FORMAT_SELECTOR,
        "--download-sections", "*00:01:00-00:02:00",
        "-o", out_copy,
        VIDEO_URL,
    ]
    t0 = time.monotonic()
    p_copy = subprocess.run(cmd_copy, capture_output=True, text=True)
    wall_copy = round(time.monotonic() - t0, 2)
    print(f"Copy-mode exit={p_copy.returncode} in {wall_copy}s")

    if os.path.exists(out_copy):
        size = os.path.getsize(out_copy)
        probe_res = subprocess.run([ffprobe_exe, "-v", "quiet", "-show_format", "-show_streams", "-print_format", "json", out_copy], capture_output=True, text=True)
        probe_info = json.loads(probe_res.stdout) if probe_res.returncode == 0 else {}
        v_stream = next((s for s in probe_info.get("streams", []) if s["codec_type"] == "video"), {})
        a_stream = next((s for s in probe_info.get("streams", []) if s["codec_type"] == "audio"), {})
        results["copy_mode"] = {
            "exit_code": p_copy.returncode,
            "wall_time_s": wall_copy,
            "size_mb": round(size / 1e6, 2),
            "video_codec": v_stream.get("codec_name"),
            "audio_codec": a_stream.get("codec_name"),
            "duration": probe_info.get("format", {}).get("duration"),
        }
        print(f"Copy-mode output: {results['copy_mode']['size_mb']}MB, {results['copy_mode']['video_codec']}+{results['copy_mode']['audio_codec']}")
    else:
        results["copy_mode"] = {"exit_code": p_copy.returncode, "error": p_copy.stderr[-300:]}

    # Test B: With --force-keyframes-at-cuts using LGPL FFmpeg (which lacks libx264)
    print("\n--- Test B: With --force-keyframes-at-cuts on LGPL FFmpeg (NO libx264) ---")
    out_force = os.path.join(RESULTS_DIR, "lgpl_force_keyframes_1min.mp4")
    if os.path.exists(out_force):
        os.remove(out_force)

    cmd_force = [
        YTDLP,
        "--no-playlist", "--no-warnings", "--newline", "--ignore-config",
        "--ffmpeg-location", LGPL_FFMPEG_DIR,
        "--js-runtimes", f"quickjs:{QJS}",
        "-f", FORMAT_SELECTOR,
        "--download-sections", "*00:01:00-00:02:00",
        "--force-keyframes-at-cuts",
        "-o", out_force,
        VIDEO_URL,
    ]
    t0 = time.monotonic()
    p_force = subprocess.run(cmd_force, capture_output=True, text=True)
    wall_force = round(time.monotonic() - t0, 2)
    print(f"Force-keyframes exit={p_force.returncode} in {wall_force}s")

    results["force_keyframes"] = {
        "exit_code": p_force.returncode,
        "wall_time_s": wall_force,
        "stdout_tail": p_force.stdout[-400:] if p_force.stdout else "",
        "stderr_tail": p_force.stderr[-400:] if p_force.stderr else "",
    }
    if os.path.exists(out_force):
        results["force_keyframes"]["output_exists"] = True
        results["force_keyframes"]["size_mb"] = round(os.path.getsize(out_force) / 1e6, 2)
        print(f"Force-keyframes output exists: {results['force_keyframes']['size_mb']}MB")
    else:
        results["force_keyframes"]["output_exists"] = False
        print("Force-keyframes failed to produce output. Error details:")
        for line in (p_force.stdout + "\n" + p_force.stderr).splitlines():
            if "error" in line.lower() or "unknown encoder" in line.lower():
                print(f"  {line}")

    with open(os.path.join(RESULTS_DIR, "lgpl_ffmpeg_results.json"), "w") as f:
        json.dump(results, f, indent=2)

if __name__ == "__main__":
    test_lgpl()
