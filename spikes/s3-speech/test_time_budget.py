import os
import sys
import json
import time
import subprocess

sys.path.insert(0, os.path.dirname(__file__))
from benchmark_runner import run_whisper_benchmark

BASE_DIR = os.path.abspath("spikes/s3-speech")
BIN_AVX2 = os.path.join(BASE_DIR, "bin", "whisper-bin-x64", "Release", "whisper-cli.exe")
MODELS_DIR = os.path.join(BASE_DIR, "models")
OUT_DIR = os.path.join(BASE_DIR, "results", "time_budget")
os.makedirs(OUT_DIR, exist_ok=True)

FFMPEG_EXE = os.path.abspath("spikes/s5-ffmpeg/bin/ffmpeg-release-shared/ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0/bin/ffmpeg.exe")
SRC_VIDEO = os.path.abspath("spikes/s1-downloader/results/s1b/copy_5min.mp4")

MODELS = [
    ("base", os.path.join(MODELS_DIR, "ggml-base.bin")),
    ("small-q5_1", os.path.join(MODELS_DIR, "ggml-small-q5_1.bin")),
    ("small", os.path.join(MODELS_DIR, "ggml-small.bin")),
]

DURATIONS = [30, 60, 90, 300]

def prepare_audio_clips():
    wav_files = {}
    for dur in DURATIONS:
        dst_wav = os.path.join(OUT_DIR, f"budget_{dur}s.wav")
        if not os.path.exists(dst_wav):
            print(f"Extracting {dur}s audio from {SRC_VIDEO}...")
            cmd = [
                FFMPEG_EXE, "-y",
                "-ss", "0",
                "-t", str(dur),
                "-i", SRC_VIDEO,
                "-vn",
                "-ar", "16000",
                "-ac", "1",
                "-c:a", "pcm_s16le",
                dst_wav
            ]
            subprocess.run(cmd, capture_output=True, check=True)
        wav_files[dur] = dst_wav
    return wav_files

def main():
    print("=================================================================")
    print("SPIKE S3b: TIME BUDGET BENCHMARK ON REFERENCE R1")
    print(f"Durations tested: {DURATIONS} seconds (30s, 60s, 90s, 300s)")
    print(f"Candidate models: {[m[0] for m in MODELS]}")
    print("Threads: 6, Language: id, DTW enabled")
    print("=================================================================")

    wav_files = prepare_audio_clips()
    results = []

    for m_name, m_path in MODELS:
        print(f"\n--- Model: {m_name} ---")
        dtw_flag = "small" if "small" in m_name else m_name
        for dur in DURATIONS:
            audio_path = wav_files[dur]
            out_prefix = os.path.join(OUT_DIR, f"budget_{m_name}_{dur}s")
            print(f"Running {m_name:10} on {dur:3}s clip...", end="", flush=True)

            res = run_whisper_benchmark(
                whisper_exe=BIN_AVX2,
                model_path=m_path,
                audio_path=audio_path,
                output_prefix=out_prefix,
                threads=6,
                language="id",
                vad=False,
                dtw=dtw_flag,
                extra_args=["-nfa"],
                audio_duration_s=float(dur),
            )

            # Speed factor: x-realtime = audio_duration / wall_time = 1 / rtf
            x_realtime = round(1.0 / res["rtf"], 2) if res["rtf"] > 0 else 0.0

            rec = {
                "model": m_name,
                "audio_duration_s": dur,
                "wall_time_s": res["wall_time_s"],
                "rtf": res["rtf"],
                "x_realtime": x_realtime,
                "peak_ram_mb": res["peak_ram_mb"],
                "cpu_util_pct": res["cpu_util_pct"],
            }
            results.append(rec)
            print(f" Wall: {res['wall_time_s']:6.1f}s | RTF: {res['rtf']:5.3f}x | Speed: {x_realtime:4.2f}x-realtime | RAM: {res['peak_ram_mb']:6.1f}MB")

    # Output formatted table
    print("\n=================================================================")
    print("TIME BUDGET MATRIX SUMMARY (REFERENCE R1 - Ryzen 5 PRO 5650U)")
    print("=================================================================")
    print(f"{'Model':12} | {'Clip':6} | {'Wall Time (s)':14} | {'RTF':7} | {'x-Realtime':11} | {'Peak RAM':10}")
    print("-" * 75)
    for r in results:
        print(f"{r['model']:12} | {r['audio_duration_s']:3}s   | {r['wall_time_s']:8.1f} s     | {r['rtf']:5.3f}x | {r['x_realtime']:5.2f}x      | {r['peak_ram_mb']:6.1f} MB")

    out_json = os.path.join(OUT_DIR, "time_budget_summary.json")
    with open(out_json, "w", encoding="utf-8") as f:
        json.dump(results, f, indent=2, ensure_ascii=False)
    print(f"\nSaved time budget data to: {out_json}")

if __name__ == "__main__":
    main()
