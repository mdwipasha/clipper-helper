"""
S3c Evaluation — Recompute WER/CER against human-corrected references.

Reuses existing whisper output JSONs from results/matrix/ to avoid
re-running the full benchmark matrix (~15 min).

Outputs:
  - STRICT WER/CER (standard normalization only)
  - LENIENT WER/CER (+ colloquial spelling normalization)
  - Per-sample error examples (names, jargon, profanity, repeated tokens)
  - Token loop detection
"""
import os
import sys
import json
import re
from collections import Counter

sys.path.insert(0, os.path.dirname(__file__))
from s3c_reference_parser import get_reference_text
from whisper_to_words import convert_whisper_json_to_words
from accuracy_eval import (
    normalize_text_for_eval, normalize_colloquial, get_colloquial_map,
    compute_wer, compute_cer, compute_wer_lenient, compute_cer_lenient,
    levenshtein_distance,
)

BASE_DIR = os.path.abspath(os.path.dirname(__file__))
MATRIX_DIR = os.path.join(BASE_DIR, "results", "matrix")
REFS_DIR = os.path.join(BASE_DIR, "references")
OUT_DIR = os.path.join(BASE_DIR, "results", "s3c")
os.makedirs(OUT_DIR, exist_ok=True)

SAMPLES = ["gaming_slang", "podcast_mixed", "gaming_live"]
MODELS = ["base", "small-q5_1", "small", "medium"]

# Words of interest for per-sample error analysis
INTEREST_WORDS = {
    "gaming_slang": ["anjing", "boki", "ngentot", "poison", "gigi", "tele"],
    "podcast_mixed": ["jonathan", "liandi", "esport", "chou", "aurora", "kapten"],
    "gaming_live": ["suamran", "windah", "lucia", "boosting", "anjing", "ngentot"],
}


def load_whisper_output(model: str, sample: str) -> dict:
    """Load existing whisper JSON output from results/matrix/."""
    json_path = os.path.join(MATRIX_DIR, f"model_{model}_{sample}.json")
    if not os.path.exists(json_path):
        raise FileNotFoundError(f"Missing whisper output: {json_path}")
    with open(json_path, "r", encoding="utf-8") as f:
        return json.load(f)


def extract_hypothesis_text(whisper_data: dict) -> str:
    """Extract full hypothesis text from whisper JSON."""
    if "transcription" in whisper_data:
        segments = whisper_data["transcription"]
        texts = [seg.get("text", "").strip() for seg in segments]
        return " ".join(t for t in texts if t)
    return ""


def detect_token_loops(text: str, min_repeat: int = 3) -> list:
    """
    Detect repeated-token loops in hypothesis text.
    Returns list of (token_sequence, repeat_count).
    """
    words = text.lower().split()
    loops = []
    i = 0
    while i < len(words):
        # Check for single-word repetitions
        count = 1
        while i + count < len(words) and words[i + count] == words[i]:
            count += 1
        if count >= min_repeat:
            loops.append((words[i], count))
            i += count
        else:
            # Check for 2-word phrase repetitions
            if i + 1 < len(words):
                phrase = (words[i], words[i + 1])
                pcount = 1
                j = i + 2
                while j + 1 < len(words) and (words[j], words[j + 1]) == phrase:
                    pcount += 1
                    j += 2
                if pcount >= min_repeat:
                    loops.append((" ".join(phrase), pcount))
                    i = j
                    continue
            i += 1
    return loops


