"""
Empirical Benchmark Suite for Spike S4
Executes 3 runs per configuration for 60s 1080x1920 vertical video export.
Measures wall time, speed, CPU %, peak RAM, file size, bitrate, SSIM, PSNR.
Compares H.264 vs VP9 source, AMF vs MF encoder, software decode vs D3D11VA decode.
"""

import ctypes
import json
import os
import re
import statistics
import subprocess
import time
from caption_generator import generate_ass_script
from render_builder import build_ffmpeg_args

FFMPEG_EXE = os.path.abspath(r"spikes\s5-ffmpeg\bin\ffmpeg-release-shared\ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0\bin\ffmpeg.exe")
FFPROBE_EXE = os.path.abspath(r"spikes\s5-ffmpeg\bin\ffmpeg-release-shared\ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0\bin\ffprobe.exe")
RESULTS_DIR = os.path.abspath("spikes/s4-render/results")
FONTS_DIR = os.path.abspath("spikes/s4-render/fonts")
os.makedirs(RESULTS_DIR, exist_ok=True)

# Sources
SRC_H264 = os.path.abspath(r"spikes\s1-downloader\results\s1c\baseline_5min_r1.mp4")
SRC_VP9 = os.path.abspath(r"spikes\s1-downloader\results\s1b\source_vp9_1080p_video.webm")


class FILETIME(ctypes.Structure):
    _fields_ = [("dwLowDateTime", ctypes.c_uint32), ("dwHighDateTime", ctypes.c_uint32)]


class PROCESS_MEMORY_COUNTERS(ctypes.Structure):
    _fields_ = [
        ("cb", ctypes.c_uint32),
        ("PageFaultCount", ctypes.c_uint32),
        ("PeakWorkingSetSize", ctypes.c_uint64),
        ("WorkingSetSize", ctypes.c_uint64),
        ("QuotaPeakPagedPoolUsage", ctypes.c_uint64),
        ("QuotaPagedPoolUsage", ctypes.c_uint64),
        ("QuotaPeakNonPagedPoolUsage", ctypes.c_uint64),
        ("QuotaNonPagedPoolUsage", ctypes.c_uint64),
        ("PagefileUsage", ctypes.c_uint64),
        ("PeakPagefileUsage", ctypes.c_uint64),
    ]


GetProcessMemoryInfo = ctypes.windll.psapi.GetProcessMemoryInfo
GetProcessMemoryInfo.argtypes = [ctypes.c_void_p, ctypes.POINTER(PROCESS_MEMORY_COUNTERS), ctypes.c_uint32]
GetProcessMemoryInfo.restype = ctypes.c_bool


def filetime_to_seconds(ft: FILETIME) -> float:
    return (ft.dwHighDateTime << 32 | ft.dwLowDateTime) / 10000000.0


def create_60s_caption() -> str:
    # 60 seconds of realistic Indonesian spoken words
    sample_phrases = [
        "Halo teman-teman selamat datang kembali di livestream Clipper Helper.",
        "Hari ini kita akan mengulas pertandingan e-sports yang sangat intens.",
        "Perhatikan bagaimana tim melakukan rotasi cepat di area tengah.",
        "Gerakan counter attack yang luar biasa berhasil membalikkan keadaan.",
        "Momen clutch ini menjadi penentu kemenangan dramatis di menit akhir.",
        "Jangan lupa like share dan subscribe untuk highlight selanjutnya ya!"
    ]
    words = []
    t = 1000
    for phrase in sample_phrases:
        tokens = phrase.split()
        duration_per_token = 450
        for tok in tokens:
            words.append({"t0": t, "t1": t + duration_per_token, "text": tok})
            t += duration_per_token + 50
        t += 800  # pause between phrases
        if t > 58000:
            break

    style = {
        "fontFamily": "Montserrat",
        "fontSizePct": 0.045,
        "weight": 700,
        "color": "#FFFFFF",
        "highlightColor": "#FFD700",
        "outlineColor": "#000000",
        "outlinePx": 4,
        "shadow": True,
        "positionAnchor": "bottom",
        "offsetYPct": 0.16,
        "animation": "word_highlight",
        "uppercase": True,
        "maxWordsPerLine": 4,
    }
    ass_path = os.path.join(RESULTS_DIR, "benchmark_caption.ass")
    content = generate_ass_script(words, style, fonts_dir=FONTS_DIR)
    with open(ass_path, "w", encoding="utf-8") as f:
        f.write(content)
    return ass_path


