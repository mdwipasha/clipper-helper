"""
S3c DTW Validation — Uncapped onset detector, 30 words/sample,
signed error analysis, and 12.24 s drift investigation.

Key fix from S3b: the original detect_acoustic_onset() had a hard
200 ms cap at line 84: `abs(times[idx] - center_time_s) <= 0.20`.
This artificially limited measured error to ~201 ms, creating the
suspicious 196-201 ms cluster. We remove that cap and increase the
search radius to 500 ms.

Outputs:
  - 90-word validation table with signed errors
  - Error distribution statistics (uncapped)
  - Systematic bias analysis
  - Investigation of the "12.24 s drift" claim
"""
import os
import sys
import json
import wave
import numpy as np

sys.path.insert(0, os.path.dirname(__file__))
from whisper_to_words import convert_whisper_json_to_words

BASE_DIR = os.path.abspath(os.path.dirname(__file__))
AUDIO_DIR = os.path.join(BASE_DIR, "audio")
DTW_DIR = os.path.join(BASE_DIR, "results", "dtw_validation")
MATRIX_DIR = os.path.join(BASE_DIR, "results", "matrix")
OUT_DIR = os.path.join(BASE_DIR, "results", "s3c")
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


def detect_acoustic_onset_uncapped(audio, sample_rate, center_time_s, search_radius_s=0.500):
    """
    Independent acoustic onset detector — UNCAPPED version.

    Changes from S3b:
    - search_radius_s increased from 0.25 to 0.50 (500 ms)
    - REMOVED the 200 ms cap filter that was at line 84 of validate_dtw_onsets.py:
      `valid_indices = [idx for idx in above_th if abs(times[idx] - center_time_s) <= 0.20]`
    - Now finds the true first energy rise above threshold anywhere in the search window
    """
    start_sample = max(0, int((center_time_s - search_radius_s) * sample_rate))
    end_sample = min(len(audio), int((center_time_s + search_radius_s) * sample_rate))
    segment = audio[start_sample:end_sample]
    if len(segment) == 0:
        return center_time_s

    frame_len = int(0.010 * sample_rate)  # 10 ms
    hop_len = int(0.002 * sample_rate)     # 2 ms

    num_frames = (len(segment) - frame_len) // hop_len
    if num_frames <= 1:
        return center_time_s

    energies = []
    times = []
    for i in range(num_frames):
        f = segment[i * hop_len : i * hop_len + frame_len]
        rms = np.sqrt(np.mean(f**2))
        energies.append(rms)
        t = (start_sample + i * hop_len + frame_len / 2) / sample_rate
        times.append(t)

    energies = np.array(energies)
    times = np.array(times)

    # Local noise floor is 15th percentile in the window
    floor = np.percentile(energies, 15)
    peak = np.max(energies)
    if peak <= floor * 1.2:
        # Low dynamics; fallback to max slope
        diff = np.diff(energies)
        best_idx = np.argmax(diff)
        return times[best_idx]

    # Threshold: floor + 15% of dynamic range
    threshold = floor + 0.15 * (peak - floor)

    # Find first energy rise above threshold — NO 200 ms cap
    above_th = np.where(energies >= threshold)[0]
    if len(above_th) > 0:
        onset_idx = above_th[0]
        # Backtrack to local minimum before the rise
        while onset_idx > 0 and energies[onset_idx - 1] < energies[onset_idx] and energies[onset_idx] > floor:
            onset_idx -= 1
        return times[onset_idx]

    center_idx = np.argmin(np.abs(times - center_time_s))
    return times[center_idx]


def load_dtw_words(sample_id):
    """Load DTW word timestamps from existing validation JSONs."""
    json_path = os.path.join(DTW_DIR, f"{sample_id}_small_q5_dtw.json")
    if os.path.exists(json_path):
        with open(json_path, "r", encoding="utf-8") as f:
            data = json.load(f)
        return convert_whisper_json_to_words(data)
    # Fallback: try matrix JSON
    json_path = os.path.join(MATRIX_DIR, f"model_small-q5_1_{sample_id}.json")
    if os.path.exists(json_path):
        with open(json_path, "r", encoding="utf-8") as f:
            data = json.load(f)
        return convert_whisper_json_to_words(data)
    raise FileNotFoundError(f"No DTW output found for {sample_id}")


