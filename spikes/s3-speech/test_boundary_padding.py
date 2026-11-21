import os
import sys
import json
import subprocess

sys.path.insert(0, os.path.dirname(__file__))
from benchmark_runner import run_whisper_benchmark
from whisper_to_words import convert_whisper_json_to_words
from accuracy_eval import normalize_text_for_eval, levenshtein_distance

BASE_DIR = os.path.abspath("spikes/s3-speech")
BIN_AVX2 = os.path.join(BASE_DIR, "bin", "whisper-bin-x64", "Release", "whisper-cli.exe")
MODELS_DIR = os.path.join(BASE_DIR, "models")
AUDIO_DIR = os.path.join(BASE_DIR, "audio")
MODEL_PATH = os.path.join(MODELS_DIR, "ggml-small-q5_1.bin")
OUT_DIR = os.path.join(BASE_DIR, "results", "boundary_padding")
os.makedirs(OUT_DIR, exist_ok=True)

FFMPEG_EXE = os.path.abspath("spikes/s5-ffmpeg/bin/ffmpeg-release-shared/ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0/bin/ffmpeg.exe")

SAMPLES = [
    ("gaming_slang", "gaming_slang.wav"),
    ("podcast_mixed", "podcast_mixed.wav"),
    ("gaming_live", "gaming_live.wav"),
]

# Target core segment: 20.0s to 40.0s (duration 20.0s)
TARGET_START = 20.0
TARGET_END = 40.0
TARGET_DUR = 20.0

PADDING_OPTIONS = [0, 1, 2, 3] # in seconds

def cut_audio(src_wav, start_s, dur_s, dst_wav):
    cmd = [
        FFMPEG_EXE, "-y",
        "-ss", str(start_s),
        "-t", str(dur_s),
        "-i", src_wav,
        "-ar", "16000",
        "-ac", "1",
        "-c:a", "pcm_s16le",
        dst_wav
    ]
    subprocess.run(cmd, capture_output=True, check=True)

