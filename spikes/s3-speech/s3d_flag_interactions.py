"""
Spike S3d - Step 2: Flag Interactions & Timing Granularity Benchmark
Tests flag combinations on Reference R1:
1. Baseline: -dtw small -nfa -ojf
2. Flash Attn: -dtw small -fa -ojf
3. Split-on-word: -dtw small -nfa -sow -ojf
4. One-word-per-segment: -dtw small -nfa -ml 1 -sow -ojf
5. Quantized DTW argument test: -dtw small vs -dtw small-q5_1
"""
import os
import sys
import json
import time
import numpy as np

sys.path.insert(0, os.path.dirname(__file__))
from benchmark_runner import run_whisper_benchmark
from whisper_to_words import convert_whisper_json_to_words

BASE_DIR = os.path.abspath(os.path.dirname(__file__))
BIN_AVX2 = os.path.join(BASE_DIR, "bin", "whisper-bin-x64", "Release", "whisper-cli.exe")
MODELS_DIR = os.path.join(BASE_DIR, "models")
AUDIO_DIR = os.path.join(BASE_DIR, "audio")
OUT_DIR = os.path.join(BASE_DIR, "results", "s3d_flags")
os.makedirs(OUT_DIR, exist_ok=True)

MODEL_PATH = os.path.join(MODELS_DIR, "ggml-small-q5_1.bin")
AUDIO_PATH = os.path.join(AUDIO_DIR, "podcast_mixed.wav")

CONFIGS = [
    {
        "name": "baseline_nfa",
        "dtw": "small",
        "extra_args": ["-nfa"],
        "desc": "Baseline: -dtw small -nfa"
    },
    {
        "name": "flash_attn_fa",
        "dtw": "small",
        "extra_args": ["-fa"],
        "desc": "Flash attention: -dtw small -fa"
    },
    {
        "name": "split_on_word_sow",
        "dtw": "small",
        "extra_args": ["-nfa", "-sow"],
        "desc": "Split-on-word: -dtw small -nfa -sow"
    },
    {
        "name": "one_word_seg_ml1_sow",
        "dtw": "small",
        "extra_args": ["-nfa", "-ml", "1", "-sow"],
        "desc": "One word per segment: -dtw small -nfa -ml 1 -sow"
    },
]

def run_flag_benchmark():
    print("=" * 80)
    print("SPIKE S3d - STEP 2: FLAG INTERACTIONS BENCHMARK ON R1")
    print(f"Model: small-q5_1 | Audio: podcast_mixed.wav (60.0s)")
    print("=" * 80)

    results = []

    for cfg in CONFIGS:
        out_prefix = os.path.join(OUT_DIR, cfg["name"])
        print(f"\nRunning: {cfg['desc']} ...", flush=True)

        res = run_whisper_benchmark(
            whisper_exe=BIN_AVX2,
            model_path=MODEL_PATH,
            audio_path=AUDIO_PATH,
            output_prefix=out_prefix,
            threads=6,
            language="id",
            vad=False,
            dtw=cfg["dtw"],
            extra_args=cfg["extra_args"],
            audio_duration_s=60.0
        )

        # Parse generated words
        words = []
        if res.get("result_data"):
            words = convert_whisper_json_to_words(res["result_data"])

        # Check whole-second timestamps and duration
        whole_sec = sum(1 for w in words if w["t0"] % 1000 == 0)
        dtw_count = sum(1 for w in words if w.get("timingSource") == "dtw")
        durs = [w["t1"] - w["t0"] for w in words]

        record = {
            "name": cfg["name"],
            "desc": cfg["desc"],
            "wall_time_s": res["wall_time_s"],
            "rtf": res["rtf"],
            "peak_ram_mb": res["peak_ram_mb"],
            "cpu_util_pct": res["cpu_util_pct"],
            "words_count": len(words),
            "dtw_source_count": dtw_count,
            "whole_sec_t0_count": whole_sec,
            "whole_sec_t0_pct": round(whole_sec / max(1, len(words)) * 100, 2),
            "mean_dur_ms": round(float(np.mean(durs)), 1) if durs else 0,
            "median_dur_ms": round(float(np.median(durs)), 1) if durs else 0,
        }
        results.append(record)

        print(f"  Wall Time: {res['wall_time_s']:.2f}s (RTF {res['rtf']:.3f}x) | Peak RAM: {res['peak_ram_mb']:.1f} MB")
        print(f"  Words: {len(words)} (DTW: {dtw_count}) | Whole-sec t0: {whole_sec}/{len(words)} ({record['whole_sec_t0_pct']}%)")
        print(f"  Mean Dur: {record['mean_dur_ms']}ms | Med Dur: {record['median_dur_ms']}ms")

    # Now test what happens if user passes -dtw small-q5_1 instead of -dtw small
    print("\nTesting: -dtw small-q5_1 (quantized name directly) ...", flush=True)
    out_prefix_q = os.path.join(OUT_DIR, "dtw_arg_quantized")
    res_q = run_whisper_benchmark(
        whisper_exe=BIN_AVX2,
        model_path=MODEL_PATH,
        audio_path=AUDIO_PATH,
        output_prefix=out_prefix_q,
        threads=6,
        language="id",
        vad=False,
        dtw="small-q5_1",
        extra_args=["-nfa"],
        audio_duration_s=60.0
    )
    print(f"  Return code: {res_q['returncode']} | Wall Time: {res_q['wall_time_s']}s")
    if res_q['returncode'] != 0 or "error" in res_q.get("stderr", "").lower():
        print(f"  Result: -dtw small-q5_1 FAILED / error: {res_q.get('stderr', '').strip()[:200]}")
    else:
        print(f"  Result: -dtw small-q5_1 accepted! Words: {len(convert_whisper_json_to_words(res_q.get('result_data', {})))}")

    print("\n" + "=" * 80)
    print("FLAG INTERACTIONS SUMMARY TABLE")
    print("=" * 80)
    print(f"{'Config':25} {'RTF':>7} {'WallTime':>9} {'RAM':>8} {'Words':>7} {'DTW%':>7} {'%1s':>7} {'MedDur':>8}")
    print("-" * 84)
    for r in results:
        dtw_pct = round(r['dtw_source_count'] / max(1, r['words_count']) * 100, 1)
        print(f"{r['name']:25} {r['rtf']:7.3f}x {r['wall_time_s']:8.2f}s {r['peak_ram_mb']:7.1f}M {r['words_count']:7d} {dtw_pct:6.1f}% {r['whole_sec_t0_pct']:6.1f}% {r['median_dur_ms']:7.1f}m")

    # Save results
    summary_file = os.path.join(OUT_DIR, "flag_interactions_summary.json")
    with open(summary_file, "w", encoding="utf-8") as f:
        json.dump(results, f, indent=2)
    print(f"\nSaved flag summary to {summary_file}")

if __name__ == "__main__":
    run_flag_benchmark()
