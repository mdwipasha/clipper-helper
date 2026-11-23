"""
Spike S3d - Step 3: Unanchored Global Acoustic Onset Validator
Performs global onset detection over the entire 60s WAV file (not windowed around t0),
then matches words to onsets monotonically within a bounded tolerance window.
Computes unanchored error distributions and bootstrap confidence intervals.
"""
import os
import sys
import wave
import json
import numpy as np

def find_peaks_numpy(x, height=None, distance=None, prominence=None):
    """Pure numpy peak detector without scipy dependency."""
    if len(x) < 3:
        return np.array([], dtype=int)
    
    # Candidate local maxima: x[i] > x[i-1] and x[i] >= x[i+1]
    dx = np.diff(x)
    peaks = np.where((dx[:-1] > 0) & (dx[1:] <= 0))[0] + 1
    
    if height is not None and len(peaks) > 0:
        peaks = peaks[x[peaks] >= height]
        
    if prominence is not None and len(peaks) > 0:
        w = 50
        prom_peaks = []
        for p in peaks:
            left_min = np.min(x[max(0, p - w) : p + 1])
            right_min = np.min(x[p : min(len(x), p + w + 1)])
            prom = x[p] - max(left_min, right_min)
            if prom >= prominence:
                prom_peaks.append(p)
        peaks = np.array(prom_peaks, dtype=int)
        
    if distance is not None and len(peaks) > 1:
        keep = []
        for p in peaks:
            if not keep:
                keep.append(p)
            elif p - keep[-1] >= distance:
                keep.append(p)
            else:
                if x[p] > x[keep[-1]]:
                    keep[-1] = p
        peaks = np.array(keep, dtype=int)
        
    return peaks

sys.path.insert(0, os.path.dirname(__file__))
from whisper_to_words import convert_whisper_json_to_words

BASE_DIR = os.path.abspath(os.path.dirname(__file__))
AUDIO_DIR = os.path.join(BASE_DIR, "audio")
MATRIX_DIR = os.path.join(BASE_DIR, "results", "matrix")
OUT_DIR = os.path.join(BASE_DIR, "results", "s3d_validation")
os.makedirs(OUT_DIR, exist_ok=True)

SAMPLES = [
    ("gaming_slang", "gaming_slang.wav"),
    ("podcast_mixed", "podcast_mixed.wav"),
    ("gaming_live", "gaming_live.wav"),
]

def load_wav_pcm16(wav_path):
    with wave.open(wav_path, "rb") as wf:
        n_frames = wf.getnframes()
        framerate = wf.getframerate()
        data = wf.readframes(n_frames)
        samples = np.frombuffer(data, dtype=np.int16).astype(np.float32) / 32768.0
        return samples, framerate

def detect_global_acoustic_onsets(audio, sr, hop_ms=2.0, frame_ms=10.0):
    """
    Computes global onset envelope across the ENTIRE clip without any windowing around t0.
    Returns array of onset timestamps in seconds.
    """
    frame_len = int((frame_ms / 1000.0) * sr)
    hop_len = int((hop_ms / 1000.0) * sr)
    n_frames = (len(audio) - frame_len) // hop_len

    times = np.array([(i * hop_len + frame_len / 2) / sr for i in range(n_frames)])
    energies = np.zeros(n_frames, dtype=np.float32)

    for i in range(n_frames):
        chunk = audio[i * hop_len : i * hop_len + frame_len]
        energies[i] = np.sqrt(np.mean(chunk**2))

    # Log-energy novelty curve (half-wave rectified difference of log energy)
    log_energy = np.log1p(energies * 100.0)
    diff = np.diff(log_energy, prepend=log_energy[0])
    onset_env = np.maximum(0, diff)

    # Smooth onset envelope with a 5-point moving average
    kernel = np.ones(5) / 5.0
    onset_env_smooth = np.convolve(onset_env, kernel, mode="same")

    # Dynamic peak picking: threshold at 25th percentile + 0.15 * (max - 25th)
    p25 = np.percentile(onset_env_smooth, 25)
    p_max = np.max(onset_env_smooth)
    height_th = p25 + 0.10 * (p_max - p25)

    # Distance: at least 40 ms between consecutive onsets (20 hops @ 2ms)
    min_dist_frames = int(0.040 / (hop_ms / 1000.0))
    peak_indices = find_peaks_numpy(
        onset_env_smooth,
        height=height_th,
        distance=min_dist_frames,
        prominence=0.05 * (p_max - p25)
    )

    onset_times = times[peak_indices]
    return onset_times, onset_env_smooth, times

