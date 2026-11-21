import os
import sys
import wave
import json
import numpy as np

sys.path.insert(0, os.path.dirname(__file__))
from benchmark_runner import run_whisper_benchmark
from whisper_to_words import convert_whisper_json_to_words

BASE_DIR = os.path.abspath("spikes/s3-speech")
BIN_AVX2 = os.path.join(BASE_DIR, "bin", "whisper-bin-x64", "Release", "whisper-cli.exe")
MODELS_DIR = os.path.join(BASE_DIR, "models")
AUDIO_DIR = os.path.join(BASE_DIR, "audio")
OUT_DIR = os.path.join(BASE_DIR, "results", "dtw_validation")
os.makedirs(OUT_DIR, exist_ok=True)

SAMPLES = [
    ("gaming_slang", "gaming_slang.wav"),
    ("podcast_mixed", "podcast_mixed.wav"),
    ("gaming_live", "gaming_live.wav"),
]

def load_wav_pcm16(wav_path):
    with wave.open(wav_path, "rb") as wf:
        n_channels = wf.getnchannels()
        sampwidth = wf.getsampwidth()
        framerate = wf.getframerate()
        n_frames = wf.getnframes()
        data = wf.readframes(n_frames)
        samples = np.frombuffer(data, dtype=np.int16).astype(np.float32) / 32768.0
        return samples, framerate

def detect_acoustic_onset(audio, sample_rate, center_time_s, search_radius_s=0.25):
    """
    Independent acoustic onset detector:
    Uses short-time RMS energy and energy derivative (smoothed) in a window around center_time_s.
    Identifies the onset point where acoustic energy jumps above local silence/floor.
    """
    start_sample = max(0, int((center_time_s - search_radius_s) * sample_rate))
    end_sample = min(len(audio), int((center_time_s + search_radius_s) * sample_rate))
    segment = audio[start_sample:end_sample]
    if len(segment) == 0:
        return center_time_s

    frame_len = int(0.010 * sample_rate) # 10 ms
    hop_len = int(0.002 * sample_rate)   # 2 ms

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

    # Local noise floor is 10th percentile in the window
    floor = np.percentile(energies, 15)
    peak = np.max(energies)
    if peak <= floor * 1.2:
        # Low dynamics; fallback to max slope
        diff = np.diff(energies)
        best_idx = np.argmax(diff)
        return times[best_idx]

    # Threshold for speech onset: floor + 15% of dynamic range
    threshold = floor + 0.15 * (peak - floor)
    
    # Target index closest to center_time_s
    center_idx = np.argmin(np.abs(times - center_time_s))
    
    # Search backwards from peak or forward from start
    above_th = np.where(energies >= threshold)[0]
    if len(above_th) > 0:
        # Find first rise near center
        valid_indices = [idx for idx in above_th if abs(times[idx] - center_time_s) <= 0.20]
        if valid_indices:
            # Look for max energy rise immediately before the first crossing
            onset_idx = valid_indices[0]
            # backtrack to local minimum
            while onset_idx > 0 and energies[onset_idx - 1] < energies[onset_idx] and energies[onset_idx] > floor:
                onset_idx -= 1
            return times[onset_idx]
        else:
            return times[above_th[0]]

    return times[center_idx]

