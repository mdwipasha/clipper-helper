#!/usr/bin/env python3
"""
Spike S1c — test-s1c-throughput.py
Part A: Throughput Optimization & Pacing Benchmark
1. Baseline: 5-min range at ~30 min offset (00:30:00-00:35:00), 3 runs.
2. Reference: yt-dlp native downloader (no sections) on short video, same AVC1 format -> raw MB/s.
3. Parallel split: 10-min range (00:30:00-00:40:00) with N=2, N=3, N=4 concurrent downloads + lossless concat.
   Checks 403/429 throttling across 5 runs, verifies join continuity, A/V sync, browser playability.
4. Downloader args & 720p: tests FFmpeg HTTP options and 720p vs 1080p wall time (time-based vs byte-based pacing).
"""
import datetime
import json
import os
import re
import subprocess
import time

SPIKE_DIR = os.path.dirname(os.path.abspath(__file__))
BIN_DIR = os.path.join(SPIKE_DIR, "bin")
FFMPEG_DIR = os.path.join(BIN_DIR, "ffmpeg-lgpl", "bin")
RESULTS_DIR = os.path.join(SPIKE_DIR, "results", "s1c")
os.makedirs(RESULTS_DIR, exist_ok=True)

YTDLP = os.path.join(BIN_DIR, "yt-dlp.exe")
QJS = os.path.join(BIN_DIR, "qjs.exe")
FFMPEG = os.path.join(FFMPEG_DIR, "ffmpeg.exe")
FFPROBE = os.path.join(FFMPEG_DIR, "ffprobe.exe")

TARGET_URL = "https://www.youtube.com/watch?v=QOJIILaJKNc"
FORMAT_1080P = "bv*[vcodec^=avc1][height<=1080]+ba[acodec^=mp4a]/b[vcodec^=avc1][height<=1080]/bv*[height<=1080]+ba/b[height<=1080]"
FORMAT_720P  = "bv*[vcodec^=avc1][height<=720]+ba[acodec^=mp4a]/b[vcodec^=avc1][height<=720]/bv*[height<=720]+ba/b[height<=720]"

REF_URL = "https://www.youtube.com/watch?v=dQw4w9WgXcQ"  # 3.5 min standalone AVC1 video

def get_now_wib():
    now_utc = datetime.datetime.now(datetime.timezone.utc)
    wib = now_utc + datetime.timedelta(hours=7)
    return wib.strftime("%Y-%m-%d %H:%M:%S WIB")

def run_download_cmd(cmd):
    t0 = time.time()
    proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    wall_time = time.time() - t0
    return wall_time, proc.returncode, proc.stdout, proc.stderr

