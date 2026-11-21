import os
import sys
import json
import time
import subprocess

sys.path.insert(0, os.path.dirname(__file__))
from benchmark_runner import run_whisper_benchmark
from whisper_to_words import convert_whisper_json_to_words
from accuracy_eval import normalize_text_for_eval

BASE_DIR = os.path.abspath("spikes/s3-speech")
BIN_AVX2 = os.path.join(BASE_DIR, "bin", "whisper-bin-x64", "Release", "whisper-cli.exe")
MODELS_DIR = os.path.join(BASE_DIR, "models")
AUDIO_DIR = os.path.join(BASE_DIR, "audio")
VAD_MODEL = os.path.join(MODELS_DIR, "ggml-silero-v6.2.0.bin")
MODEL_SMALL = os.path.join(MODELS_DIR, "ggml-small-q5_1.bin")
OUT_DIR = os.path.join(BASE_DIR, "results", "vad_tuning")
os.makedirs(OUT_DIR, exist_ok=True)

HALLUCINATION_BLOCKLIST = [
    "sampai jumpa di video selanjutnya",
    "sampai jumpa di video berikutnya",
    "sampai jumpa",
    "terima kasih telah menonton",
    "terima kasih sudah menonton",
    "like comment subscribe",
    "jangan lupa subscribe",
    "see you in the next video",
    "thank you for watching",
]

def check_hallucination(text):
    norm = normalize_text_for_eval(text)
    for phrase in HALLUCINATION_BLOCKLIST:
        if phrase in norm:
            return True, phrase
    return False, None

def main():
    print("=================================================================")
    print("SPIKE S3b: VAD TUNING & HALLUCINATION MITIGATION ON REFERENCE R1")
    print("=================================================================")

    results = []

    # 1. Investigate gaming_slang VAD slowdown
    print("\n--- Experiment 1: gaming_slang VAD Behavior & Thresholds ---")
    slang_audio = os.path.join(AUDIO_DIR, "gaming_slang.wav")

    configs = [
        ("vad_off", False, None, []),
        ("vad_on_th0.50", True, VAD_MODEL, ["-vt", "0.50"]),
        ("vad_on_th0.35", True, VAD_MODEL, ["-vt", "0.35"]),
    ]

    for label, vad_flag, vad_mod, extra in configs:
        out_prefix = os.path.join(OUT_DIR, f"slang_{label}")
        print(f"Running gaming_slang ({label})...", end="", flush=True)
        res = run_whisper_benchmark(
            whisper_exe=BIN_AVX2,
            model_path=MODEL_SMALL,
            audio_path=slang_audio,
            output_prefix=out_prefix,
            threads=6,
            language="id",
            vad=vad_flag,
            vad_model=vad_mod,
            dtw="small",
            extra_args=["-nfa"] + extra,
        )
        words = convert_whisper_json_to_words(res["result_data"]) if res["result_data"] else []
        text = " ".join(w["text"] for w in words)
        
        # Count segments from raw json
        seg_count = len(res["result_data"].get("transcription", [])) if res["result_data"] else 0

        rec = {
            "sample": "gaming_slang",
            "config": label,
            "wall_time_s": res["wall_time_s"],
            "rtf": res["rtf"],
            "peak_ram_mb": res["peak_ram_mb"],
            "segments_count": seg_count,
            "words_count": len(words),
            "text_snippet": text[:120] + "...",
        }
        results.append(rec)
        print(f" RTF: {res['rtf']}x | Time: {res['wall_time_s']}s | Segments: {seg_count} | Words: {len(words)}")

    # 2. Investigate gaming_live trailing hallucination and alternatives
    print("\n--- Experiment 2: gaming_live Trailing Hallucination & Alternatives ---")
    live_audio = os.path.join(AUDIO_DIR, "gaming_live.wav")

    # Alternative A: Trim trailing silence with FFmpeg
    # Find silence or trim last 2 seconds where speech has ended
    trimmed_audio = os.path.join(OUT_DIR, "gaming_live_trimmed_silence.wav")
    S5_FFMPEG = os.path.abspath("spikes/s5-ffmpeg/bin/ffmpeg-release-shared/ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0/bin/ffmpeg.exe")
    # Trim to 58.0 seconds (remove trailing 2s silence)
    subprocess.run([
        S5_FFMPEG, "-y", "-i", live_audio, "-t", "58.0", "-c", "copy", trimmed_audio
    ], capture_output=True)

    alt_configs = [
        ("baseline_vad_off", live_audio, False, None, []),
        ("vad_on_th0.50", live_audio, True, VAD_MODEL, ["-vt", "0.50"]),
        ("vad_on_th0.35", live_audio, True, VAD_MODEL, ["-vt", "0.35"]),
        ("alt_trimmed_trailing_silence", trimmed_audio, False, None, []),
        ("alt_sns_alone", live_audio, False, None, ["-sns"]),
    ]

    for label, aud_path, vad_flag, vad_mod, extra in alt_configs:
        out_prefix = os.path.join(OUT_DIR, f"live_{label}")
        print(f"Running gaming_live ({label})...", end="", flush=True)
        res = run_whisper_benchmark(
            whisper_exe=BIN_AVX2,
            model_path=MODEL_SMALL,
            audio_path=aud_path,
            output_prefix=out_prefix,
            threads=6,
            language="id",
            vad=vad_flag,
            vad_model=vad_mod,
            dtw="small",
            extra_args=["-nfa"] + extra,
        )
        words = convert_whisper_json_to_words(res["result_data"]) if res["result_data"] else []
        text = " ".join(w["text"] for w in words)
        
        has_halluc, matched_phrase = check_hallucination(text)

        # Apply blocklist filter test
        filtered_words = []
        for w in words:
            # check if word is part of hallucination phrase
            filtered_words.append(w)
        
        rec = {
            "sample": "gaming_live",
            "config": label,
            "wall_time_s": res["wall_time_s"],
            "rtf": res["rtf"],
            "peak_ram_mb": res["peak_ram_mb"],
            "words_count": len(words),
            "hallucination_detected": has_halluc,
            "matched_phrase": matched_phrase,
            "tail_text": text[-100:] if len(text) > 100 else text,
        }
        results.append(rec)
        halluc_str = f" [HALLUCINATION: '{matched_phrase}']" if has_halluc else " [CLEAN - NO HALLUCINATION]"
        print(f" RTF: {res['rtf']}x | Time: {res['wall_time_s']}s | Words: {len(words)}{halluc_str}")

    # Save summary
    out_json = os.path.join(OUT_DIR, "vad_tuning_summary.json")
    with open(out_json, "w", encoding="utf-8") as f:
        json.dump(results, f, indent=2, ensure_ascii=False)
    print(f"\nSaved VAD tuning results to {out_json}")

if __name__ == "__main__":
    main()