def compute_alignment_errors(ref_text: str, hyp_text: str, interest_words: list) -> list:
    """
    Find specific error examples for words of interest.
    Returns list of error descriptions.
    """
    ref_norm = normalize_text_for_eval(ref_text).split()
    hyp_norm = normalize_text_for_eval(hyp_text).split()
    errors = []

    for word in interest_words:
        word_lower = word.lower()
        in_ref = word_lower in [w.lower() for w in ref_norm]
        in_hyp = word_lower in [w.lower() for w in hyp_norm]

        if in_ref and not in_hyp:
            errors.append(f"DELETION: '{word}' in reference but missing from hypothesis")
        elif not in_ref and in_hyp:
            errors.append(f"INSERTION: '{word}' not in reference but inserted in hypothesis")
        # Check for substitutions near the position
        for ri, rw in enumerate(ref_norm):
            if rw.lower() == word_lower:
                # Find closest match in hypothesis
                closest_idx = min(range(len(hyp_norm)),
                                  key=lambda j: abs(j - ri)) if hyp_norm else -1
                if closest_idx >= 0 and hyp_norm[closest_idx].lower() != word_lower:
                    # Check neighborhood
                    start = max(0, ri - 2)
                    end = min(len(hyp_norm), ri + 3)
                    neighborhood = hyp_norm[start:end]
                    if word_lower not in [w.lower() for w in neighborhood]:
                        errors.append(
                            f"SUBSTITUTION: ref[{ri}]='{word}', hyp neighborhood={neighborhood}"
                        )
                break

    return errors