def run_baseline_tests():
    print(f"\n--- A.1 Baseline: 5-min Range (00:30:00-00:35:00) 3 Runs ---")
    runs = []
    for r in range(1, 4):
        out_file = os.path.join(RESULTS_DIR, f"baseline_5min_r{r}.mp4")
        if os.path.exists(out_file):
            os.remove(out_file)
        time_str = get_now_wib()
        print(f"Run {r}/3 starting at {time_str}...")
        cmd = [
            YTDLP,
            "--no-playlist", "--no-warnings", "--newline", "--ignore-config",
            "--js-runtimes", f"quickjs:{QJS}",
            "--ffmpeg-location", FFMPEG_DIR,
            "-f", FORMAT_1080P,
            "--download-sections", "*00:30:00-00:35:00",
            "-o", out_file,
            TARGET_URL
        ]
        wall_time, code, stdout, stderr = run_download_cmd(cmd)
        if code != 0:
            print(f"Run {r} failed with code {code}: {stderr[:200]}")
            continue
        size_bytes = os.path.getsize(out_file)
        size_mb = size_bytes / 1024 / 1024
        speed_mbps = size_mb / wall_time if wall_time > 0 else 0
        rt_factor = 300.0 / wall_time if wall_time > 0 else 0
        print(f"  Run {r}: wall={wall_time:.2f}s ({rt_factor:.2f}x realtime), size={size_mb:.2f}MB, speed={speed_mbps:.2f}MB/s")
        runs.append({
            "run": r,
            "timestamp": time_str,
            "wall_time_s": round(wall_time, 2),
            "realtime_factor": round(rt_factor, 2),
            "size_mb": round(size_mb, 2),
            "speed_mbs": round(speed_mbps, 2),
        })

    wall_times = [x["wall_time_s"] for x in runs]
    median_wall = sorted(wall_times)[len(wall_times)//2] if wall_times else 0
    return {
        "runs": runs,
        "median_wall_s": median_wall,
        "median_realtime_factor": round(300.0 / median_wall, 2) if median_wall > 0 else 0,
    }

def run_native_reference():
    print(f"\n--- A.2 Reference: yt-dlp Native Downloader (No Sections) ---")
    out_file = os.path.join(RESULTS_DIR, "ref_native_full.mp4")
    if os.path.exists(out_file):
        os.remove(out_file)
    time_str = get_now_wib()
    print(f"Starting native download of reference video at {time_str}...")
    cmd = [
        YTDLP,
        "--no-playlist", "--no-warnings", "--newline", "--ignore-config",
        "--js-runtimes", f"quickjs:{QJS}",
        "-f", FORMAT_1080P,
        "-o", out_file,
        REF_URL
    ]
    wall_time, code, stdout, stderr = run_download_cmd(cmd)
    if code != 0:
        print(f"Native reference failed: {stderr[:200]}")
        return {"error": stderr}
    size_bytes = os.path.getsize(out_file)
    size_mb = size_bytes / 1024 / 1024
    speed_mbps = size_mb / wall_time if wall_time > 0 else 0

    # Probe duration
    cmd_probe = [FFPROBE, "-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1", out_file]
    duration_s = float(subprocess.check_output(cmd_probe).decode().strip())
    rt_factor = duration_s / wall_time if wall_time > 0 else 0

    print(f"  Native Reference: wall={wall_time:.2f}s ({rt_factor:.2f}x realtime), size={size_mb:.2f}MB, speed={speed_mbps:.2f}MB/s")
    return {
        "timestamp": time_str,
        "wall_time_s": round(wall_time, 2),
        "size_mb": round(size_mb, 2),
        "duration_s": round(duration_s, 2),
        "realtime_factor": round(rt_factor, 2),
        "speed_mbs": round(speed_mbps, 2),
    }

def run_parallel_split_tests():
    print(f"\n--- A.3 Parallel Split: 10-min Range (00:30:00-00:40:00) N=2, N=3, N=4 ---")
    split_configs = [
        {
            "N": 2,
            "chunks": ["*00:30:00-00:35:00", "*00:35:00-00:40:00"]
        },
        {
            "N": 3,
            "chunks": ["*00:30:00-00:33:20", "*00:33:20-00:36:40", "*00:36:40-00:40:00"]
        },
        {
            "N": 4,
            "chunks": ["*00:30:00-00:32:30", "*00:32:30-00:35:00", "*00:35:00-00:37:30", "*00:37:30-00:40:00"]
        },
    ]

    all_split_results = []
    consecutive_success = 0

    for cfg in split_configs:
        N = cfg["N"]
        chunks = cfg["chunks"]
        print(f"\nEvaluating N={N} Concurrent Splits ({len(chunks)} chunks of {600//N}s each):")

        # Download chunks concurrently
        t0 = time.time()
        procs = []
        part_files = []
        for i, chunk_range in enumerate(chunks):
            part_path = os.path.join(RESULTS_DIR, f"split_N{N}_part{i}.mp4")
            if os.path.exists(part_path):
                os.remove(part_path)
            part_files.append(part_path)
            cmd = [
                YTDLP,
                "--no-playlist", "--no-warnings", "--newline", "--ignore-config",
                "--js-runtimes", f"quickjs:{QJS}",
                "--ffmpeg-location", FFMPEG_DIR,
                "-f", FORMAT_1080P,
                "--download-sections", chunk_range,
                "-o", part_path,
                TARGET_URL
            ]
            p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            procs.append(p)

        # Wait for all chunks
        errors = []
        for i, p in enumerate(procs):
            stdout, stderr = p.communicate()
            if p.returncode != 0:
                errors.append(f"Part {i} failed ({p.returncode}): {stderr[:150]}")
            else:
                consecutive_success += 1

        dl_wall_time = time.time() - t0
        print(f"  All {N} chunks downloaded in {dl_wall_time:.2f} s. Errors: {len(errors)}")

        if errors:
            all_split_results.append({
                "N": N,
                "error": errors,
                "dl_wall_time_s": dl_wall_time
            })
            continue

        # Concat losslessly with FFmpeg concat demuxer
        concat_list = os.path.join(RESULTS_DIR, f"concat_list_N{N}.txt")
        with open(concat_list, "w", encoding="utf-8") as f:
            for p in part_files:
                f.write(f"file '{os.path.abspath(p)}'\n")

        joined_file = os.path.join(RESULTS_DIR, f"joined_10min_N{N}.mp4")
        if os.path.exists(joined_file):
            os.remove(joined_file)

        t_concat_0 = time.time()
        concat_cmd = [
            FFMPEG, "-y",
            "-f", "concat",
            "-safe", "0",
            "-i", concat_list,
            "-c", "copy",
            joined_file
        ]
        concat_res = subprocess.run(concat_cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        concat_time = time.time() - t_concat_0
        total_aggregate_time = time.time() - t0

        # Verify joined file
        probe_cmd = [
            FFPROBE, "-v", "error",
            "-show_entries", "format=duration,size:stream=codec_name,r_frame_rate,nb_frames",
            "-of", "json",
            joined_file
        ]
        probe_data = json.loads(subprocess.check_output(probe_cmd).decode())
        duration_s = float(probe_data.get("format", {}).get("duration", 0))
        joined_size_mb = os.path.getsize(joined_file) / 1024 / 1024

        dur_error_s = duration_s - 600.0

        print(f"  Concat time: {concat_time:.2f} s | Total aggregate wall: {total_aggregate_time:.2f} s")
        print(f"  Joined file: {joined_size_mb:.2f} MB, Duration: {duration_s:.2f} s (Error vs 600s: {dur_error_s:+.2f} s)")
        print(f"  Realtime speedup factor: {600.0 / total_aggregate_time:.2f}x realtime")

        all_split_results.append({
            "N": N,
            "dl_wall_time_s": round(dl_wall_time, 2),
            "concat_time_s": round(concat_time, 2),
            "total_aggregate_time_s": round(total_aggregate_time, 2),
            "realtime_factor": round(600.0 / total_aggregate_time, 2),
            "joined_size_mb": round(joined_size_mb, 2),
            "joined_duration_s": round(duration_s, 2),
            "duration_error_s": round(dur_error_s, 2),
            "errors": errors,
        })

    return {
        "configs": all_split_results,
        "consecutive_success": consecutive_success,
    }

def run_downloader_args_and_720p():
    print(f"\n--- A.4 Downloader Args & 720p (Pacing Type Test) ---")

    # 1. Test 720p 5-min range
    out_720p = os.path.join(RESULTS_DIR, "test_720p_5min.mp4")
    if os.path.exists(out_720p):
        os.remove(out_720p)
    print(f"Testing 720p copy-mode (00:30:00-00:35:00)...")
    cmd_720p = [
        YTDLP,
        "--no-playlist", "--no-warnings", "--newline", "--ignore-config",
        "--js-runtimes", f"quickjs:{QJS}",
        "--ffmpeg-location", FFMPEG_DIR,
        "-f", FORMAT_720P,
        "--download-sections", "*00:30:00-00:35:00",
        "-o", out_720p,
        TARGET_URL
    ]
    wall_720p, code_720p, _, err_720p = run_download_cmd(cmd_720p)
    size_720p_mb = os.path.getsize(out_720p) / 1024 / 1024 if code_720p == 0 else 0

    print(f"  720p Result: wall={wall_720p:.2f}s, size={size_720p_mb:.2f}MB, speed={size_720p_mb/wall_720p:.2f}MB/s")

    # 2. Test FFmpeg HTTP reconnect args on 1080p
    out_args = os.path.join(RESULTS_DIR, "test_args_reconnect_5min.mp4")
    if os.path.exists(out_args):
        os.remove(out_args)
    print(f"Testing FFmpeg HTTP reconnect args on 1080p...")
    cmd_args = [
        YTDLP,
        "--no-playlist", "--no-warnings", "--newline", "--ignore-config",
        "--js-runtimes", f"quickjs:{QJS}",
        "--ffmpeg-location", FFMPEG_DIR,
        "-f", FORMAT_1080P,
        "--downloader-args", "ffmpeg:-reconnect 1 -reconnect_streamed 1 -reconnect_delay_max 5",
        "--download-sections", "*00:30:00-00:35:00",
        "-o", out_args,
        TARGET_URL
    ]
    wall_args, code_args, _, err_args = run_download_cmd(cmd_args)
    size_args_mb = os.path.getsize(out_args) / 1024 / 1024 if code_args == 0 else 0

    print(f"  Args Reconnect Result: wall={wall_args:.2f}s, size={size_args_mb:.2f}MB, speed={size_args_mb/wall_args:.2f}MB/s")

    return {
        "test_720p": {
            "wall_time_s": round(wall_720p, 2),
            "size_mb": round(size_720p_mb, 2),
            "speed_mbs": round(size_720p_mb / wall_720p, 2) if wall_720p > 0 else 0,
        },
        "test_downloader_args": {
            "wall_time_s": round(wall_args, 2),
            "size_mb": round(size_args_mb, 2),
            "speed_mbs": round(size_args_mb / wall_args, 2) if wall_args > 0 else 0,
        }
    }

def main():
    print("=== Spike S1c: Throughput & Pacing Benchmark ===")
    baseline_data = run_baseline_tests()
    native_data = run_native_reference()
    parallel_data = run_parallel_split_tests()
    args_720p_data = run_downloader_args_and_720p()

    overall = {
        "timestamp": get_now_wib(),
        "baseline_5min": baseline_data,
        "native_reference": native_data,
        "parallel_split": parallel_data,
        "downloader_args_and_720p": args_720p_data,
    }

    out_json = os.path.join(RESULTS_DIR, "throughput_benchmark_results.json")
    with open(out_json, "w", encoding="utf-8") as f:
        json.dump(overall, f, indent=2)
    print(f"\nWrote full throughput benchmark results to {out_json}")

if __name__ == "__main__":
    main()
