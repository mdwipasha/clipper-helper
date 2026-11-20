#!/usr/bin/env python3
"""
Spike S1c — test-s1c-proxy.py
Part D: h264_mf Proxy Benchmark on non-AVC1 video (VP9 1080p60)
- Encodes VP9 1080p60 sample at 3 explicit bitrates: 2000k (2.0 Mbps), 3500k (3.5 Mbps), 5000k (5.0 Mbps)
- Measures wall time, encode fps, realtime factor, output file size
- Computes objective quality metrics (SSIM, PSNR) via FFmpeg filter
- Visual / playback verification
"""
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

FFMPEG = os.path.join(FFMPEG_DIR, "ffmpeg.exe")
FFPROBE = os.path.join(FFMPEG_DIR, "ffprobe.exe")

INPUT_FILE = os.path.join(SPIKE_DIR, "results", "s1b", "source_vp9_1080p_video.webm")
if not os.path.exists(INPUT_FILE):
    raise FileNotFoundError(f"Missing input sample: {INPUT_FILE}")

BITRATES = [
    {"label": "2.0_Mbps", "bitrate": "2000k", "bps": 2000000},
    {"label": "3.5_Mbps", "bitrate": "3500k", "bps": 3500000},
    {"label": "5.0_Mbps", "bitrate": "5000k", "bps": 5000000},
]

def run_proxy_test():
    results = []

    # Get duration of input
    probe_cmd = [
        FFPROBE, "-v", "error",
        "-show_entries", "format=duration",
        "-of", "default=noprint_wrappers=1:nokey=1",
        INPUT_FILE
    ]
    duration_sec = float(subprocess.check_output(probe_cmd).decode().strip())
    print(f"Input sample duration: {duration_sec:.2f} s")

    for cfg in BITRATES:
        label = cfg["label"]
        bitrate = cfg["bitrate"]
        out_path = os.path.join(RESULTS_DIR, f"proxy_{label}.mp4")
        if os.path.exists(out_path):
            os.remove(out_path)

        print(f"\n--- Testing h264_mf @ {bitrate} ({label}) ---")
        cmd = [
            FFMPEG, "-y",
            "-i", INPUT_FILE,
            "-c:v", "h264_mf",
            "-b:v", bitrate,
            "-pix_fmt", "yuv420p",
            "-c:a", "aac",
            "-b:a", "128k",
            out_path
        ]

        t0 = time.time()
        proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        wall_time = time.time() - t0

        if proc.returncode != 0:
            print(f"Error encoding {label}: {proc.stderr[:300]}")
            continue

        file_size = os.path.getsize(out_path)
        realtime_factor = duration_sec / wall_time if wall_time > 0 else 0
        effective_bitrate_kbps = (file_size * 8 / 1000) / duration_sec

        # Parse fps from ffmpeg stderr
        fps_matches = re.findall(r"(\d+(?:\.\d+)?)\s*fps", proc.stderr)
        avg_fps = float(fps_matches[-1]) if fps_matches else (duration_sec * 60 / wall_time)

        print(f"  Wall time: {wall_time:.2f} s ({realtime_factor:.2f}x realtime)")
        print(f"  Encode Speed: {avg_fps:.1f} fps")
        print(f"  Output size: {file_size / 1024 / 1024:.2f} MB ({effective_bitrate_kbps:.1f} kbps)")

        # Calculate SSIM & PSNR against original
        print("  Computing SSIM & PSNR...")
        metric_cmd = [
            FFMPEG,
            "-i", out_path,
            "-i", INPUT_FILE,
            "-filter_complex", "[0:v][1:v]ssim=stats_file=ssim.log;[0:v][1:v]psnr=stats_file=psnr.log",
            "-f", "null", "-"
        ]
        metric_proc = subprocess.run(metric_cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, cwd=RESULTS_DIR)

        ssim_val = "N/A"
        psnr_val = "N/A"
        ssim_match = re.search(r"SSIM\s+Y:([0-9.]+)\s+\([0-9.]+\)\s+U:([0-9.]+)\s+V:([0-9.]+)\s+All:([0-9.]+)", metric_proc.stderr)
        if ssim_match:
            ssim_val = ssim_match.group(4)

        psnr_match = re.search(r"average:([0-9.]+)", metric_proc.stderr)
        if psnr_match:
            psnr_val = psnr_match.group(1)

        print(f"  SSIM (All): {ssim_val}, PSNR (average): {psnr_val} dB")

        # Cleanup metric logs
        for tmp in ["ssim.log", "psnr.log"]:
            tmp_path = os.path.join(RESULTS_DIR, tmp)
            if os.path.exists(tmp_path):
                os.remove(tmp_path)

        results.append({
            "label": label,
            "target_bitrate": bitrate,
            "wall_time_s": round(wall_time, 2),
            "realtime_factor": round(realtime_factor, 2),
            "avg_fps": round(avg_fps, 1),
            "file_size_bytes": file_size,
            "file_size_mb": round(file_size / 1024 / 1024, 2),
            "effective_kbps": round(effective_bitrate_kbps, 1),
            "ssim": ssim_val,
            "psnr_db": psnr_val,
        })

    out_json = os.path.join(RESULTS_DIR, "proxy_benchmark_results.json")
    with open(out_json, "w", encoding="utf-8") as f:
        json.dump(results, f, indent=2)
    print(f"\nSaved results to {out_json}")

if __name__ == "__main__":
    run_proxy_test()
