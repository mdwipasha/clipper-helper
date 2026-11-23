"""
Spike S3d - Step 1: Token & Converter Diagnostic
Audits all tokens and words across models (base, small-q5_1, small) and samples.
Checks:
- Presence of t_dtw vs offsets
- % of tokens with t_dtw >= 0 vs -1
- Distribution of t0 % 1000 (whole-second artifact)
- Word duration distributions
- Lists any true fallback tokens
"""
import os
import json
from collections import Counter
import numpy as np

BASE_DIR = os.path.abspath(os.path.dirname(__file__))
MATRIX_DIR = os.path.join(BASE_DIR, "results", "matrix")

MODELS = ["base", "small-q5_1", "small"]
SAMPLES = ["gaming_slang", "podcast_mixed", "gaming_live"]

def audit_json_file(filepath):
    with open(filepath, "r", encoding="utf-8") as f:
        data = json.load(f)
    
    tx = data.get("transcription", [])
    
    total_segments = len(tx)
    total_tokens = 0
    special_tokens = 0
    text_tokens = 0
    tokens_with_valid_dtw = 0
    tokens_with_neg_dtw = 0
    tokens_missing_dtw = 0
    
    offsets_from_vals = []
    dtw_from_vals = []
    fallback_tokens = []
    
    for seg_idx, seg in enumerate(tx):
        seg_from = seg.get("offsets", {}).get("from", 0)
        seg_to = seg.get("offsets", {}).get("to", 0)
        tokens = seg.get("tokens", [])
        total_tokens += len(tokens)
        
        for tok in tokens:
            raw_text = tok.get("text", "")
            is_special = raw_text.startswith("[_") or raw_text.startswith("<|")
            if is_special:
                special_tokens += 1
                continue
            
            text_tokens += 1
            t_dtw = tok.get("t_dtw", None)
            offsets = tok.get("offsets", {})
            o_from = offsets.get("from", None)
            
            if o_from is not None:
                offsets_from_vals.append(o_from)
            
            if t_dtw is None:
                tokens_missing_dtw += 1
                fallback_tokens.append((seg_idx, raw_text, "missing_dtw", offsets))
            elif t_dtw < 0:
                tokens_with_neg_dtw += 1
                fallback_tokens.append((seg_idx, raw_text, f"negative_dtw_{t_dtw}", offsets))
            else:
                tokens_with_valid_dtw += 1
                dtw_ms = t_dtw * 10
                dtw_from_vals.append(dtw_ms)
                
    # Calculate % whole seconds
    offsets_whole_sec = sum(1 for v in offsets_from_vals if v % 1000 == 0)
    dtw_whole_sec = sum(1 for v in dtw_from_vals if v % 1000 == 0)
    
    return {
        "total_segments": total_segments,
        "total_tokens": total_tokens,
        "special_tokens": special_tokens,
        "text_tokens": text_tokens,
        "tokens_with_valid_dtw": tokens_with_valid_dtw,
        "tokens_with_neg_dtw": tokens_with_neg_dtw,
        "tokens_missing_dtw": tokens_missing_dtw,
        "offsets_count": len(offsets_from_vals),
        "offsets_whole_sec": offsets_whole_sec,
        "offsets_whole_sec_pct": round(offsets_whole_sec / max(1, len(offsets_from_vals)) * 100, 2),
        "dtw_count": len(dtw_from_vals),
        "dtw_whole_sec": dtw_whole_sec,
        "dtw_whole_sec_pct": round(dtw_whole_sec / max(1, len(dtw_from_vals)) * 100, 2),
        "fallback_tokens": fallback_tokens,
        "offsets_from_vals": offsets_from_vals,
        "dtw_from_vals": dtw_from_vals,
    }

def main():
    print("=" * 80)
    print("SPIKE S3d - STEP 1: TOKEN AUDIT ACROSS MODELS & SAMPLES")
    print("=" * 80)
    
    summary = []
    
    for model in MODELS:
        for sample in SAMPLES:
            filename = f"model_{model}_{sample}.json"
            filepath = os.path.join(MATRIX_DIR, filename)
            if not os.path.exists(filepath):
                print(f"Skipping {filename} (not found)")
                continue
                
            res = audit_json_file(filepath)
            summary.append({
                "model": model,
                "sample": sample,
                **res
            })
            
            print(f"\n[{model}] - [{sample}]")
            print(f"  Segments: {res['total_segments']} | Text Tokens: {res['text_tokens']}")
            print(f"  t_dtw >= 0: {res['tokens_with_valid_dtw']}/{res['text_tokens']} ({res['tokens_with_valid_dtw']/max(1,res['text_tokens'])*100:.1f}%)")
            print(f"  t_dtw < 0:  {res['tokens_with_neg_dtw']}/{res['text_tokens']}")
            print(f"  Missing:    {res['tokens_missing_dtw']}/{res['text_tokens']}")
            print(f"  offsets.from % 1000 == 0: {res['offsets_whole_sec']}/{res['offsets_count']} ({res['offsets_whole_sec_pct']}%)  <-- WHOLE SECOND ARTIFACT")
            print(f"  (t_dtw*10)   % 1000 == 0: {res['dtw_whole_sec']}/{res['dtw_count']} ({res['dtw_whole_sec_pct']}%)  <-- TRUE DTW RESOLUTION")
            
            if res['fallback_tokens']:
                print(f"  Fallback tokens ({len(res['fallback_tokens'])}):")
                for fb in res['fallback_tokens'][:5]:
                    print(f"    seg {fb[0]}: {fb[1]!r} ({fb[2]})")
            else:
                print("  Fallback tokens: NONE (100% of text tokens have valid t_dtw!)")

    print("\n" + "=" * 80)
    print("AGGREGATE SUMMARY ACROSS ALL 9 RUNS")
    print("=" * 80)
    print(f"{'Model':12} {'Sample':15} {'Tokens':>7} {'DTW valid%':>11} {'Offsets %1s':>13} {'DTW %1s':>10}")
    print("-" * 72)
    for s in summary:
        print(f"{s['model']:12} {s['sample']:15} {s['text_tokens']:7d} {s['tokens_with_valid_dtw']/max(1,s['text_tokens'])*100:10.1f}% {s['offsets_whole_sec_pct']:12.1f}% {s['dtw_whole_sec_pct']:9.1f}%")

    # Save summary json
    out_path = os.path.join(BASE_DIR, "results", "s3c", "s3d_token_audit_summary.json")
    # remove large arrays for saving
    save_data = []
    for s in summary:
        d = dict(s)
        del d["offsets_from_vals"]
        del d["dtw_from_vals"]
        save_data.append(d)
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(save_data, f, indent=2)
    print(f"\nSaved audit summary to {out_path}")

if __name__ == "__main__":
    main()
