#!/usr/bin/env python3
"""
Spike S1b — test-copy-mode.py
Item 1: Copy-mode benchmark (WITHOUT --force-keyframes-at-cuts).
Item 2: Deep offsets benchmark (30m, 1.5h, 3h).
Measures:
- Wall time, file size, bitrate vs source bitrate
- Progress emission (rate, format)
- Keyframe alignment (PTS offset of first video packet)
- Browser playability check
"""
import json
import os
import re
import subprocess
import sys
import time

SPIKE_DIR = os.path.dirname(os.path.abspath(__file__))
BIN_DIR = os.path.join(SPIKE_DIR, "bin")
RESULTS_DIR = os.path.join(SPIKE_DIR, "results", "s1b")
os.makedirs(RESULTS_DIR, exist_ok=True)

YTDLP = os.path.join(BIN_DIR, "yt-dlp.exe")
QJS = os.path.join(BIN_DIR, "qjs.exe")

# Locate FFmpeg/ffprobe
FFMPEG_DIR = None
for root, dirs, files in os.walk(BIN_DIR):
    if "ffmpeg.exe" in files and "ffmpeg-lgpl" not in root:
        FFMPEG_DIR = root
        break

FFPROBE = os.path.join(FFMPEG_DIR, "ffprobe.exe") if FFMPEG_DIR else None

VIDEO_URL = "https://www.youtube.com/watch?v=QOJIILaJKNc"
FORMAT_SELECTOR = "bv*[vcodec^=avc1][height<=1080]+ba[acodec^=mp4a]/b[vcodec^=avc1][height<=1080]/bv*[height<=1080]+ba/b[height<=1080]"

def format_time(seconds: int) -> str:
    h, r = divmod(seconds, 3600)
    m, s = divmod(r, 60)
    return f"{h:02d}:{m:02d}:{s:02d}"

def probe_file(filepath: str, requested_start_s: float) -> dict:
    cmd = [
        FFPROBE, "-v", "quiet", "-print_format", "json",
        "-show_format", "-show_streams", filepath
    ]
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
    if r.returncode != 0:
        return {"error": r.stderr}
    info = json.loads(r.stdout)
    
    # Query first packets for video to check keyframe PTS offset
    pkt_cmd = [
        FFPROBE, "-v", "quiet", "-print_format", "json",
        "-select_streams", "v:0",
        "-show_packets", "-read_intervals", "%+#5",
        filepath
    ]
    pkt_r = subprocess.run(pkt_cmd, capture_output=True, text=True, timeout=30)
    first_pts = None
    first_flags = None
    if pkt_r.returncode == 0:
        pkts = json.loads(pkt_r.stdout).get("packets", [])
        if pkts:
            first_pts = float(pkts[0].get("pts_time", 0))
            first_flags = pkts[0].get("flags", "")

    streams = info.get("streams", [])
    vstream = next((s for s in streams if s["codec_type"] == "video"), {})
    astream = next((s for s in streams if s["codec_type"] == "audio"), {})
    fmt = info.get("format", {})

    v_start_time = float(vstream.get("start_time", 0.0))
    duration = float(fmt.get("duration", 0.0))
    size_bytes = int(fmt.get("size", 0))
    bit_rate = int(fmt.get("bit_rate", 0))

    return {
        "vcodec": vstream.get("codec_name"),
        "acodec": astream.get("codec_name"),
        "resolution": f"{vstream.get('width')}x{vstream.get('height')}",
        "fps": vstream.get("r_frame_rate"),
        "duration_s": duration,
        "size_bytes": size_bytes,
        "bit_rate_kbps": round(bit_rate / 1000, 1),
        "v_start_time": v_start_time,
        "first_packet_pts": first_pts,
        "first_packet_flags": first_flags,
        "browser_playable": vstream.get("codec_name") == "h264" and astream.get("codec_name") == "aac",
    }

