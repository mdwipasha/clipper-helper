import json
import os

BASE_DIR = os.path.abspath("spikes/s3-speech")
RESULTS_DIR = os.path.join(BASE_DIR, "results", "matrix")

def compare_timestamps(dtw_json_path, nodtw_json_path, sample_count=30):
    if not os.path.exists(dtw_json_path) or not os.path.exists(nodtw_json_path):
        print(f"Files not ready yet: {dtw_json_path}")
        return

    from whisper_to_words import convert_whisper_json_to_words
    
    with open(dtw_json_path, "r", encoding="utf-8") as f:
        dtw_words = convert_whisper_json_to_words(json.load(f))
    with open(nodtw_json_path, "r", encoding="utf-8") as f:
        nodtw_words = convert_whisper_json_to_words(json.load(f))

    print(f"\nComparing Word Timestamps: DTW vs Non-DTW (first {sample_count} words)")
    print(f"{'Idx':3} | {'Word Text':18} | {'DTW [t0 - t1]':18} | {'Non-DTW [t0 - t1]':20} | {'Delta t0 (ms)':12}")
    print("-" * 80)

    n = min(len(dtw_words), len(nodtw_words), sample_count)
    deltas = []
    within_100ms_count = 0

    for i in range(n):
        w_dtw = dtw_words[i]
        w_nodtw = nodtw_words[i]
        delta_t0 = abs(w_dtw["t0"] - w_nodtw["t0"])
        deltas.append(delta_t0)
        if delta_t0 <= 100:
            within_100ms_count += 1
        
        dtw_str = f"{w_dtw['t0']:5} - {w_dtw['t1']:5} ms"
        nodtw_str = f"{w_nodtw['t0']:5} - {w_nodtw['t1']:5} ms"
        print(f"{i:3} | {w_dtw['text']:18} | {dtw_str:18} | {nodtw_str:20} | {delta_t0:8} ms")

    avg_delta = sum(deltas) / len(deltas) if deltas else 0
    max_delta = max(deltas) if deltas else 0
    pct_within_100ms = (within_100ms_count / len(deltas)) * 100 if deltas else 0
    
    print("-" * 80)
    print(f"Average |Delta t0|: {avg_delta:.1f} ms")
    print(f"Max |Delta t0|:     {max_delta:.1f} ms")
    print(f"Words within +-100ms target: {within_100ms_count}/{len(deltas)} ({pct_within_100ms:.1f}%)")

if __name__ == "__main__":
    dtw_path = os.path.join(RESULTS_DIR, "dtw_True.json")
    nodtw_path = os.path.join(RESULTS_DIR, "dtw_False.json")
    compare_timestamps(dtw_path, nodtw_path)