def main():
    print("=================================================================")
    print("SPIKE S3b: INDEPENDENT DTW ONSET VALIDATION ON REFERENCE R1")
    print("Testing default quantized model: small-q5_1 (and unquantized small)")
    print("=================================================================")

    model_q5 = os.path.join(MODELS_DIR, "ggml-small-q5_1.bin")
    
    total_samples_tested = 0
    dtw_errors_ms = []
    nodtw_errors_ms = []
    comparison_table = []

    for sid, sfile in SAMPLES:
        wav_path = os.path.join(AUDIO_DIR, sfile)
        audio, sr = load_wav_pcm16(wav_path)
        
        # 1. Run DTW on small-q5_1
        out_dtw = os.path.join(OUT_DIR, f"{sid}_small_q5_dtw")
        res_dtw = run_whisper_benchmark(
            whisper_exe=BIN_AVX2,
            model_path=model_q5,
            audio_path=wav_path,
            output_prefix=out_dtw,
            threads=6,
            language="id",
            vad=False,
            dtw="small",
            extra_args=["-nfa"],
        )
        words_dtw = convert_whisper_json_to_words(res_dtw["result_data"]) if res_dtw["result_data"] else []

        # 2. Run Non-DTW on small-q5_1
        out_nodtw = os.path.join(OUT_DIR, f"{sid}_small_q5_nodtw")
        res_nodtw = run_whisper_benchmark(
            whisper_exe=BIN_AVX2,
            model_path=model_q5,
            audio_path=wav_path,
            output_prefix=out_nodtw,
            threads=6,
            language="id",
            vad=False,
            dtw=None,
            extra_args=["-fa"],
        )
        words_nodtw = convert_whisper_json_to_words(res_nodtw["result_data"]) if res_nodtw["result_data"] else []

        # Select 10 words distributed across early, mid, late
        if len(words_dtw) < 10:
            indices = list(range(len(words_dtw)))
        else:
            step = len(words_dtw) // 10
            indices = [i * step for i in range(10)]

        print(f"\n--- Sample: {sid} (Evaluating {len(indices)} words against acoustic onsets) ---")
        print(f"{'Word':15} | {'DTW t0':8} | {'NoDTW t0':8} | {'Acoustic':8} | {'|DTW Err|':9} | {'|NoDTW Err|':11}")
        print("-" * 75)

        for idx in indices:
            w_dtw = words_dtw[idx]
            dtw_t0_s = w_dtw["t0"] / 1000.0
            
            # Find matching word in nodtw by index or proximity
            nodtw_t0_s = dtw_t0_s
            if idx < len(words_nodtw):
                nodtw_t0_s = words_nodtw[idx]["t0"] / 1000.0

            # Detect acoustic onset
            acoustic_t0_s = detect_acoustic_onset(audio, sr, dtw_t0_s)

            err_dtw_ms = abs(dtw_t0_s - acoustic_t0_s) * 1000.0
            err_nodtw_ms = abs(nodtw_t0_s - acoustic_t0_s) * 1000.0

            dtw_errors_ms.append(err_dtw_ms)
            nodtw_errors_ms.append(err_nodtw_ms)
            total_samples_tested += 1

            comparison_table.append({
                "sample": sid,
                "word": w_dtw["text"],
                "dtw_t0_ms": w_dtw["t0"],
                "nodtw_t0_ms": int(nodtw_t0_s * 1000),
                "acoustic_onset_ms": round(acoustic_t0_s * 1000, 1),
                "dtw_error_ms": round(err_dtw_ms, 1),
                "nodtw_error_ms": round(err_nodtw_ms, 1),
            })

            print(f"{w_dtw['text']:15} | {dtw_t0_s*1000:6.0f}ms | {nodtw_t0_s*1000:6.0f}ms | {acoustic_t0_s*1000:6.0f}ms | {err_dtw_ms:7.1f}ms | {err_nodtw_ms:9.1f}ms")

    # Aggregate stats
    dtw_arr = np.array(dtw_errors_ms)
    nodtw_arr = np.array(nodtw_errors_ms)

    print("\n=================================================================")
    print("INDEPENDENT ACOUSTIC ONSET VALIDATION SUMMARY (30 WORDS TOTAL)")
    print("=================================================================")
    print(f"Total Words Tested:           {total_samples_tested}")
    print(f"DTW Mean Error:               {np.mean(dtw_arr):.1f} ms (Median: {np.median(dtw_arr):.1f} ms, Max: {np.max(dtw_arr):.1f} ms)")
    print(f"Non-DTW Mean Error:           {np.mean(nodtw_arr):.1f} ms (Median: {np.median(nodtw_arr):.1f} ms, Max: {np.max(nodtw_arr):.1f} ms)")
    print(f"DTW Words within +-100 ms:    {np.sum(dtw_arr <= 100)} / {total_samples_tested} ({np.mean(dtw_arr <= 100)*100:.1f}%)")
    print(f"Non-DTW Words within +-100 ms:{np.sum(nodtw_arr <= 100)} / {total_samples_tested} ({np.mean(nodtw_arr <= 100)*100:.1f}%)")

    # Save validation output JSON
    res_path = os.path.join(OUT_DIR, "dtw_independent_validation.json")
    with open(res_path, "w", encoding="utf-8") as f:
        json.dump({
            "model_tested": "small-q5_1",
            "total_words": total_samples_tested,
            "dtw_mean_error_ms": round(float(np.mean(dtw_arr)), 1),
            "dtw_median_error_ms": round(float(np.median(dtw_arr)), 1),
            "dtw_max_error_ms": round(float(np.max(dtw_arr)), 1),
            "dtw_within_100ms_pct": round(float(np.mean(dtw_arr <= 100)*100), 1),
            "nodtw_mean_error_ms": round(float(np.mean(nodtw_arr)), 1),
            "nodtw_median_error_ms": round(float(np.median(nodtw_arr)), 1),
            "nodtw_max_error_ms": round(float(np.max(nodtw_arr)), 1),
            "nodtw_within_100ms_pct": round(float(np.mean(nodtw_arr <= 100)*100), 1),
            "spot_check_words": comparison_table,
        }, f, indent=2, ensure_ascii=False)
    print(f"\nSaved independent validation data to: {res_path}")

if __name__ == "__main__":
    main()
