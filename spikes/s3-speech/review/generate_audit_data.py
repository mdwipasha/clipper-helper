"""
Generate deterministically sampled 30 words per sample (seed=42)
for the word timing audit page in spikes/s3-speech/review/
"""
import os
import sys
import json
import random

sys.path.insert(0, os.path.dirname(os.path.dirname(__file__)))
from whisper_to_words import convert_whisper_json_to_words

BASE_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MATRIX_DIR = os.path.join(BASE_DIR, "results", "matrix")
REVIEW_DIR = os.path.join(BASE_DIR, "review")

SAMPLES = [
    {
        "id": "gaming_live",
        "title": "Gaming Livestream (Windah Basudara GTA 6)",
        "audio": "../audio/gaming_live.wav",
        "json": "model_small-q5_1_gaming_live.json"
    },
    {
        "id": "gaming_slang",
        "title": "Gaming Slang (MiawAug Deceit 2)",
        "audio": "../audio/gaming_slang.wav",
        "json": "model_small-q5_1_gaming_slang.json"
    },
    {
        "id": "podcast_mixed",
        "title": "Podcast Mixed (Liandi Podcast)",
        "audio": "../audio/podcast_mixed.wav",
        "json": "model_small-q5_1_podcast_mixed.json"
    }
]

def generate():
    audit_data = {}

    for s in SAMPLES:
        json_path = os.path.join(MATRIX_DIR, s["json"])
        with open(json_path, "r", encoding="utf-8") as f:
            data = json.load(f)

        words = convert_whisper_json_to_words(data)

        # Set fixed seed=42 for deterministic sampling
        rng = random.Random(42)
        n_sample = min(30, len(words))
        sampled_indices = sorted(rng.sample(range(len(words)), n_sample))

        sampled_words = []
        for rank, idx in enumerate(sampled_indices):
            w = words[idx]
            # Context window: 3 words before, 3 words after
            start_ctx = max(0, idx - 3)
            end_ctx = min(len(words), idx + 4)
            ctx_text = " ".join(
                f"[{words[i]['text']}]" if i == idx else words[i]["text"]
                for i in range(start_ctx, end_ctx)
            )

            sampled_words.append({
                "sample_id": s["id"],
                "index": idx,
                "rank": rank + 1,
                "word": w["text"],
                "t0_ms": w["t0"],
                "t1_ms": w["t1"],
                "t0_s": round(w["t0"] / 1000.0, 3),
                "t1_s": round(w["t1"] / 1000.0, 3),
                "dur_ms": w["t1"] - w["t0"],
                "timingSource": w.get("timingSource", "dtw"),
                "context": ctx_text,
                # Snippet boundaries [t0 - 0.3s, t0 + 0.6s]
                "snippet_start_s": max(0.0, round(w["t0"] / 1000.0 - 0.3, 3)),
                "snippet_end_s": round(w["t0"] / 1000.0 + 0.6, 3),
            })

        audit_data[s["id"]] = {
            "title": s["title"],
            "audio": s["audio"],
            "total_words": len(words),
            "sampled_count": len(sampled_words),
            "words": sampled_words
        }

    # Write as JS file for easy loading in review web app
    out_js = os.path.join(REVIEW_DIR, "audit_data.js")
    with open(out_js, "w", encoding="utf-8") as f:
        f.write("// Generated deterministically by generate_audit_data.py (seed=42)\n")
        f.write("const AUDIT_DATA = " + json.dumps(audit_data, indent=2) + ";\n")

    # Also save as JSON
    out_json = os.path.join(BASE_DIR, "results", "s3d_validation", "audit_samples_30.json")
    os.makedirs(os.path.dirname(out_json), exist_ok=True)
    with open(out_json, "w", encoding="utf-8") as f:
        json.dump(audit_data, f, indent=2)

    print(f"Generated audit data: 30 words per sample, written to {out_js} and {out_json}")

if __name__ == "__main__":
    generate()
