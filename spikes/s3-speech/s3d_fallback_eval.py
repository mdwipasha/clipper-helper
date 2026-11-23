"""
Spike S3d - Step 4: Character-Proportional Fallback Interpolation Evaluation
Measures the error of character-proportional word timing fallback:
1. When DTW is active (true DTW)
2. When DTW is missing (character-proportional interpolation inside segment)
3. Direct comparison between DTW word timestamps and Interpolated timestamps
4. Independent validation against unanchored acoustic onsets
"""
import os
import sys
import json
import wave
import numpy as np

sys.path.insert(0, os.path.dirname(__file__))
from whisper_to_words import convert_whisper_json_to_words, clean_token_text
from s3d_global_onset_validator import detect_global_acoustic_onsets, load_wav_pcm16, match_words_to_onsets_monotonic

BASE_DIR = os.path.abspath(os.path.dirname(__file__))
AUDIO_DIR = os.path.join(BASE_DIR, "audio")
MATRIX_DIR = os.path.join(BASE_DIR, "results", "matrix")
OUT_DIR = os.path.join(BASE_DIR, "results", "s3d_fallback")
os.makedirs(OUT_DIR, exist_ok=True)

SAMPLES = [
    ("gaming_slang", "gaming_slang.wav"),
    ("podcast_mixed", "podcast_mixed.wav"),
    ("gaming_live", "gaming_live.wav"),
]

def generate_interpolated_words(whisper_data):
    """
    Simulates whisper output with DTW missing.
    Forces character-proportional interpolation inside each segment.
    """
    # Create deep copy and strip t_dtw
    import copy
    data_no_dtw = copy.deepcopy(whisper_data)
    transcription = data_no_dtw.get("transcription", [])

    all_words = []
    for seg in transcription:
        seg_from = int(seg.get("offsets", {}).get("from", 0))
        seg_to = int(seg.get("offsets", {}).get("to", seg_from + 1000))
        text = seg.get("text", "").strip()
        tokens = seg.get("tokens", [])

        # Group words from text or tokens
        # If tokens exist, build words from cleaned tokens
        words_in_seg = []
        if tokens:
            curr_w = []
            for tok in tokens:
                raw_text = tok.get("text", "")
                if raw_text.startswith("[_TT_") or raw_text.startswith("<|"):
                    continue
                cleaned = clean_token_text(raw_text)
                if not cleaned:
                    continue
                is_new_word = cleaned.startswith(" ") or cleaned.startswith("\n")
                if is_new_word and curr_w:
                    words_in_seg.append("".join(curr_w).strip())
                    curr_w = [cleaned.lstrip(" \n")]
                else:
                    curr_w.append(cleaned.lstrip(" \n") if not curr_w else cleaned)
            if curr_w:
                words_in_seg.append("".join(curr_w).strip())
        else:
            words_in_seg = text.split()

        words_in_seg = [w for w in words_in_seg if w]
        if not words_in_seg:
            continue

        # Character-proportional allocation
        total_chars = sum(len(w) for w in words_in_seg)
        seg_dur = max(seg_to - seg_from, len(words_in_seg) * 50)
        curr_t = seg_from

        for w_idx, w_text in enumerate(words_in_seg):
            w_len = len(w_text)
            w_dur = max(60, int(seg_dur * (w_len / max(1, total_chars))))
            if w_idx == len(words_in_seg) - 1:
                w_t1 = seg_to
            else:
                w_t1 = min(seg_to, curr_t + w_dur)

            all_words.append({
                "t0": curr_t,
                "t1": max(curr_t + 30, w_t1),
                "text": w_text,
                "timingSource": "interpolated"
            })
            curr_t = w_t1

    return all_words

