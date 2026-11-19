#!/usr/bin/env python3
"""
Spike S1b — test-cancel-resume.py
Item 5: Cancel & Resume, Leftover files, and Concurrency test:
- Kill yt-dlp process tree mid-download on Windows
- List leftover files (.part/temp)
- Test whether re-run resumes or restarts
- Check for orphan ffmpeg processes
- Test two concurrent downloads for throttling
"""
import json
import os
import subprocess
import time

SPIKE_DIR = os.path.dirname(os.path.abspath(__file__))
BIN_DIR = os.path.join(SPIKE_DIR, "bin")
FFMPEG_DIR = os.path.join(BIN_DIR, "ffmpeg-lgpl", "bin")
RESULTS_DIR = os.path.join(SPIKE_DIR, "results", "s1b")
os.makedirs(RESULTS_DIR, exist_ok=True)

YTDLP = os.path.join(BIN_DIR, "yt-dlp.exe")
QJS = os.path.join(BIN_DIR, "qjs.exe")

VIDEO_URL = "https://www.youtube.com/watch?v=QOJIILaJKNc"
FORMAT_SELECTOR = "bv*[vcodec^=avc1][height<=1080]+ba[acodec^=mp4a]/b[vcodec^=avc1][height<=1080]/bv*[height<=1080]+ba/b[height<=1080]"

def run_cancel_resume_test():
    print("=" * 70)
    print("Spike S1b: Cancel & Resume, Orphan Processes, and Concurrency")
    print("=" * 70)

    results = {}

    # --- Part 1: Kill Mid-Download ---
    print("\n--- Part 1: Process Kill Mid-Download ---")
    cancel_out = os.path.join(RESULTS_DIR, "cancel_test.mp4")
    part_file = cancel_out + ".part"
    for f in [cancel_out, part_file]:
        if os.path.exists(f):
            os.remove(f)

    cmd = [
        YTDLP,
        "--no-playlist", "--no-warnings", "--newline", "--ignore-config",
        "--ffmpeg-location", FFMPEG_DIR,
        "--js-runtimes", f"quickjs:{QJS}",
        "-f", FORMAT_SELECTOR,
        "--download-sections", "*00:01:00-00:06:00",  # 5 min download
        "-o", cancel_out,
        VIDEO_URL,
    ]

    print(f"Starting download process (PID)...")
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    print(f"Spawned yt-dlp with PID={proc.pid}")

    # Let it run for 10 seconds to establish FFmpeg and start receiving data
    time.sleep(10)

    # Check child processes before kill
    print(f"Terminating process tree for PID={proc.pid}...")
    kill_cmd = f"taskkill /F /T /PID {proc.pid}"
    kill_res = subprocess.run(kill_cmd, shell=True, capture_output=True, text=True)
    print(f"Kill output: {kill_res.stdout.strip()}")

    # Check if any ffmpeg or yt-dlp remains
    time.sleep(1)
    orphans = check_orphan_processes()
    results["orphan_processes"] = orphans
    print(f"Orphan processes found: {orphans}")

    # Check leftover files
    leftovers = []
    for fname in os.listdir(RESULTS_DIR):
        if fname.startswith("cancel_test"):
            fpath = os.path.join(RESULTS_DIR, fname)
            sz = os.path.getsize(fpath)
            leftovers.append({"filename": fname, "size_bytes": sz, "size_mb": round(sz / 1e6, 3)})
            print(f"Leftover file: {fname} ({round(sz / 1e6, 3)} MB)")

    results["leftover_files"] = leftovers

    # --- Part 2: Resume vs Restart ---
    print("\n--- Part 2: Re-run with Leftover .part File ---")
    t0 = time.monotonic()
    proc_resume = subprocess.run(cmd, capture_output=True, text=True)
    resume_wall = round(time.monotonic() - t0, 2)
    print(f"Re-run completed: exit={proc_resume.returncode} in {resume_wall}s")

    out_sz = os.path.getsize(cancel_out) if os.path.exists(cancel_out) else 0
    results["resume_test"] = {
        "exit_code": proc_resume.returncode,
        "wall_time_s": resume_wall,
        "final_size_bytes": out_sz,
        "final_size_mb": round(out_sz / 1e6, 2),
        "stdout_tail": proc_resume.stdout[-500:] if proc_resume.stdout else "",
    }
    # Check if restart occurred (e.g. FFmpeg overwriting .part)
    resumed = "resuming" in proc_resume.stdout.lower() or "resuming" in proc_resume.stderr.lower()
    print(f"Did it resume or restart? {'RESUMED' if resumed else 'RESTARTED / OVERWRITTEN'}")
    results["resume_test"]["resumed_vs_restarted"] = "resumed" if resumed else "restarted_overwritten"

    # --- Part 3: Concurrent Downloads ---
    print("\n--- Part 3: Two Concurrent Downloads ---")
    c1_out = os.path.join(RESULTS_DIR, "concurrent_1.mp4")
    c2_out = os.path.join(RESULTS_DIR, "concurrent_2.mp4")
    for f in [c1_out, c2_out]:
        if os.path.exists(f):
            os.remove(f)

    cmd1 = [
        YTDLP,
        "--no-playlist", "--no-warnings", "--newline", "--ignore-config",
        "--ffmpeg-location", FFMPEG_DIR,
        "--js-runtimes", f"quickjs:{QJS}",
        "-f", FORMAT_SELECTOR,
        "--download-sections", "*00:01:00-00:02:00",
        "-o", c1_out,
        VIDEO_URL,
    ]
    cmd2 = [
        YTDLP,
        "--no-playlist", "--no-warnings", "--newline", "--ignore-config",
        "--ffmpeg-location", FFMPEG_DIR,
        "--js-runtimes", f"quickjs:{QJS}",
        "-f", FORMAT_SELECTOR,
        "--download-sections", "*00:05:00-00:06:00",
        "-o", c2_out,
        VIDEO_URL,
    ]

    t0 = time.monotonic()
    p1 = subprocess.Popen(cmd1, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    p2 = subprocess.Popen(cmd2, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    p1_out, p1_err = p1.communicate()
    p2_out, p2_err = p2.communicate()
    concurrent_total_wall = round(time.monotonic() - t0, 2)

    sz1 = os.path.getsize(c1_out) if os.path.exists(c1_out) else 0
    sz2 = os.path.getsize(c2_out) if os.path.exists(c2_out) else 0

    results["concurrent_downloads"] = {
        "p1_exit": p1.returncode,
        "p2_exit": p2.returncode,
        "total_wall_s": concurrent_total_wall,
        "p1_size_mb": round(sz1 / 1e6, 2),
        "p2_size_mb": round(sz2 / 1e6, 2),
        "both_succeeded": p1.returncode == 0 and p2.returncode == 0,
    }
    print(f"Concurrent downloads finished in {concurrent_total_wall}s:")
    print(f"  P1 exit={p1.returncode}, size={round(sz1 / 1e6, 2)} MB")
    print(f"  P2 exit={p2.returncode}, size={round(sz2 / 1e6, 2)} MB")

    with open(os.path.join(RESULTS_DIR, "cancel_resume_results.json"), "w") as f:
        json.dump(results, f, indent=2)

def check_orphan_processes():
    try:
        out = subprocess.check_output("tasklist /FI \"IMAGENAME eq ffmpeg.exe\" /FO CSV", shell=True, text=True)
        lines = [l for l in out.splitlines() if "ffmpeg.exe" in l]
        return lines
    except Exception as e:
        return [str(e)]

if __name__ == "__main__":
    run_cancel_resume_test()