def match_words_to_onsets_monotonic(words, global_onsets, max_tol_s=0.350):
    """
    Matches words to unanchored global onsets with order constraint (monotonic).
    Returns list of matches and unmatched count.
    """
    matches = []
    unmatched_count = 0
    last_onset_idx = -1

    for w_idx, w in enumerate(words):
        w_t0_s = w["t0"] / 1000.0

        # Valid candidate onsets: index > last_onset_idx and within max_tol_s
        candidates = []
        for o_idx in range(last_onset_idx + 1, len(global_onsets)):
            o_time = global_onsets[o_idx]
            delta_s = w_t0_s - o_time  # positive = DTW is late
            if abs(delta_s) <= max_tol_s:
                candidates.append((o_idx, o_time, delta_s))
            elif o_time > w_t0_s + max_tol_s:
                # Onsets are strictly increasing; can stop scanning
                break

        if candidates:
            # Pick candidate with minimum absolute delta
            best = min(candidates, key=lambda c: abs(c[2]))
            last_onset_idx = best[0]
            matches.append({
                "word_idx": w_idx,
                "word": w["text"],
                "dtw_t0_ms": w["t0"],
                "dtw_t0_s": w_t0_s,
                "onset_s": round(float(best[1]), 4),
                "onset_ms": round(float(best[1] * 1000), 1),
                "signed_err_ms": round(float(best[2] * 1000), 1),
                "abs_err_ms": round(float(abs(best[2]) * 1000), 1),
                "timingSource": w.get("timingSource", "dtw")
            })
        else:
            unmatched_count += 1
            matches.append({
                "word_idx": w_idx,
                "word": w["text"],
                "dtw_t0_ms": w["t0"],
                "dtw_t0_s": w_t0_s,
                "onset_s": None,
                "onset_ms": None,
                "signed_err_ms": None,
                "abs_err_ms": None,
                "timingSource": w.get("timingSource", "dtw"),
                "status": "unmatched"
            })

    return matches, unmatched_count

def bootstrap_ci(data, n_boot=2000, ci=95):
    """Computes bootstrap confidence interval for the mean."""
    if len(data) == 0:
        return (0.0, 0.0)
    boot_means = []
    n = len(data)
    for _ in range(n_boot):
        sample = np.random.choice(data, size=n, replace=True)
        boot_means.append(np.mean(sample))
    lower = np.percentile(boot_means, (100 - ci) / 2.0)
    upper = np.percentile(boot_means, 100 - (100 - ci) / 2.0)
    return round(float(lower), 1), round(float(upper), 1)

def evaluate_sample(sample_id, wav_filename, model_name="small-q5_1"):
    wav_path = os.path.join(AUDIO_DIR, wav_filename)
    audio, sr = load_wav_pcm16(wav_path)

    # 1. Detect global onsets across entire 60s
    global_onsets, _, _ = detect_global_acoustic_onsets(audio, sr)

    # 2. Load whisper words (using fixed converter)
    json_path = os.path.join(MATRIX_DIR, f"model_{model_name}_{sample_id}.json")
    with open(json_path, "r", encoding="utf-8") as f:
        whisper_data = json.load(f)
    words = convert_whisper_json_to_words(whisper_data)

    # 3. Match words monotonically
    matches, unmatched = match_words_to_onsets_monotonic(words, global_onsets, max_tol_s=0.350)
    matched_entries = [m for m in matches if m["signed_err_ms"] is not None]

    signed_errs = [m["signed_err_ms"] for m in matched_entries]
    abs_errs = [m["abs_err_ms"] for m in matched_entries]

    ci_low, ci_high = bootstrap_ci(signed_errs)

    return {
        "sample": sample_id,
        "model": model_name,
        "clip_duration_s": round(len(audio) / sr, 1),
        "total_global_onsets": len(global_onsets),
        "total_words": len(words),
        "matched_words": len(matched_entries),
        "unmatched_words": unmatched,
        "match_rate_pct": round(len(matched_entries) / max(1, len(words)) * 100, 1),
        "mean_signed_bias_ms": round(float(np.mean(signed_errs)), 1) if signed_errs else 0,
        "median_signed_bias_ms": round(float(np.median(signed_errs)), 1) if signed_errs else 0,
        "bias_ci_95": [ci_low, ci_high],
        "std_ms": round(float(np.std(signed_errs)), 1) if signed_errs else 0,
        "mean_abs_err_ms": round(float(np.mean(abs_errs)), 1) if abs_errs else 0,
        "median_abs_err_ms": round(float(np.median(abs_errs)), 1) if abs_errs else 0,
        "p95_abs_err_ms": round(float(np.percentile(abs_errs, 95)), 1) if abs_errs else 0,
        "max_abs_err_ms": round(float(np.max(abs_errs)), 1) if abs_errs else 0,
        "matches": matches,
    }

