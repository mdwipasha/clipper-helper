"""
Spike S3d: Word-level statistics with fixed whisper_to_words.py
"""
import os
import json
import numpy as np

BASE_DIR = os.path.abspath(os.path.dirname(__file__))
MATRIX_DIR = os.path.join(BASE_DIR, "results", "matrix")
from whisper_to_words import convert_whisper_json_to_words

MODELS = ["base", "small-q5_1", "small"]
SAMPLES = ["gaming_slang", "podcast_mixed", "gaming_live"]

print("=" * 80)
print("WORD-LEVEL TIMING DIAGNOSTIC WITH FIXED whisper_to_words.py")
print("=" * 80)
print(f"{'Model':12} {'Sample':15} {'Words':>6} {'timingSource':>15} {'Whole-sec t0':>15} {'Mean Dur':>10} {'Med Dur':>10}")
print("-" * 80)

for model in MODELS:
    for sample in SAMPLES:
        path = os.path.join(MATRIX_DIR, f"model_{model}_{sample}.json")
        if not os.path.exists(path):
            continue
        with open(path, "r", encoding="utf-8") as f:
            data = json.load(f)
        words = convert_whisper_json_to_words(data)
        durs = [w["t1"] - w["t0"] for w in words]
        ws_count = sum(1 for w in words if w["t0"] % 1000 == 0)
        dtw_count = sum(1 for w in words if w.get("timingSource") == "dtw")
        
        print(f"{model:12} {sample:15} {len(words):6d} {f'{dtw_count}/{len(words)} dtw':>15} {f'{ws_count}/{len(words)} ({ws_count/max(1,len(words))*100:.1f}%)':>15} {np.mean(durs):9.1f}ms {np.median(durs):9.1f}ms")
