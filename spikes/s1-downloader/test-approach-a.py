#!/usr/bin/env python3
"""
Spike S1 — test-approach-a.py
Approach A: yt-dlp --download-sections downloads the range directly.
Captures and parses progress output. Measures wall time, speed, file size.
"""
import json
import os
import re
import subprocess
import sys
import time

SPIKE_DIR = os.path.dirname(os.path.abspath(__file__))
BIN_DIR = os.path.join(SPIKE_DIR, "bin")
RESULTS_DIR = os.path.join(SPIKE_DIR, "results")

YTDLP = os.path.join(BIN_DIR, "yt-dlp.exe")
DENO = os.path.join(BIN_DIR, "deno.exe")
QJS = None  # set after finding it

# Find FFmpeg dir
FFMPEG_DIR = None
for root, dirs, files in os.walk(BIN_DIR):
    if "ffmpeg.exe" in files:
        FFMPEG_DIR = root
        break

# Ranges to test: (label, start_seconds, end_seconds)
RANGES = [
    ("1min", 60, 120),
    ("5min", 60, 360),
    ("10min", 60, 660),
]

# Format selector: prefer avc1 + m4a ≤1080p
FORMAT_SELECTOR = "bv*[vcodec^=avc1][height<=1080]+ba[acodec^=mp4a]/b[vcodec^=avc1][height<=1080]/bv*[height<=1080]+ba/b[height<=1080]"


def format_time(seconds: int) -> str:
    """Convert seconds to HH:MM:SS"""
    h, r = divmod(seconds, 3600)
    m, s = divmod(r, 60)
    return f"{h:02d}:{m:02d}:{s:02d}"


def probe_file(filepath: str) -> dict:
    """Use ffprobe to check output file."""
    ffprobe = os.path.join(FFMPEG_DIR, "ffprobe.exe")
    r = subprocess.run(
        [ffprobe, "-v", "quiet", "-print_format", "json",
         "-show_format", "-show_streams", filepath],
        capture_output=True, text=True, timeout=30
    )
    if r.returncode != 0:
        return {"error": r.stderr[:500]}
    return json.loads(r.stdout)