def main():
    print("=" * 80)
    print("SPIKE S3d - STEP 3: UNANCHORED GLOBAL ACOUSTIC ONSET VALIDATION")
    print("Detection across ENTIRE clip without t0 windowing; monotonic matching")
    print("=" * 80)

    all_results = []
    all_signed = []
    all_abs = []

    for sid, sfile in SAMPLES:
        res = evaluate_sample(sid, sfile, model_name="small-q5_1")
        all_results.append(res)
        matched = [m for m in res["matches"] if m["signed_err_ms"] is not None]
        all_signed.extend([m["signed_err_ms"] for m in matched])
        all_abs.extend([m["abs_err_ms"] for m in matched])

        print(f"\n[{sid}]")
        print(f"  Global Onsets Detected: {res['total_global_onsets']} across {res['clip_duration_s']}s")
        print(f"  Words: {res['total_words']} | Matched: {res['matched_words']}/{res['total_words']} ({res['match_rate_pct']}%) | Unmatched: {res['unmatched_words']}")
        print(f"  Mean Signed Bias:  {res['mean_signed_bias_ms']:+6.1f} ms (95% CI: [{res['bias_ci_95'][0]:+0.1f}, {res['bias_ci_95'][1]:+0.1f}] ms)")
        print(f"  Median Signed Bias:{res['median_signed_bias_ms']:+6.1f} ms | Std: {res['std_ms']:.1f} ms")
        print(f"  Mean Absolute Err: {res['mean_abs_err_ms']:6.1f} ms | Median: {res['median_abs_err_ms']:.1f} ms | P95: {res['p95_abs_err_ms']:.1f} ms | Max: {res['max_abs_err_ms']:.1f} ms")

    # Aggregate stats across all 3 samples
    ci_low, ci_high = bootstrap_ci(all_signed)
    print("\n" + "=" * 80)
    print("AGGREGATE UNANCHORED VALIDATION SUMMARY (small-q5_1 across 3 samples)")
    print("=" * 80)
    print(f"Total Words Tested:           {len(all_signed) + sum(r['unmatched_words'] for r in all_results)}")
    print(f"Matched Words:                {len(all_signed)} ({len(all_signed)/(len(all_signed)+sum(r['unmatched_words'] for r in all_results))*100:.1f}%)")
    print(f"Mean Signed Bias:             {np.mean(all_signed):+6.1f} ms (95% CI: [{ci_low:+0.1f}, {ci_high:+0.1f}] ms)")
    print(f"Median Signed Bias:           {np.median(all_signed):+6.1f} ms")
    print(f"Std Dev:                      {np.std(all_signed):.1f} ms")
    print(f"Mean Absolute Error:          {np.mean(all_abs):.1f} ms")
    print(f"Median Absolute Error:        {np.median(all_abs):.1f} ms")
    print(f"P95 Absolute Error:           {np.percentile(all_abs, 95):.1f} ms")
    print(f"Max Absolute Error:           {np.max(all_abs):.1f} ms")

    # Save summary
    out_file = os.path.join(OUT_DIR, "global_onset_validation_summary.json")
    with open(out_file, "w", encoding="utf-8") as f:
        json.dump({
            "aggregate": {
                "matched_words": len(all_signed),
                "mean_signed_bias_ms": round(float(np.mean(all_signed)), 1),
                "bias_ci_95": [ci_low, ci_high],
                "median_signed_bias_ms": round(float(np.median(all_signed)), 1),
                "std_ms": round(float(np.std(all_signed)), 1),
                "mean_abs_err_ms": round(float(np.mean(all_abs)), 1),
                "median_abs_err_ms": round(float(np.median(all_abs)), 1),
                "p95_abs_err_ms": round(float(np.percentile(all_abs, 95)), 1),
                "max_abs_err_ms": round(float(np.max(all_abs)), 1),
            },
            "per_sample": all_results
        }, f, indent=2, ensure_ascii=False)
    print(f"\nSaved global validation results to {out_file}")

if __name__ == "__main__":
    main()