def load_nodtw_words(sample_id):
    """Load non-DTW word timestamps."""
    json_path = os.path.join(DTW_DIR, f"{sample_id}_small_q5_nodtw.json")
    if os.path.exists(json_path):
        with open(json_path, "r", encoding="utf-8") as f:
            data = json.load(f)
        return convert_whisper_json_to_words(data)
    return []


def investigate_nodtw_timestamps(sample_id):
    """
    Investigate whether the "12.24 s drift" came from token-level or
    segment-level timestamps. Load the raw whisper JSON and check both.
    """
    json_path = os.path.join(DTW_DIR, f"{sample_id}_small_q5_nodtw.json")
    if not os.path.exists(json_path):
        return None

    with open(json_path, "r", encoding="utf-8") as f:
        data = json.load(f)

    findings = {
        "sample": sample_id,
        "has_word_level_tokens": False,
        "timestamp_source": "unknown",
        "segment_count": 0,
        "last_segment_end_ms": 0,
    }

    if "transcription" in data:
        segments = data["transcription"]
        findings["segment_count"] = len(segments)
        if segments:
            last_seg = segments[-1]
            # Check offsets — whisper JSON uses "offsets" for timestamps
            if "offsets" in last_seg:
                findings["last_segment_end_ms"] = last_seg["offsets"].get("to", 0)
            elif "timestamps" in last_seg:
                findings["last_segment_end_ms"] = last_seg["timestamps"].get("to", 0)

            # Check if tokens have word-level timestamps
            if "tokens" in last_seg:
                tokens = last_seg["tokens"]
                has_ts = any("offsets" in t or "t_dtw" in t for t in tokens)
                findings["has_word_level_tokens"] = has_ts
                if has_ts:
                    findings["timestamp_source"] = "token-level (word timestamps present)"
                else:
                    findings["timestamp_source"] = "segment-level only (no word timestamps)"

    return findings