def run_approach_a(video_url: str, start_s: int, end_s: int, label: str,
                   runtime: str = "deno", runtime_path: str = None) -> dict:
    """
    Download a range using yt-dlp --download-sections.
    Returns measurement dict.
    """
    os.makedirs(RESULTS_DIR, exist_ok=True)
    output_file = os.path.join(RESULTS_DIR, f"a_{label}_{runtime}.mp4")
    # Clean up previous
    for f in [output_file, output_file.replace(".mp4", ".mkv"), output_file.replace(".mp4", ".webm")]:
        if os.path.exists(f):
            os.remove(f)

    section_spec = f"*{format_time(start_s)}-{format_time(end_s)}"

    cmd = [
        YTDLP,
        "--no-playlist", "--no-warnings", "--newline", "--ignore-config",
        "--ffmpeg-location", FFMPEG_DIR,
        "-f", FORMAT_SELECTOR,
        "--download-sections", section_spec,
        "-o", output_file,
        "--force-keyframes-at-cuts",
        video_url,
    ]

    # JS runtime
    if runtime_path:
        cmd.extend(["--js-runtimes", f"{runtime}:{runtime_path}"])
    elif runtime == "deno":
        cmd.extend(["--js-runtimes", f"deno:{DENO}"])

    print(f"\n--- Approach A: {label} runtime={runtime} ---")
    print(f"Command: {' '.join(cmd[:6])}...")
    print(f"Section: {section_spec}")

    progress_events = []
    stderr_lines = []
    t0 = time.monotonic()

    proc = subprocess.Popen(
        cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        text=True, bufsize=1
    )

    for line in proc.stdout:
        line = line.rstrip()
        ts = time.monotonic() - t0
        # Parse yt-dlp download progress lines
        # Format: [download]  42.0% of ~118.00MiB at  6.20MiB/s ETA 00:12
        # Also:   [download]  42.0% of  118.00MiB at  6.20MiB/s ETA 00:12 (frag 5/12)
        m_pct = re.search(r'\[download\]\s+([\d.]+)%\s+of\s+~?([\d.]+)(MiB|KiB|GiB|B)', line)
        m_speed = re.search(r'at\s+([\d.]+)(MiB|KiB|GiB|B)/s', line)
        m_eta = re.search(r'ETA\s+(\d+:\d+(?::\d+)?)', line)
        # FFmpeg progress (when yt-dlp shells out to ffmpeg for section cutting)
        m_ffmpeg_time = re.search(r'time=(\d+:\d+:\d+\.\d+)', line)
        m_ffmpeg_speed = re.search(r'speed=([\d.]+)x', line)

        event = {"ts": round(ts, 2), "raw": line[:200]}
        if m_pct:
            event["percent"] = float(m_pct.group(1))
            event["total_str"] = m_pct.group(2) + m_pct.group(3)
        if m_speed:
            event["speed_str"] = m_speed.group(1) + m_speed.group(2) + "/s"
        if m_eta:
            event["eta_str"] = m_eta.group(1)
        if m_ffmpeg_time:
            event["ffmpeg_time"] = m_ffmpeg_time.group(1)
        if m_ffmpeg_speed:
            event["ffmpeg_speed"] = m_ffmpeg_speed.group(1) + "x"

        progress_events.append(event)
        # Print key progress
        if m_pct or m_ffmpeg_time:
            pct = event.get("percent", "")
            ffts = event.get("ffmpeg_time", "")
            spd = event.get("speed_str", event.get("ffmpeg_speed", ""))
            print(f"  [{ts:6.1f}s] pct={pct} ffmpeg_time={ffts} speed={spd}")
        elif "[error]" in line.lower() or "error" in line.lower():
            print(f"  [{ts:6.1f}s] {line[:150]}")

    proc.wait()
    wall_time = time.monotonic() - t0

    # Find the actual output file (might have different extension)
    actual_file = None
    for ext in [".mp4", ".mkv", ".webm", ".m4a"]:
        candidate = output_file.replace(".mp4", ext) if ext != ".mp4" else output_file
        if os.path.exists(candidate):
            actual_file = candidate
            break
    # Also check without extension replacement (yt-dlp might add its own)
    if not actual_file:
        base = output_file.rsplit(".", 1)[0]
        for f in os.listdir(RESULTS_DIR):
            if f.startswith(os.path.basename(base)):
                actual_file = os.path.join(RESULTS_DIR, f)
                break

    result = {
        "approach": "A",
        "label": label,
        "runtime": runtime,
        "exit_code": proc.returncode,
        "wall_time_s": round(wall_time, 2),
        "progress_event_count": len(progress_events),
        "has_percent_progress": any(e.get("percent") for e in progress_events),
        "has_ffmpeg_progress": any(e.get("ffmpeg_time") for e in progress_events),
        "output_file": actual_file,
    }

    if actual_file and os.path.exists(actual_file):
        file_size = os.path.getsize(actual_file)
        result["file_size_bytes"] = file_size
        result["file_size_mb"] = round(file_size / 1e6, 2)
        result["avg_speed_mbps"] = round(file_size / wall_time / 1e6, 2) if wall_time > 0 else 0
        result["progress_rate_per_s"] = round(len(progress_events) / wall_time, 2) if wall_time > 0 else 0

        # Probe output
        probe = probe_file(actual_file)
        if "error" not in probe:
            streams = probe.get("streams", [])
            vstream = next((s for s in streams if s["codec_type"] == "video"), None)
            astream = next((s for s in streams if s["codec_type"] == "audio"), None)
            result["video_codec"] = vstream.get("codec_name", "?") if vstream else "none"
            result["audio_codec"] = astream.get("codec_name", "?") if astream else "none"
            result["resolution"] = f"{vstream.get('width')}x{vstream.get('height')}" if vstream else "?"
            result["duration_s"] = float(probe.get("format", {}).get("duration", 0))
            expected_dur = end_s - start_s
            result["duration_error_s"] = round(result["duration_s"] - expected_dur, 2)
            result["playable_in_browser"] = (
                result["video_codec"] in ("h264",) and
                result["audio_codec"] in ("aac",)
            )
        else:
            result["probe_error"] = probe["error"]
    else:
        result["error"] = "no output file found"

    # Save progress trace
    trace_file = os.path.join(RESULTS_DIR, f"a_{label}_{runtime}_trace.json")
    with open(trace_file, "w") as f:
        json.dump(progress_events, f, indent=2)

    print(f"\n  Result: exit={result['exit_code']} wall={result['wall_time_s']}s "
          f"size={result.get('file_size_mb', '?')}MB codec={result.get('video_codec', '?')}+{result.get('audio_codec', '?')} "
          f"playable={result.get('playable_in_browser', '?')} progress_events={len(progress_events)}")

    return result


def main():
    if not os.path.exists(YTDLP):
        print(f"ERROR: yt-dlp not found at {YTDLP}. Run fetch-binaries.py first.")
        sys.exit(1)
    if not FFMPEG_DIR:
        print("ERROR: FFmpeg not found in bin/. Run fetch-binaries.py first.")
        sys.exit(1)

    # Test URL — the replay VOD
    video_url = "https://www.youtube.com/watch?v=QOJIILaJKNc"

    all_results = []

    for range_label, start_s, end_s in RANGES:
        result = run_approach_a(video_url, start_s, end_s, range_label)
        all_results.append(result)

    # Save all results
    with open(os.path.join(RESULTS_DIR, "approach_a_results.json"), "w") as f:
        json.dump(all_results, f, indent=2)

    # Print summary table
    print(f"\n{'='*80}")
    print("APPROACH A SUMMARY")
    print(f"{'='*80}")
    print(f"{'Range':<8} {'Wall(s)':>8} {'Size(MB)':>9} {'Speed(MB/s)':>11} {'Events':>7} {'Evt/s':>6} "
          f"{'Codec':>10} {'Playable':>8} {'Dur Err':>8}")
    for r in all_results:
        print(f"{r['label']:<8} {r.get('wall_time_s', '?'):>8} {r.get('file_size_mb', '?'):>9} "
              f"{r.get('avg_speed_mbps', '?'):>11} {r['progress_event_count']:>7} "
              f"{r.get('progress_rate_per_s', '?'):>6} "
              f"{r.get('video_codec', '?')+'+'+r.get('audio_codec', '?'):>10} "
              f"{str(r.get('playable_in_browser', '?')):>8} "
              f"{r.get('duration_error_s', '?'):>8}")


if __name__ == "__main__":
    main()