def run_fallback_evaluation():
    print("=" * 80)
    print("SPIKE S3d - STEP 4: CHARACTER-PROPORTIONAL FALLBACK EVALUATION")
    print("Comparing True DTW vs Fallback Character Interpolation across 3 samples")
    print("=" * 80)

    summary = []

    for sample_id, wav_filename in SAMPLES:
        wav_path = os.path.join(AUDIO_DIR, wav_filename)
        audio, sr = load_wav_pcm16(wav_path)
        global_onsets, _, _ = detect_global_acoustic_onsets(audio, sr)

        json_path = os.path.join(MATRIX_DIR, f"model_small-q5_1_{sample_id}.json")
        with open(json_path, "r", encoding="utf-8") as f:
            whisper_data = json.load(f)

        # 1. True DTW words
        dtw_words = convert_whisper_json_to_words(whisper_data)

        # 2. Simulated Interpolated words
        interp_words = generate_interpolated_words(whisper_data)

        # Direct word-by-word delta between DTW and Interpolation
        # Align words by index if counts match, else zip up to min len
        min_len = min(len(dtw_words), len(interp_words))
        deltas = []
        for i in range(min_len):
            w_dtw = dtw_words[i]
            w_int = interp_words[i]
            delta = abs(w_dtw["t0"] - w_int["t0"])
            deltas.append(delta)

        # Match DTW to acoustic onsets
        dtw_matches, dtw_unmatched = match_words_to_onsets_monotonic(dtw_words, global_onsets, max_tol_s=0.350)
        dtw_errs = [m["abs_err_ms"] for m in dtw_matches if m["abs_err_ms"] is not None]

        # Match Interpolated to acoustic onsets
        int_matches, int_unmatched = match_words_to_onsets_monotonic(interp_words, global_onsets, max_tol_s=0.350)
        int_errs = [m["abs_err_ms"] for m in int_matches if m["abs_err_ms"] is not None]

        rec = {
            "sample_id": sample_id,
            "dtw_words": len(dtw_words),
            "interp_words": len(interp_words),
            "direct_delta_mean_ms": round(float(np.mean(deltas)), 1) if deltas else 0,
            "direct_delta_median_ms": round(float(np.median(deltas)), 1) if deltas else 0,
            "direct_delta_p90_ms": round(float(np.percentile(deltas, 90)), 1) if deltas else 0,
            "direct_delta_max_ms": round(float(np.max(deltas)), 1) if deltas else 0,
            "dtw_vs_acoustic": {
                "matched_count": len(dtw_errs),
                "unmatched_count": dtw_unmatched,
                "mae_ms": round(float(np.mean(dtw_errs)), 1) if dtw_errs else 0,
                "median_err_ms": round(float(np.median(dtw_errs)), 1) if dtw_errs else 0,
                "p90_err_ms": round(float(np.percentile(dtw_errs, 90)), 1) if dtw_errs else 0,
            },
            "interp_vs_acoustic": {
                "matched_count": len(int_errs),
                "unmatched_count": int_unmatched,
                "mae_ms": round(float(np.mean(int_errs)), 1) if int_errs else 0,
                "median_err_ms": round(float(np.median(int_errs)), 1) if int_errs else 0,
                "p90_err_ms": round(float(np.percentile(int_errs, 90)), 1) if int_errs else 0,
            }
        }
        summary.append(rec)

        print(f"\n--- Sample: {sample_id} ---")
        print(f"  Words: DTW={len(dtw_words)}, Interp={len(interp_words)}")
        print(f"  Direct |DTW - Interp|: Mean={rec['direct_delta_mean_ms']}ms | Med={rec['direct_delta_median_ms']}ms | P90={rec['direct_delta_p90_ms']}ms | Max={rec['direct_delta_max_ms']}ms")
        print(f"  DTW vs Acoustic:     Matched={len(dtw_errs)}/{len(dtw_words)} | MAE={rec['dtw_vs_acoustic']['mae_ms']}ms | Med={rec['dtw_vs_acoustic']['median_err_ms']}ms | P90={rec['dtw_vs_acoustic']['p90_err_ms']}ms")
        print(f"  Interp vs Acoustic:  Matched={len(int_errs)}/{len(interp_words)} | MAE={rec['interp_vs_acoustic']['mae_ms']}ms | Med={rec['interp_vs_acoustic']['median_err_ms']}ms | P90={rec['interp_vs_acoustic']['p90_err_ms']}ms")

    # Save summary
    out_file = os.path.join(OUT_DIR, "fallback_eval_summary.json")
    with open(out_file, "w", encoding="utf-8") as f:
        json.dump(summary, f, indent=2)
    print(f"\nSaved fallback evaluation summary to {out_file}")

if __name__ == "__main__":
    run_fallback_evaluation()