def run_copy_download(video_url: str, start_s: int, end_s: int, label: str, ffmpeg_path: str = None) -> dict:
    ff_dir = ffmpeg_path or FFMPEG_DIR
    out_file = os.path.join(RESULTS_DIR, f"{label}.mp4")
    for f in [out_file, out_file + ".part"]:
        if os.path.exists(f):
            os.remove(f)

    section_spec = f"*{format_time(start_s)}-{format_time(end_s)}"
    cmd = [
        YTDLP,
        "--no-playlist", "--no-warnings", "--newline", "--ignore-config",
        "--ffmpeg-location", ff_dir,
        "--js-runtimes", f"quickjs:{QJS}",
        "-f", FORMAT_SELECTOR,
        "--download-sections", section_spec,
        # NO --force-keyframes-at-cuts !
        "-o", out_file,
        video_url,
    ]

    print(f"\n[{label}] Downloading range {section_spec} (COPY MODE)...")
    t0 = time.monotonic()
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1)

    progress_lines = []
    has_ffmpeg_time = False
    has_download_pct = False

    for line in proc.stdout:
        line = line.strip()
        ts = round(time.monotonic() - t0, 2)
        if "time=" in line and "speed=" in line:
            has_ffmpeg_time = True
            progress_lines.append((ts, line))
        elif "[download]" in line and "%" in line:
            has_download_pct = True
            progress_lines.append((ts, line))
        elif "error" in line.lower():
            print(f"  [{ts}s] ERR: {line[:120]}")

    proc.wait()
    wall_time = round(time.monotonic() - t0, 2)

    actual_file = out_file if os.path.exists(out_file) else None
    if not actual_file:
        for f in os.listdir(RESULTS_DIR):
            if f.startswith(label) and not f.endswith(".part"):
                actual_file = os.path.join(RESULTS_DIR, f)
                break

    res = {
        "label": label,
        "start_s": start_s,
        "end_s": end_s,
        "expected_duration": end_s - start_s,
        "exit_code": proc.returncode,
        "wall_time_s": wall_time,
        "progress_events_count": len(progress_lines),
        "events_per_sec": round(len(progress_lines) / wall_time, 2) if wall_time > 0 else 0,
        "has_ffmpeg_time": has_ffmpeg_time,
        "has_download_pct": has_download_pct,
    }

    if actual_file and os.path.exists(actual_file):
        probe = probe_file(actual_file, start_s)
        res["output_file"] = actual_file
        res["size_bytes"] = probe.get("size_bytes")
        res["size_mb"] = round(probe.get("size_bytes", 0) / 1e6, 2)
        res["bit_rate_kbps"] = probe.get("bit_rate_kbps")
        res["duration_s"] = probe.get("duration_s")
        res["duration_error_s"] = round(probe.get("duration_s", 0) - (end_s - start_s), 2)
        res["v_start_time"] = probe.get("v_start_time")
        res["first_packet_pts"] = probe.get("first_packet_pts")
        res["first_packet_flags"] = probe.get("first_packet_flags")
        res["codec"] = f"{probe.get('vcodec')}+{probe.get('acodec')}"
        res["browser_playable"] = probe.get("browser_playable")
        print(f"  -> Exit: {proc.returncode}, Wall: {wall_time}s, Size: {res['size_mb']}MB, Codec: {res['codec']}, First PTS: {res['first_packet_pts']}, Evts/s: {res['events_per_sec']}")
    else:
        res["error"] = "Output file not found"
        print(f"  -> FAILED: Exit {proc.returncode}")

    return res

def main():
    print("=" * 70)
    print("Spike S1b: Copy-Mode & Deep Offset Benchmarks")
    print("=" * 70)

    # 1. Fetch Source Stream Info for baseline comparison
    print("Probing source format info with yt-dlp -J...")
    r = subprocess.run([
        YTDLP, "-J", "--no-playlist", "--no-warnings", "--ignore-config",
        "--js-runtimes", f"quickjs:{QJS}",
        "-f", FORMAT_SELECTOR,
        VIDEO_URL
    ], capture_output=True, text=True, timeout=60)
    source_info = {}
    if r.returncode == 0:
        info = json.loads(r.stdout)
        rf = info.get("requested_formats") or []
        v_f = next((f for f in rf if f.get("vcodec") != "none"), {})
        a_f = next((f for f in rf if f.get("acodec") != "none"), {})
        source_info = {
            "title": info.get("title"),
            "duration": info.get("duration"),
            "v_tbr": v_f.get("tbr"),
            "a_abr": a_f.get("abr"),
            "total_tbr_kbps": (v_f.get("tbr") or 0) + (a_f.get("abr") or 0),
            "v_codec": v_f.get("vcodec"),
            "a_codec": a_f.get("acodec"),
        }
        print(f"Source stream: {source_info['v_codec']} @ {source_info['v_tbr']}kbps + {source_info['a_codec']} @ {source_info['a_abr']}kbps (~{source_info['total_tbr_kbps']} kbps total)")

    results = {"source_info": source_info, "copy_mode": [], "deep_offsets": []}

    # Item 1: Copy-Mode Ranges (1min, 5min, 10min)
    # We do 3 runs for 1min to get variance, and 1 solid run for 5min and 10min
    copy_tests = [
        ("copy_1min_run1", 60, 120),
        ("copy_1min_run2", 60, 120),
        ("copy_1min_run3", 60, 120),
        ("copy_5min", 60, 360),
        ("copy_10min", 60, 660),
    ]

    for label, s, e in copy_tests:
        res = run_copy_download(VIDEO_URL, s, e, label)
        results["copy_mode"].append(res)

    # Item 2: Deep Offsets (1min starting at 30min, 1.5h, 3h)
    deep_tests = [
        ("offset_30min", 1800, 1860),     # 30m
        ("offset_90min", 5400, 5460),     # 1h 30m
        ("offset_180min", 10800, 10860),  # 3h
    ]

    for label, s, e in deep_tests:
        res = run_copy_download(VIDEO_URL, s, e, label)
        results["deep_offsets"].append(res)

    # Save results
    out_json = os.path.join(RESULTS_DIR, "copy_mode_and_deep_offsets.json")
    with open(out_json, "w") as f:
        json.dump(results, f, indent=2)
    print(f"\nAll results saved to {out_json}")

if __name__ == "__main__":
    main()