def measure_quality(encoded_file: str, reference_file: str) -> dict:
    """Computes SSIM and PSNR between encoded file and reference."""
    cmd = [
        FFMPEG_EXE, "-y", "-hide_banner",
        "-i", encoded_file, "-i", reference_file,
        "-filter_complex", "[0:v][1:v]ssim[v_ssim];[0:v][1:v]psnr[v_psnr]",
        "-map", "[v_ssim]", "-map", "[v_psnr]",
        "-f", "null", "-"
    ]
    res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    ssim = None
    psnr = None
    for line in res.stderr.splitlines():
        if "SSIM" in line and "All:" in line:
            m = re.search(r"All:([0-9.]+)", line)
            if m:
                ssim = float(m.group(1))
        if "PSNR" in line and "average:" in line:
            m = re.search(r"average:([0-9.]+)", line)
            if m:
                psnr = float(m.group(1))
    return {"ssim": ssim, "psnr": psnr}


def run_single_benchmark(
    name: str,
    source_path: str,
    encoder: str,
    hwaccel: str,
    caption_ass: str,
    run_idx: int
) -> dict:
    out_file = os.path.join(RESULTS_DIR, f"{name}_run{run_idx}.mp4")
    comp = {
        "schemaVersion": 1,
        "id": f"bench-{name}-{run_idx}",
        "canvas": {"width": 1080, "height": 1920, "fps": 30},
        "sources": [
            {
                "id": "src-1",
                "provider": "youtube",
                "videoId": "bench_test",
                "path": source_path,
                "absoluteStartMs": 0,
                "durationMs": 60000,
                "width": 1920,
                "height": 1080,
                "fps": 60,
            }
        ],
        "timeline": [{"id": "t1", "sourceId": "src-1", "inMs": 0, "outMs": 60000}],
        "layout": {"mode": "fill", "crop": {"x": 0.25, "y": 0.0, "w": 0.5, "h": 1.0}},
        "captions": {"enabled": True},
        "overlays": [],
        "audio": {"gainDb": 0.0, "normalize": {"enabled": True, "targetLufs": -16.0}},
        "export": {"preset": "standard", "audioKbps": 192},
    }

    cmd = build_ffmpeg_args(
        composition=comp,
        encoder=encoder,
        output_path=out_file,
        ffmpeg_exe=FFMPEG_EXE,
        caption_ass_path=caption_ass,
        fonts_dir=FONTS_DIR,
        hwaccel=hwaccel if hwaccel else None,
        progress_pipe=False
    )

    t0 = time.time()
    stderr_log_path = os.path.join(RESULTS_DIR, f"{name}_run{run_idx}_stderr.log")
    with open(stderr_log_path, "w", encoding="utf-8") as stderr_fp:
        proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=stderr_fp)
        h_proc = proc._handle

        # Monitor memory
        peak_ram_mb = 0.0
        pmc = PROCESS_MEMORY_COUNTERS()
        pmc.cb = ctypes.sizeof(PROCESS_MEMORY_COUNTERS)

        while proc.poll() is None:
            if GetProcessMemoryInfo(h_proc, ctypes.byref(pmc), pmc.cb):
                ram_mb = pmc.PeakWorkingSetSize / (1024.0 * 1024.0)
                if ram_mb > peak_ram_mb:
                    peak_ram_mb = ram_mb
            time.sleep(0.05)

    t1 = time.time()
    wall_time = t1 - t0

    # CPU time
    creation_t = FILETIME()
    exit_t = FILETIME()
    kernel_t = FILETIME()
    user_t = FILETIME()
    ctypes.windll.kernel32.GetProcessTimes(
        ctypes.c_void_p(h_proc),
        ctypes.byref(creation_t), ctypes.byref(exit_t),
        ctypes.byref(kernel_t), ctypes.byref(user_t)
    )
    cpu_sec = filetime_to_seconds(kernel_t) + filetime_to_seconds(user_t)
    # CPU % of total system capacity across threads (AMD Ryzen 5600: 12 logical cores)
    cpu_pct = (cpu_sec / wall_time) * 100.0 / 12.0 if wall_time > 0 else 0.0

    if proc.returncode != 0:
        err_snippet = ""
        if os.path.exists(stderr_log_path):
            with open(stderr_log_path, "r", encoding="utf-8", errors="ignore") as f:
                err_snippet = f.read()[-500:]
        return {"error": err_snippet, "success": False}

    file_size_mb = os.path.getsize(out_file) / (1024.0 * 1024.0)
    bitrate_kbps = (file_size_mb * 8192.0) / 60.0
    speed_x = 60.0 / wall_time if wall_time > 0 else 0.0

    return {
        "success": True,
        "run_idx": run_idx,
        "wall_time_s": round(wall_time, 2),
        "speed_x": round(speed_x, 2),
        "cpu_pct": round(cpu_pct, 1),
        "peak_ram_mb": round(peak_ram_mb, 1),
        "file_size_mb": round(file_size_mb, 2),
        "bitrate_kbps": round(bitrate_kbps, 1),
        "output_file": out_file
    }