def main():
    print("=" * 70)
    print("S3c: RECOMPUTING WER/CER AGAINST HUMAN-CORRECTED REFERENCES")
    print("=" * 70)

    # Print colloquial map for review
    cmap = get_colloquial_map()
    print(f"\nColloquial normalization map ({len(cmap)} entries):")
    for variant in sorted(cmap.keys()):
        print(f"  {variant:10s} → {cmap[variant]}")

    all_results = []

    for model in MODELS:
        print(f"\n{'=' * 70}")
        print(f"MODEL: {model}")
        print(f"{'=' * 70}")

        for sample in SAMPLES:
            print(f"\n  --- {sample} ---")

            # Load reference
            ref_text, ref_source, unknowns, max_time = get_reference_text(sample, REFS_DIR)
            ref_words = ref_text.split()
            print(f"  Reference: {ref_source}, {len(ref_words)} words, [?] excluded: {unknowns}")

            # Load hypothesis
            try:
                whisper_data = load_whisper_output(model, sample)
            except FileNotFoundError as e:
                print(f"  SKIP: {e}")
                continue

            hyp_text = extract_hypothesis_text(whisper_data)
            hyp_words_raw = hyp_text.split()

            # Compute STRICT WER/CER
            strict_wer = compute_wer(ref_text, hyp_text, normalize=True)
            strict_cer = compute_cer(ref_text, hyp_text, normalize=True)

            # Compute LENIENT WER/CER
            lenient_wer = compute_wer_lenient(ref_text, hyp_text)
            lenient_cer = compute_cer_lenient(ref_text, hyp_text)

            # Detect token loops
            loops = detect_token_loops(hyp_text)

            # Interest word errors
            interest = INTEREST_WORDS.get(sample, [])
            error_examples = compute_alignment_errors(ref_text, hyp_text, interest)

            # Check for hallucinated outro
            has_outro = False
            if sample == "gaming_live":
                hyp_lower = hyp_text.lower()
                if "sampai jumpa" in hyp_lower or "video selanjutnya" in hyp_lower:
                    has_outro = True

            print(f"  Hypothesis: {len(hyp_words_raw)} raw words")
            print(f"  STRICT  WER: {strict_wer*100:6.2f}%  CER: {strict_cer*100:6.2f}%")
            print(f"  LENIENT WER: {lenient_wer*100:6.2f}%  CER: {lenient_cer*100:6.2f}%")
            if loops:
                print(f"  TOKEN LOOPS: {loops}")
            if error_examples:
                for err in error_examples:
                    print(f"    {err}")
            if has_outro:
                print(f"  ⚠ HALLUCINATED OUTRO DETECTED")

            result = {
                "model": model,
                "sample": sample,
                "ref_source": ref_source,
                "ref_word_count": len(ref_words),
                "hyp_word_count": len(hyp_words_raw),
                "unknown_tokens_excluded": unknowns,
                "strict_wer_pct": round(strict_wer * 100, 2),
                "strict_cer_pct": round(strict_cer * 100, 2),
                "lenient_wer_pct": round(lenient_wer * 100, 2),
                "lenient_cer_pct": round(lenient_cer * 100, 2),
                "token_loops": [{"token": t, "count": c} for t, c in loops],
                "error_examples": error_examples,
                "has_outro_hallucination": has_outro,
                "hyp_text_normalized": normalize_text_for_eval(hyp_text),
                "ref_text_normalized": normalize_text_for_eval(ref_text),
            }
            all_results.append(result)

    # Summary table
    print(f"\n{'=' * 70}")
    print("SUMMARY TABLE: Model × Sample (vs human reference)")
    print(f"{'=' * 70}")
    print(f"{'Model':12} {'Sample':16} {'S-WER%':>8} {'S-CER%':>8} {'L-WER%':>8} {'L-CER%':>8} {'Loops':>6} {'Outro':>6}")
    print("-" * 78)
    for r in all_results:
        loops_str = str(len(r["token_loops"])) if r["token_loops"] else "-"
        outro_str = "YES" if r["has_outro_hallucination"] else "-"
        print(f"{r['model']:12} {r['sample']:16} {r['strict_wer_pct']:8.2f} {r['strict_cer_pct']:8.2f} "
              f"{r['lenient_wer_pct']:8.2f} {r['lenient_cer_pct']:8.2f} {loops_str:>6} {outro_str:>6}")

    # Check S3b claims
    print(f"\n{'=' * 70}")
    print("CLAIM AUDIT")
    print(f"{'=' * 70}")

    # Claim 1: "small-q5_1 achieved lower WER than small on podcast_mixed"
    sq5_pod = next((r for r in all_results if r["model"] == "small-q5_1" and r["sample"] == "podcast_mixed"), None)
    s_pod = next((r for r in all_results if r["model"] == "small" and r["sample"] == "podcast_mixed"), None)
    if sq5_pod and s_pod:
        print(f"\nClaim: 'small-q5_1 achieved lower WER than small on podcast_mixed (less token looping)'")
        print(f"  small-q5_1 STRICT WER: {sq5_pod['strict_wer_pct']:.2f}%  vs  small STRICT WER: {s_pod['strict_wer_pct']:.2f}%")
        print(f"  small-q5_1 LENIENT WER: {sq5_pod['lenient_wer_pct']:.2f}%  vs  small LENIENT WER: {s_pod['lenient_wer_pct']:.2f}%")
        if sq5_pod['strict_wer_pct'] < s_pod['strict_wer_pct']:
            print(f"  → SUPPORTED: small-q5_1 has lower WER against human ref")
        else:
            print(f"  → NOT SUPPORTED: small has equal or lower WER against human ref")

    # Claim 2: "identical or superior fidelity" for small-q5_1
    print(f"\nClaim: 'small-q5_1 delivers identical or superior fidelity to small'")
    for sample in SAMPLES:
        sq5 = next((r for r in all_results if r["model"] == "small-q5_1" and r["sample"] == sample), None)
        s = next((r for r in all_results if r["model"] == "small" and r["sample"] == sample), None)
        if sq5 and s:
            diff = sq5['lenient_wer_pct'] - s['lenient_wer_pct']
            verdict = "BETTER" if diff < -1.0 else "SIMILAR" if abs(diff) <= 1.0 else "WORSE"
            print(f"  {sample}: q5={sq5['lenient_wer_pct']:.1f}% vs full={s['lenient_wer_pct']:.1f}% → {verdict} (Δ={diff:+.1f}%)")

    # Save results
    out_path = os.path.join(OUT_DIR, "s3c_eval_results.json")
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(all_results, f, indent=2, ensure_ascii=False)
    print(f"\nSaved {len(all_results)} results to {out_path}")


if __name__ == "__main__":
    main()