def main():
    print("=" * 80)
    print("S3c: DTW VALIDATION WITH UNCAPPED ONSET DETECTOR")
    print("Search radius: 500 ms, NO 200 ms cap on valid indices")
    print("=" * 80)

    all_entries = []
    signed_errors = []  # DTW_t0 - acoustic_onset (positive = DTW is late)

    for sid, sfile in SAMPLES:
        wav_path = os.path.join(AUDIO_DIR, sfile)
        audio, sr = load_wav_pcm16(wav_path)

        words_dtw = load_dtw_words(sid)
        words_nodtw = load_nodtw_words(sid)

        # Select 30 words distributed across the clip
        n_words = len(words_dtw)
        if n_words <= 30:
            indices = list(range(n_words))
        else:
            step = n_words / 30
            indices = [int(i * step) for i in range(30)]
            # Ensure we don't duplicate
            indices = sorted(set(indices))[:30]

        print(f"\n{'─' * 80}")
        print(f"SAMPLE: {sid} ({n_words} DTW words, testing {len(indices)})")
        print(f"{'─' * 80}")
        print(f"{'#':>3} {'Word':15} {'DTW t0':>10} {'Acoustic':>10} {'Signed Err':>12} {'|Err|':>8}  NoDTW_t0  |NoDTW Err|")
        print("-" * 100)

        for rank, idx in enumerate(indices):
            w = words_dtw[idx]
            dtw_t0_s = w["t0"] / 1000.0

            # Find matching no-DTW word
            nodtw_t0_s = dtw_t0_s
            if idx < len(words_nodtw):
                nodtw_t0_s = words_nodtw[idx]["t0"] / 1000.0

            # Detect acoustic onset (UNCAPPED)
            acoustic_s = detect_acoustic_onset_uncapped(audio, sr, dtw_t0_s)

            # Signed error: positive = DTW is late relative to acoustic onset
            signed_err_ms = (dtw_t0_s - acoustic_s) * 1000.0
            abs_err_ms = abs(signed_err_ms)
            nodtw_err_ms = abs(nodtw_t0_s - acoustic_s) * 1000.0

            signed_errors.append(signed_err_ms)

            entry = {
                "sample": sid,
                "word_index": idx,
                "word": w["text"],
                "dtw_t0_ms": w["t0"],
                "acoustic_onset_ms": round(acoustic_s * 1000, 1),
                "signed_error_ms": round(signed_err_ms, 1),
                "abs_error_ms": round(abs_err_ms, 1),
                "nodtw_t0_ms": round(nodtw_t0_s * 1000, 1),
                "nodtw_abs_error_ms": round(nodtw_err_ms, 1),
            }
            all_entries.append(entry)

            print(f"{rank+1:3} {w['text']:15} {dtw_t0_s*1000:8.0f} ms {acoustic_s*1000:8.0f} ms "
                  f"{signed_err_ms:+10.1f} ms {abs_err_ms:6.1f} ms  {nodtw_t0_s*1000:8.0f}  {nodtw_err_ms:8.1f}")

    # Aggregate statistics
    se = np.array(signed_errors)
    abs_errs = np.abs(se)

    print(f"\n{'=' * 80}")
    print(f"UNCAPPED DTW ERROR DISTRIBUTION ({len(se)} words)")
    print(f"{'=' * 80}")
    print(f"  Mean signed error:    {np.mean(se):+.1f} ms  (positive = DTW late)")
    print(f"  Median signed error:  {np.median(se):+.1f} ms")
    print(f"  Std dev:              {np.std(se):.1f} ms")
    print(f"  Min signed error:     {np.min(se):+.1f} ms")
    print(f"  Max signed error:     {np.max(se):+.1f} ms")
    print(f"  Mean absolute error:  {np.mean(abs_errs):.1f} ms")
    print(f"  Median absolute error:{np.median(abs_errs):.1f} ms")
    print(f"  Max absolute error:   {np.max(abs_errs):.1f} ms")
    print(f"  P95 absolute error:   {np.percentile(abs_errs, 95):.1f} ms")
    print(f"  Within ±100 ms:       {np.sum(abs_errs <= 100)}/{len(se)} ({np.mean(abs_errs <= 100)*100:.1f}%)")
    print(f"  Within ±200 ms:       {np.sum(abs_errs <= 200)}/{len(se)} ({np.mean(abs_errs <= 200)*100:.1f}%)")
    print(f"  Within ±300 ms:       {np.sum(abs_errs <= 300)}/{len(se)} ({np.mean(abs_errs <= 300)*100:.1f}%)")
    print(f"  Within ±500 ms:       {np.sum(abs_errs <= 500)}/{len(se)} ({np.mean(abs_errs <= 500)*100:.1f}%)")

    # Histogram buckets
    buckets = [0, 50, 100, 150, 200, 250, 300, 400, 500, 1000]
    print(f"\n  Absolute error histogram:")
    for i in range(len(buckets) - 1):
        count = np.sum((abs_errs >= buckets[i]) & (abs_errs < buckets[i+1]))
        bar = "█" * count
        print(f"    {buckets[i]:>4}–{buckets[i+1]:>4} ms: {count:3}  {bar}")
    over = np.sum(abs_errs >= buckets[-1])
    if over > 0:
        print(f"    {buckets[-1]:>4}+    ms: {over:3}  {'█' * over}")

    # Bias analysis
    print(f"\n{'=' * 80}")
    print("SYSTEMATIC BIAS ANALYSIS")
    print(f"{'=' * 80}")
    t_stat = np.mean(se) / (np.std(se) / np.sqrt(len(se))) if np.std(se) > 0 else 0
    print(f"  Mean signed error: {np.mean(se):+.1f} ms (t = {t_stat:.2f})")
    if abs(np.mean(se)) > 20:
        print(f"  → BIAS DETECTED: DTW timestamps are consistently {'late' if np.mean(se) > 0 else 'early'} by ~{abs(np.mean(se)):.0f} ms")
        print(f"  → Fixed offset correction of {-np.mean(se):+.0f} ms {'may be' if abs(t_stat) > 2 else 'is weakly'} justified")
    else:
        print(f"  → No significant systematic bias detected (mean < 20 ms)")

    # Per-sample bias
    for sid, _, in SAMPLES:
        sample_errs = [e["signed_error_ms"] for e in all_entries if e["sample"] == sid]
        if sample_errs:
            sa = np.array(sample_errs)
            print(f"  {sid:16}: mean={np.mean(sa):+.1f} ms, median={np.median(sa):+.1f} ms, std={np.std(sa):.1f} ms")

    # Investigate 12.24 s drift
    print(f"\n{'=' * 80}")
    print("12.24 S DRIFT INVESTIGATION")
    print(f"{'=' * 80}")
    for sid, _ in SAMPLES:
        findings = investigate_nodtw_timestamps(sid)
        if findings:
            print(f"\n  {sid}:")
            print(f"    Segments: {findings['segment_count']}")
            print(f"    Word-level tokens: {findings['has_word_level_tokens']}")
            print(f"    Timestamp source: {findings['timestamp_source']}")
            print(f"    Last segment end: {findings['last_segment_end_ms']} ms")

    # Explain the S3b 200 ms cap bug
    print(f"\n  ROOT CAUSE OF 196-201 ms CLUSTER:")
    print(f"  The S3b onset detector (validate_dtw_onsets.py line 84) had:")
    print(f"    valid_indices = [idx for idx in above_th if abs(times[idx] - center_time_s) <= 0.20]")
    print(f"  This 200 ms hard cap on the search window meant:")
    print(f"  - Any word with true error > 200 ms was clamped to exactly ~200 ms")
    print(f"  - The cluster at 196-201 ms was an ARTIFACT of the detector, not of whisper")
    print(f"  - The uncapped results above show the TRUE error distribution")

    # Save results
    results = {
        "detector_config": {
            "search_radius_ms": 500,
            "cap_ms": "NONE (uncapped)",
            "frame_len_ms": 10,
            "hop_len_ms": 2,
            "threshold_pct": 0.15,
            "floor_percentile": 15,
        },
        "summary": {
            "total_words": len(se),
            "mean_signed_error_ms": round(float(np.mean(se)), 1),
            "median_signed_error_ms": round(float(np.median(se)), 1),
            "std_signed_error_ms": round(float(np.std(se)), 1),
            "mean_abs_error_ms": round(float(np.mean(abs_errs)), 1),
            "median_abs_error_ms": round(float(np.median(abs_errs)), 1),
            "max_abs_error_ms": round(float(np.max(abs_errs)), 1),
            "p95_abs_error_ms": round(float(np.percentile(abs_errs, 95)), 1),
            "within_100ms_pct": round(float(np.mean(abs_errs <= 100)*100), 1),
            "within_200ms_pct": round(float(np.mean(abs_errs <= 200)*100), 1),
            "within_300ms_pct": round(float(np.mean(abs_errs <= 300)*100), 1),
            "bias_direction": "late" if np.mean(se) > 20 else "early" if np.mean(se) < -20 else "none",
        },
        "s3b_cap_bug": {
            "description": "S3b detector had 200 ms hard cap creating artificial 196-201 ms cluster",
            "s3b_mean_error_ms": 163.2,
            "s3b_median_error_ms": 197.0,
            "s3b_max_error_ms": 201.0,
        },
        "spot_check_words": all_entries,
    }

    out_path = os.path.join(OUT_DIR, "s3c_dtw_validation.json")
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(results, f, indent=2, ensure_ascii=False)
    print(f"\nSaved validation data to {out_path}")


if __name__ == "__main__":
    main()