def execute_suite():
    caption_ass = create_60s_caption()
    print(f"Created 60s benchmark caption: {caption_ass}")

    matrix = [
        # (name, source_type, source_path, encoder, hwaccel)
        ("h264_src_amf_swdec", "H.264 1080p60", SRC_H264, "h264_amf", None),
        ("h264_src_amf_d3d11va", "H.264 1080p60", SRC_H264, "h264_amf", "d3d11va"),
        ("h264_src_mf_swdec", "H.264 1080p60", SRC_H264, "h264_mf", None),
        ("h264_src_mf_d3d11va", "H.264 1080p60", SRC_H264, "h264_mf", "d3d11va"),
        ("vp9_src_amf_swdec", "VP9 1080p", SRC_VP9, "h264_amf", None),
        ("vp9_src_amf_d3d11va", "VP9 1080p", SRC_VP9, "h264_amf", "d3d11va"),
        ("vp9_src_mf_swdec", "VP9 1080p", SRC_VP9, "h264_mf", None),
    ]

    all_results = {}

    for name, src_desc, src_path, enc, hw in matrix:
        print(f"\n=======================================================")
        print(f"Running Benchmark: {name} ({src_desc} | Enc={enc} | HWAcc={hw})")
        print(f"=======================================================")

        runs = []
        for r in range(1, 4):
            print(f"  Run {r}/3 in progress...")
            res = run_single_benchmark(name, src_path, enc, hw, caption_ass, r)
            if res.get("success"):
                runs.append(res)
                print(f"    Done: Wall={res['wall_time_s']}s ({res['speed_x']}x) | CPU={res['cpu_pct']}% | RAM={res['peak_ram_mb']}MB | Size={res['file_size_mb']}MB")
            else:
                print(f"    Failed: {res.get('error')}")

        if runs:
            # Medians
            median_wall = statistics.median([r["wall_time_s"] for r in runs])
            median_speed = statistics.median([r["speed_x"] for r in runs])
            median_cpu = statistics.median([r["cpu_pct"] for r in runs])
            median_ram = statistics.median([r["peak_ram_mb"] for r in runs])
            median_size = statistics.median([r["file_size_mb"] for r in runs])
            median_bitrate = statistics.median([r["bitrate_kbps"] for r in runs])

            # Measure quality of run 1 against reference (h264_src_amf_swdec run 1)
            ref_file = runs[0]["output_file"]
            all_results[name] = {
                "name": name,
                "source_type": src_desc,
                "encoder": enc,
                "hwaccel": hw if hw else "none (software)",
                "runs": runs,
                "median": {
                    "wall_time_s": median_wall,
                    "speed_x": median_speed,
                    "cpu_pct": median_cpu,
                    "peak_ram_mb": median_ram,
                    "file_size_mb": median_size,
                    "bitrate_kbps": median_bitrate,
                }
            }

    # Cross-quality measurement against AMF baseline
    if "h264_src_amf_swdec" in all_results and "h264_src_mf_swdec" in all_results:
        amf_file = all_results["h264_src_amf_swdec"]["runs"][0]["output_file"]
        mf_file = all_results["h264_src_mf_swdec"]["runs"][0]["output_file"]
        q = measure_quality(mf_file, amf_file)
        all_results["h264_src_mf_swdec"]["median"]["quality_vs_amf"] = q
        print(f"\nMF Quality vs AMF Baseline: SSIM={q['ssim']}, PSNR={q['psnr']} dB")

    # Save summary JSON
    summary_path = os.path.join(RESULTS_DIR, "benchmark_60s_summary.json")
    with open(summary_path, "w", encoding="utf-8") as f:
        json.dump(all_results, f, indent=2)

    print(f"\nAll benchmark runs completed! Results saved to {summary_path}")


if __name__ == "__main__":
    execute_suite()
