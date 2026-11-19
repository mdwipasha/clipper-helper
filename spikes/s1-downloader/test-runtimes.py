#!/usr/bin/env python3
"""
Spike S1 — test-runtimes.py
Compare Deno vs QuickJS as yt-dlp JS runtimes.
Measures: probe time (yt-dlp -J), download time (1-min range).
"""
import json
import os
import subprocess
import sys
import time

SPIKE_DIR = os.path.dirname(os.path.abspath(__file__))
BIN_DIR = os.path.join(SPIKE_DIR, "bin")
RESULTS_DIR = os.path.join(SPIKE_DIR, "results")

YTDLP = os.path.join(BIN_DIR, "yt-dlp.exe")
DENO = os.path.join(BIN_DIR, "deno.exe")
QJS = os.path.join(BIN_DIR, "qjs.exe")

FFMPEG_DIR = None
for root, dirs, files in os.walk(BIN_DIR):
    if "ffmpeg.exe" in files:
        FFMPEG_DIR = root
        break

VIDEO_URL = "https://www.youtube.com/watch?v=QOJIILaJKNc"
FORMAT_SELECTOR = "bv*[vcodec^=avc1][height<=1080]+ba[acodec^=mp4a]/b[vcodec^=avc1][height<=1080]/bv*[height<=1080]+ba/b[height<=1080]"


def test_probe(runtime_name: str, runtime_flag: str) -> dict:
    """Time yt-dlp -J (metadata probe) with a given runtime."""
    cmd = [
        YTDLP, "-J", "--no-playlist", "--no-warnings", "--ignore-config",
        "--js-runtimes", runtime_flag,
        "-f", FORMAT_SELECTOR,
        VIDEO_URL,
    ]
    print(f"\n  Probing with {runtime_name}...")
    t0 = time.monotonic()
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
    elapsed = time.monotonic() - t0
    
    result = {
        "runtime": runtime_name,
        "operation": "probe",
        "exit_code": r.returncode,
        "time_s": round(elapsed, 2),
        "success": r.returncode == 0,
    }
    if r.returncode == 0:
        info = json.loads(r.stdout)
        result["title"] = info.get("title", "?")[:50]
    else:
        result["error"] = r.stderr[:300]
    
    print(f"  Result: {'OK' if result['success'] else 'FAIL'} in {result['time_s']}s")
    return result


def test_download(runtime_name: str, runtime_flag: str) -> dict:
    """Time a 1-min section download with a given runtime."""
    os.makedirs(RESULTS_DIR, exist_ok=True)
    output = os.path.join(RESULTS_DIR, f"runtime_{runtime_name}_1min.mp4")
    if os.path.exists(output):
        os.remove(output)

    cmd = [
        YTDLP,
        "--no-playlist", "--no-warnings", "--newline", "--ignore-config",
        "--ffmpeg-location", FFMPEG_DIR,
        "-f", FORMAT_SELECTOR,
        "--download-sections", "*00:01:00-00:02:00",
        "--force-keyframes-at-cuts",
        "--js-runtimes", runtime_flag,
        "-o", output,
        VIDEO_URL,
    ]
    print(f"\n  Downloading 1min with {runtime_name}...")
    t0 = time.monotonic()
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=300)
    elapsed = time.monotonic() - t0

    result = {
        "runtime": runtime_name,
        "operation": "download_1min",
        "exit_code": r.returncode,
        "time_s": round(elapsed, 2),
        "success": r.returncode == 0,
    }
    if os.path.exists(output):
        result["file_size_mb"] = round(os.path.getsize(output) / 1e6, 2)
    else:
        # Check for other extensions
        for f in os.listdir(RESULTS_DIR):
            if f.startswith(f"runtime_{runtime_name}_1min"):
                fp = os.path.join(RESULTS_DIR, f)
                result["file_size_mb"] = round(os.path.getsize(fp) / 1e6, 2)
                result["actual_file"] = f
                break
    
    if r.returncode != 0:
        result["error"] = r.stderr[:300]
    
    print(f"  Result: {'OK' if result['success'] else 'FAIL'} in {result['time_s']}s, "
          f"size={result.get('file_size_mb', '?')}MB")
    return result


def main():
    if not os.path.exists(YTDLP):
        print("ERROR: yt-dlp not found")
        sys.exit(1)

    runtimes = [
        ("deno", f"deno:{DENO}"),
    ]
    if os.path.exists(QJS):
        runtimes.append(("quickjs", f"quickjs:{QJS}"))
    else:
        print("WARNING: QuickJS binary not found, skipping")

    # Binary sizes
    print("=" * 60)
    print("RUNTIME BINARY SIZES")
    print("=" * 60)
    deno_size = os.path.getsize(DENO)
    print(f"  Deno:    {deno_size / 1e6:.1f} MB")
    if os.path.exists(QJS):
        qjs_size = os.path.getsize(QJS)
        print(f"  QuickJS: {qjs_size / 1e6:.1f} MB")
        print(f"  Ratio:   Deno is {deno_size / qjs_size:.0f}x larger")

    all_results = []

    # Probe tests (run twice each for consistency)
    print("\n" + "=" * 60)
    print("PROBE TESTS (yt-dlp -J)")
    print("=" * 60)
    for runtime_name, runtime_flag in runtimes:
        # Warm-up run
        r = test_probe(runtime_name, runtime_flag)
        all_results.append(r)
        # Second run
        r2 = test_probe(runtime_name + "_run2", runtime_flag)
        all_results.append(r2)

    # Download tests
    print("\n" + "=" * 60)
    print("DOWNLOAD TESTS (1-min section)")
    print("=" * 60)
    for runtime_name, runtime_flag in runtimes:
        r = test_download(runtime_name, runtime_flag)
        all_results.append(r)

    # Summary
    print("\n" + "=" * 60)
    print("RUNTIME COMPARISON SUMMARY")
    print("=" * 60)
    print(f"{'Runtime':<15} {'Operation':<20} {'Time (s)':>10} {'Size (MB)':>10} {'Status':>8}")
    for r in all_results:
        print(f"{r['runtime']:<15} {r['operation']:<20} {r['time_s']:>10} "
              f"{r.get('file_size_mb', '-'):>10} "
              f"{'OK' if r['success'] else 'FAIL':>8}")

    # Save results
    with open(os.path.join(RESULTS_DIR, "runtime_comparison.json"), "w") as f:
        json.dump(all_results, f, indent=2)


if __name__ == "__main__":
    main()