def main():
    print("=================================================================")
    print("SPIKE S3b: BOUNDARY PADDING EXPERIMENT ON REFERENCE R1")
    print(f"Core segment: [{TARGET_START}s - {TARGET_END}s] (20s duration)")
    print(f"Paddings tested: {PADDING_OPTIONS} seconds")
    print("=================================================================")

    all_results = []

    for sid, sfile in SAMPLES:
        src_wav = os.path.join(AUDIO_DIR, sfile)
        print(f"\n--- Processing Sample: {sid} ---")
        
        # 1. Full context reference: transcribe full 60s
        out_full = os.path.join(OUT_DIR, f"{sid}_full_context")
        res_full = run_whisper_benchmark(
            whisper_exe=BIN_AVX2,
            model_path=MODEL_PATH,
            audio_path=src_wav,
            output_prefix=out_full,
            threads=6,
            language="id",
            vad=False,
            dtw="small",
            extra_args=["-nfa"],
        )
        full_words = convert_whisper_json_to_words(res_full["result_data"]) if res_full["result_data"] else []
        
        # Extract words belonging to target segment [TARGET_START, TARGET_END]
        # in source time (ms)
        target_words_full = [
            w for w in full_words
            if (w["t0"] >= int(TARGET_START * 1000) - 200 and w["t1"] <= int(TARGET_END * 1000) + 200)
        ]
        
        if len(target_words_full) < 6:
            print(f"Warning: only {len(target_words_full)} words in target segment for {sid}")
            first_3_ref = [w["text"] for w in target_words_full[:min(3, len(target_words_full))]]
            last_3_ref = [w["text"] for w in target_words_full[-min(3, len(target_words_full)):]]
        else:
            first_3_ref = [w["text"] for w in target_words_full[:3]]
            last_3_ref = [w["text"] for w in target_words_full[-3:]]

        print(f"Full-context first 3 words in [{TARGET_START}s-{TARGET_END}s]: {first_3_ref}")
        print(f"Full-context last 3 words in  [{TARGET_START}s-{TARGET_END}s]: {last_3_ref}")

        for pad in PADDING_OPTIONS:
            cut_start = max(0.0, TARGET_START - pad)
            cut_end = TARGET_END + pad
            cut_dur = cut_end - cut_start
            
            cut_wav = os.path.join(OUT_DIR, f"{sid}_pad_{pad}s.wav")
            cut_audio(src_wav, cut_start, cut_dur, cut_wav)

            out_cut = os.path.join(OUT_DIR, f"{sid}_pad_{pad}s")
            res_cut = run_whisper_benchmark(
                whisper_exe=BIN_AVX2,
                model_path=MODEL_PATH,
                audio_path=cut_wav,
                output_prefix=out_cut,
                threads=6,
                language="id",
                vad=False,
                dtw="small",
                extra_args=["-nfa"],
                audio_duration_s=cut_dur,
            )
            cut_words_raw = convert_whisper_json_to_words(res_cut["result_data"]) if res_cut["result_data"] else []
            
            # Map words back to source timeline by adding cut_start * 1000 ms
            mapped_words = []
            for w in cut_words_raw:
                mapped_words.append({
                    "t0": w["t0"] + int(cut_start * 1000),
                    "t1": w["t1"] + int(cut_start * 1000),
                    "text": w["text"]
                })

            # Filter to target [TARGET_START, TARGET_END]
            target_words_cut = [
                w for w in mapped_words
                if (w["t0"] >= int(TARGET_START * 1000) - 250 and w["t1"] <= int(TARGET_END * 1000) + 250)
            ]

            first_3_cut = [w["text"] for w in target_words_cut[:min(3, len(target_words_cut))]]
            last_3_cut = [w["text"] for w in target_words_cut[-min(3, len(target_words_cut)):]] if len(target_words_cut) >= 3 else []

            # Compute error count (Levenshtein distance) on normalized boundary words
            norm_ref_first = [normalize_text_for_eval(w) for w in first_3_ref]
            norm_cut_first = [normalize_text_for_eval(w) for w in first_3_cut]
            first_err = levenshtein_distance(norm_ref_first, norm_cut_first)

            norm_ref_last = [normalize_text_for_eval(w) for w in last_3_ref]
            norm_cut_last = [normalize_text_for_eval(w) for w in last_3_cut]
            last_err = levenshtein_distance(norm_ref_last, norm_cut_last)

            total_boundary_err = first_err + last_err

            record = {
                "sample": sid,
                "padding_s": pad,
                "cut_start_s": cut_start,
                "cut_dur_s": cut_dur,
                "first_3_ref": first_3_ref,
                "first_3_hyp": first_3_cut,
                "first_3_errors": first_err,
                "last_3_ref": last_3_ref,
                "last_3_hyp": last_3_cut,
                "last_3_errors": last_err,
                "total_boundary_errors": total_boundary_err,
                "words_in_segment": len(target_words_cut),
                "wall_time_s": res_cut["wall_time_s"],
                "rtf": res_cut["rtf"],
            }
            all_results.append(record)
            print(f"Pad {pad}s: First: {first_3_cut} (err={first_err}) | Last: {last_3_cut} (err={last_err}) | Total boundary errors: {total_boundary_err}")

    # Summary table by padding across all samples
    print("\n=================================================================")
    print("SUMMARY: BOUNDARY ERRORS VS CONTEXT PADDING (3 SAMPLES)")
    print("=================================================================")
    for pad in PADDING_OPTIONS:
        pad_records = [r for r in all_results if r["padding_s"] == pad]
        tot_err = sum(r["total_boundary_errors"] for r in pad_records)
        first_errs = sum(r["first_3_errors"] for r in pad_records)
        last_errs = sum(r["last_3_errors"] for r in pad_records)
        print(f"Padding {pad}s | Total Boundary Errors: {tot_err:2} (First 3: {first_errs:2}, Last 3: {last_errs:2})")

    out_file = os.path.join(OUT_DIR, "boundary_padding_summary.json")
    with open(out_file, "w", encoding="utf-8") as f:
        json.dump(all_results, f, indent=2, ensure_ascii=False)
    print(f"\nSaved boundary padding experiment data to: {out_file}")

if __name__ == "__main__":
    main()
