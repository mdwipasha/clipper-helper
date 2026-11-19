#!/usr/bin/env python3
"""
Spike S1b — test-startup-cache.py
Item 6: Startup overhead & probe caching:
- Compare extraction time in -J vs in download run
- Test whether --load-info-json (reusing probe JSON) skips re-extraction
- How long resolved URLs stay valid (TTL from 'expire' param)
- Error shown when URLs expire
"""
import json
import os
import subprocess
import time
import urllib.parse

SPIKE_DIR = os.path.dirname(os.path.abspath(__file__))
BIN_DIR = os.path.join(SPIKE_DIR, "bin")
FFMPEG_DIR = os.path.join(BIN_DIR, "ffmpeg-lgpl", "bin")
RESULTS_DIR = os.path.join(SPIKE_DIR, "results", "s1b")
os.makedirs(RESULTS_DIR, exist_ok=True)

YTDLP = os.path.join(BIN_DIR, "yt-dlp.exe")
QJS = os.path.join(BIN_DIR, "qjs.exe")
FFMPEG = os.path.join(FFMPEG_DIR, "ffmpeg.exe")

VIDEO_URL = "https://www.youtube.com/watch?v=QOJIILaJKNc"
FORMAT_SELECTOR = "bv*[vcodec^=avc1][height<=1080]+ba[acodec^=mp4a]/b[vcodec^=avc1][height<=1080]/bv*[height<=1080]+ba/b[height<=1080]"

def test_startup_cache():
    print("=" * 70)
    print("Spike S1b: Startup Overhead & Probe Info Caching")
    print("=" * 70)

    results = {}

    # 1. Measure -J probe time and save info JSON
    print("\n--- Step 1: Probe metadata with -J and save info JSON ---")
    info_json_path = os.path.join(RESULTS_DIR, "probe_info.json")
    t0 = time.monotonic()
    probe_cmd = [
        YTDLP,
        "-J",
        "--no-playlist", "--no-warnings", "--ignore-config",
        "--js-runtimes", f"quickjs:{QJS}",
        VIDEO_URL,
    ]
    p_probe = subprocess.run(probe_cmd, capture_output=True, text=True)
    probe_wall = round(time.monotonic() - t0, 2)
    print(f"Probe exit={p_probe.returncode} in {probe_wall}s")

    if p_probe.returncode == 0:
        with open(info_json_path, "w", encoding="utf-8") as f:
            f.write(p_probe.stdout)
        info = json.loads(p_probe.stdout)
        results["probe_time_s"] = probe_wall
        results["title"] = info.get("title")
        results["duration_s"] = info.get("duration")
        results["availability"] = info.get("availability")
        results["live_status"] = info.get("live_status")

        # Inspect stream URLs for expiration
        formats = info.get("formats", [])
        video_formats = [f for f in formats if f.get("vcodec") != "none" and "expire=" in f.get("url", "")]
        if video_formats:
            sample_url = video_formats[-1]["url"]
            parsed_url = urllib.parse.urlparse(sample_url)
            qs = urllib.parse.parse_qs(parsed_url.query)
            expire_ts = int(qs.get("expire", [0])[0])
            now_ts = int(time.time())
            ttl_s = expire_ts - now_ts
            results["url_expire_epoch"] = expire_ts
            results["current_epoch"] = now_ts
            results["url_ttl_seconds"] = ttl_s
            results["url_ttl_hours"] = round(ttl_s / 3600.0, 2)
            print(f"Stream URL expires in: {ttl_s}s (~{results['url_ttl_hours']} hours)")
    else:
        print(f"Probe failed: {p_probe.stderr}")
        return

    # 2. Test download WITH --load-info-json
    print("\n--- Step 2: Download range WITH --load-info-json ---")
    out_cached = os.path.join(RESULTS_DIR, "cached_info_30s.mp4")
    if os.path.exists(out_cached):
        os.remove(out_cached)

    cmd_cached = [
        YTDLP,
        "--load-info-json", info_json_path,
        "--no-playlist", "--no-warnings", "--newline", "--ignore-config",
        "--ffmpeg-location", FFMPEG_DIR,
        "-f", FORMAT_SELECTOR,
        "--download-sections", "*00:01:00-00:01:30",
        "-o", out_cached,
        VIDEO_URL,
    ]
    t0 = time.monotonic()
    p_cached = subprocess.run(cmd_cached, capture_output=True, text=True)
    cached_wall = round(time.monotonic() - t0, 2)
    print(f"Cached download exit={p_cached.returncode} in {cached_wall}s")
    results["cached_download_time_s"] = cached_wall
    results["cached_output_size_bytes"] = os.path.getsize(out_cached) if os.path.exists(out_cached) else 0

    # 3. Test download WITHOUT --load-info-json (cold run)
    print("\n--- Step 3: Download range WITHOUT --load-info-json (fresh extraction) ---")
    out_fresh = os.path.join(RESULTS_DIR, "fresh_info_30s.mp4")
    if os.path.exists(out_fresh):
        os.remove(out_fresh)

    cmd_fresh = [
        YTDLP,
        "--no-playlist", "--no-warnings", "--newline", "--ignore-config",
        "--ffmpeg-location", FFMPEG_DIR,
        "--js-runtimes", f"quickjs:{QJS}",
        "-f", FORMAT_SELECTOR,
        "--download-sections", "*00:01:00-00:01:30",
        "-o", out_fresh,
        VIDEO_URL,
    ]
    t0 = time.monotonic()
    p_fresh = subprocess.run(cmd_fresh, capture_output=True, text=True)
    fresh_wall = round(time.monotonic() - t0, 2)
    print(f"Fresh download exit={p_fresh.returncode} in {fresh_wall}s")
    results["fresh_download_time_s"] = fresh_wall
    results["fresh_output_size_bytes"] = os.path.getsize(out_fresh) if os.path.exists(out_fresh) else 0
    results["time_saved_s"] = round(fresh_wall - cached_wall, 2)
    print(f"Time saved by --load-info-json: {results['time_saved_s']}s (Probe overhead skipped!)")

    # 4. Test expired URL error
    print("\n--- Step 4: Testing expired URL error behavior ---")
    if video_formats:
        expired_url = re_expire_url(video_formats[-1]["url"], expire_epoch=1000)
        # Attempt to probe or read expired URL via ffmpeg
        p_exp = subprocess.run([FFMPEG, "-headers", f"User-Agent: Mozilla/5.0\r\n", "-i", expired_url, "-t", "1", "-f", "null", "-"], capture_output=True, text=True)
        results["expired_url_test"] = {
            "exit_code": p_exp.returncode,
            "stderr_snippet": p_exp.stderr[-300:] if p_exp.stderr else ""
        }
        print(f"Expired URL exit code: {p_exp.returncode}")
        for l in p_exp.stderr.splitlines()[-5:]:
            print(f"  FFmpeg: {l}")

    with open(os.path.join(RESULTS_DIR, "startup_cache_results.json"), "w") as f:
        json.dump(results, f, indent=2)

def re_expire_url(url: str, expire_epoch: int) -> str:
    parsed = urllib.parse.urlparse(url)
    qs = urllib.parse.parse_qs(parsed.query)
    qs["expire"] = [str(expire_epoch)]
    new_query = urllib.parse.urlencode(qs, doseq=True)
    return urllib.parse.urlunparse((parsed.scheme, parsed.netloc, parsed.path, parsed.params, new_query, parsed.fragment))

if __name__ == "__main__":
    test_startup_cache()
