#!/usr/bin/env python3
"""
Spike S1 — test-approach-b.py
Approach B: yt-dlp resolves URLs (-J), our FFmpeg downloads the range.
Uses -progress pipe:1 for precise out_time progress.
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

# Find FFmpeg
FFMPEG = None
FFPROBE = None
for root, dirs, files in os.walk(BIN_DIR):
    if "ffmpeg.exe" in files:
        FFMPEG = os.path.join(root, "ffmpeg.exe")
        FFPROBE = os.path.join(root, "ffprobe.exe")
        break

RANGES = [
    ("1min", 60, 120),
    ("5min", 60, 360),
    ("10min", 60, 660),
]

FORMAT_SELECTOR = "bv*[vcodec^=avc1][height<=1080]+ba[acodec^=mp4a]/b[vcodec^=avc1][height<=1080]/bv*[height<=1080]+ba/b[height<=1080]"


def resolve_urls(video_url: str) -> dict:
    """Use yt-dlp -J to resolve direct URLs and metadata."""
    cmd = [
        YTDLP,
        "-J", "--no-playlist", "--no-warnings", "--ignore-config",
        "--js-runtimes", f"deno:{DENO}",
        "-f", FORMAT_SELECTOR,
        video_url,
    ]
    print(f"  Resolving: {video_url}")
    t0 = time.monotonic()
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
    resolve_time = time.monotonic() - t0
    print(f"  Resolve time: {resolve_time:.1f}s, exit={r.returncode}")

    if r.returncode != 0:
        print(f"  ERROR: {r.stderr[:500]}")
        return {"error": r.stderr[:500], "resolve_time_s": round(resolve_time, 2)}

    info = json.loads(r.stdout)
    result = {
        "resolve_time_s": round(resolve_time, 2),
        "title": info.get("title", "?"),
        "duration": info.get("duration"),
        "formats_count": len(info.get("formats", [])),
    }

    # Find the requested format's URL(s)
    rf = info.get("requested_formats") or []
    if rf:
        # Separate video and audio
        for f in rf:
            if f.get("vcodec", "none") != "none":
                result["video_url"] = f.get("url")
                result["video_headers"] = f.get("http_headers", {})
                result["video_format"] = f"{f.get('vcodec')} {f.get('width')}x{f.get('height')} {f.get('tbr', '?')}kbps"
            if f.get("acodec", "none") != "none":
                result["audio_url"] = f.get("url")
                result["audio_headers"] = f.get("http_headers", {})
                result["audio_format"] = f"{f.get('acodec')} {f.get('abr', '?')}kbps"
    elif info.get("url"):
        # Single format with both
        result["video_url"] = info["url"]
        result["audio_url"] = info["url"]
        result["video_headers"] = info.get("http_headers", {})
        result["audio_headers"] = info.get("http_headers", {})
        result["video_format"] = f"{info.get('vcodec')} {info.get('width')}x{info.get('height')}"
        result["audio_format"] = f"{info.get('acodec')}"

    return result


def probe_file(filepath: str) -> dict:
    r = subprocess.run(
        [FFPROBE, "-v", "quiet", "-print_format", "json",
         "-show_format", "-show_streams", filepath],
        capture_output=True, text=True, timeout=30
    )
    if r.returncode != 0:
        return {"error": r.stderr[:500]}
    return json.loads(r.stdout)


def download_with_ffmpeg(video_url: str, audio_url: str,
                         video_headers: dict, audio_headers: dict,
                         start_s: int, end_s: int, label: str) -> dict:
    """
    Download range using FFmpeg with -progress pipe:1.
    Uses -ss before -i for fast seek, -t for duration.
    """
    os.makedirs(RESULTS_DIR, exist_ok=True)
    output_file = os.path.join(RESULTS_DIR, f"b_{label}.mp4")
    if os.path.exists(output_file):
        os.remove(output_file)

    expected_dur = end_s - start_s

    # Build FFmpeg command
    cmd = [FFMPEG, "-y"]

    # Video input with seek and headers immediately before -i
    cmd.extend(["-ss", str(start_s), "-t", str(expected_dur)])
    v_hdr_str = "".join(f"{k}: {v}\r\n" for k, v in video_headers.items()) if video_headers else ""
    if v_hdr_str:
        cmd.extend(["-headers", v_hdr_str])
    cmd.extend(["-i", video_url])

    if audio_url and audio_url != video_url:
        cmd.extend(["-ss", str(start_s), "-t", str(expected_dur)])
        a_hdr_str = "".join(f"{k}: {v}\r\n" for k, v in audio_headers.items()) if audio_headers else ""
        if a_hdr_str:
            cmd.extend(["-headers", a_hdr_str])
        cmd.extend(["-i", audio_url])
        cmd.extend(["-map", "0:v:0", "-map", "1:a:0"])

    cmd.extend([
        "-c", "copy",
        "-movflags", "+faststart",
        "-progress", "pipe:1",
        output_file,
    ])

    print(f"\n--- Approach B: {label} ---")
    print(f"  Expected duration: {expected_dur}s")

    progress_events = []
    t0 = time.monotonic()

    stderr_log_file = os.path.join(RESULTS_DIR, f"b_{label}_ffmpeg_stderr.log")
    with open(stderr_log_file, "w", encoding="utf-8") as stderr_f:
        proc = subprocess.Popen(
            cmd, stdout=subprocess.PIPE, stderr=stderr_f,
            text=True, bufsize=1
        )

        # Read -progress pipe:1 output from stdout
        # Format: key=value lines, with "progress=continue" or "progress=end" as separators
        current_block = {}
        for line in proc.stdout:
            line = line.strip()
            ts = time.monotonic() - t0

            if "=" in line:
                key, _, val = line.partition("=")
                current_block[key] = val

            if line.startswith("progress="):
                # End of a progress block
                event = {"ts": round(ts, 2)}

                # Parse out_time (HH:MM:SS.ffffff)
                out_time = current_block.get("out_time_us") or current_block.get("out_time")
                if out_time and out_time != "N/A":
                    try:
                        if ":" in str(out_time):
                            # HH:MM:SS.ffffff format
                            parts = out_time.split(":")
                            out_seconds = int(parts[0]) * 3600 + int(parts[1]) * 60 + float(parts[2])
                        else:
                            out_seconds = int(out_time) / 1_000_000  # microseconds
                        event["out_time_s"] = round(out_seconds, 2)
                        event["percent"] = round(min(out_seconds / expected_dur * 100, 100), 1)
                    except (ValueError, IndexError):
                        pass

                speed = current_block.get("speed")
                if speed and speed != "N/A":
                    event["speed"] = speed

                total_size = current_block.get("total_size")
                if total_size and total_size != "N/A":
                    event["total_size"] = int(total_size)

                progress_events.append(event)
                pct = event.get("percent", "?")
                spd = event.get("speed", "?")
                print(f"  [{ts:6.1f}s] out_time={event.get('out_time_s', '?')}s pct={pct}% speed={spd}")

                current_block = {}

        proc.wait()
    wall_time = time.monotonic() - t0

    stderr_out = ""
    if os.path.exists(stderr_log_file):
        with open(stderr_log_file, "r", encoding="utf-8", errors="replace") as f:
            stderr_out = f.read()

    result = {
        "approach": "B",
        "label": label,
        "exit_code": proc.returncode,
        "wall_time_s": round(wall_time, 2),
        "progress_event_count": len(progress_events),
        "has_percent_progress": any(e.get("percent") for e in progress_events),
    }

    if proc.returncode != 0:
        result["error"] = stderr_out[:1000]
        # Check for throttling indicators
        if "403" in stderr_out or "Forbidden" in stderr_out:
            result["throttled"] = True
        if "Server returned 5" in stderr_out:
            result["server_error"] = True

    if os.path.exists(output_file):
        file_size = os.path.getsize(output_file)
        result["file_size_bytes"] = file_size
        result["file_size_mb"] = round(file_size / 1e6, 2)
        result["avg_speed_mbps"] = round(file_size / wall_time / 1e6, 2) if wall_time > 0 else 0
        result["progress_rate_per_s"] = round(len(progress_events) / wall_time, 2) if wall_time > 0 else 0

        probe = probe_file(output_file)
        if "error" not in probe:
            streams = probe.get("streams", [])
            vstream = next((s for s in streams if s["codec_type"] == "video"), None)
            astream = next((s for s in streams if s["codec_type"] == "audio"), None)
            result["video_codec"] = vstream.get("codec_name", "?") if vstream else "none"
            result["audio_codec"] = astream.get("codec_name", "?") if astream else "none"
            result["resolution"] = f"{vstream.get('width')}x{vstream.get('height')}" if vstream else "?"
            result["duration_s"] = float(probe.get("format", {}).get("duration", 0))
            result["duration_error_s"] = round(result["duration_s"] - expected_dur, 2)
            result["playable_in_browser"] = (
                result["video_codec"] in ("h264",) and
                result["audio_codec"] in ("aac",)
            )
    else:
        result["error"] = result.get("error", "") + " | no output file"

    # Save progress trace
    trace_file = os.path.join(RESULTS_DIR, f"b_{label}_trace.json")
    with open(trace_file, "w") as f:
        json.dump(progress_events, f, indent=2)

    print(f"\n  Result: exit={result['exit_code']} wall={result['wall_time_s']}s "
          f"size={result.get('file_size_mb', '?')}MB codec={result.get('video_codec', '?')}+{result.get('audio_codec', '?')} "
          f"playable={result.get('playable_in_browser', '?')} progress_events={len(progress_events)}")

    return result


def main():
    if not FFMPEG:
        print("ERROR: FFmpeg not found. Run fetch-binaries.py first.")
        sys.exit(1)

    video_url = "https://www.youtube.com/watch?v=QOJIILaJKNc"

    # Step 1: Resolve URLs
    print("="*60)
    print("PHASE 1: Resolve URLs with yt-dlp -J")
    print("="*60)
    resolved = resolve_urls(video_url)

    if "error" in resolved:
        print(f"FATAL: Could not resolve URLs: {resolved['error']}")
        sys.exit(1)

    print(f"  Title: {resolved.get('title')}")
    print(f"  Duration: {resolved.get('duration')}s")
    print(f"  Video: {resolved.get('video_format')}")
    print(f"  Audio: {resolved.get('audio_format')}")

    video_direct = resolved.get("video_url")
    audio_direct = resolved.get("audio_url")
    video_headers = resolved.get("video_headers", {})
    audio_headers = resolved.get("audio_headers", {})
    if not video_direct:
        print("FATAL: No video URL resolved.")
        sys.exit(1)

    # Save resolve result (without URLs which are sensitive/ephemeral)
    resolve_safe = {k: v for k, v in resolved.items() if "url" not in k.lower()}
    with open(os.path.join(RESULTS_DIR, "resolve_result.json"), "w") as f:
        json.dump(resolve_safe, f, indent=2)

    # Step 2: Download each range
    print("\n" + "="*60)
    print("PHASE 2: Download ranges with FFmpeg")
    print("="*60)

    all_results = []
    for range_label, start_s, end_s in RANGES:
        result = download_with_ffmpeg(
            video_direct, audio_direct,
            video_headers, audio_headers,
            start_s, end_s, range_label
        )
        result["resolve_time_s"] = resolved["resolve_time_s"]
        all_results.append(result)

    # Save all results
    with open(os.path.join(RESULTS_DIR, "approach_b_results.json"), "w") as f:
        json.dump(all_results, f, indent=2)

    # Print summary
    print(f"\n{'='*80}")
    print("APPROACH B SUMMARY")
    print(f"{'='*80}")
    print(f"Resolve time: {resolved['resolve_time_s']}s")
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
