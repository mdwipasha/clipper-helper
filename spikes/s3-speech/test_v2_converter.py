"""
Spike S3d: Test improved whisper_to_words logic with timingSource and DTW support.
"""
import re
import json

SPECIAL_TOKEN_REGEX = re.compile(r"^(\[_.*\]|<\|.*\|>)$")

def is_special_token(text):
    if not text:
        return True
    return bool(SPECIAL_TOKEN_REGEX.match(text.strip()))

def clean_token_text(text):
    return text.replace('\r', '').replace('\u00a0', ' ')

def convert_whisper_json_to_words_v2(whisper_data):
    """
    Converts whisper.cpp JSON output (-oj or -ojf) into the canonical
    Composition JSON captions format:
      words: Array<{id?: string, t0: number, t1: number, text: string, timingSource: 'dtw'|'interpolated'|'segment'}>
    
    Uses t_dtw * 10 ms when valid (>=0).
    Falls back to character-proportional interpolation within segment when t_dtw is missing.
    Tags each word with its timingSource. Never silently uses whole-second segment starts.
    """
    all_words = []
    
    transcription = whisper_data.get("transcription", [])
    if not transcription and "result" in whisper_data:
        transcription = whisper_data.get("result", {}).get("transcription", [])

    for seg in transcription:
        seg_from = int(seg.get("offsets", {}).get("from", 0))
        seg_to = int(seg.get("offsets", {}).get("to", seg_from + 1000))
        if seg_to <= seg_from:
            seg_to = seg_from + 1000

        tokens = seg.get("tokens", [])
        
        # 1. Segment-level fallback if tokens are absent
        if not tokens:
            seg_text = seg.get("text", "").strip()
            if not seg_text:
                continue
            seg_words = seg_text.split()
            if not seg_words:
                continue
            
            # Character-proportional interpolation
            total_chars = sum(len(w) for w in seg_words)
            dur = max(seg_to - seg_from, len(seg_words) * 50)
            curr_t = seg_from
            for w in seg_words:
                w_dur = int(dur * (len(w) / max(1, total_chars)))
                w_dur = max(50, w_dur)
                all_words.append({
                    "t0": curr_t,
                    "t1": curr_t + w_dur,
                    "text": w,
                    "timingSource": "segment"
                })
                curr_t += w_dur
            continue

        # Filter out special tokens
        text_tokens = []
        for tok in tokens:
            raw_text = tok.get("text", "")
            if is_special_token(raw_text):
                continue
            cleaned = clean_token_text(raw_text)
            if cleaned:
                text_tokens.append({
                    "raw": raw_text,
                    "cleaned": cleaned,
                    "offsets": tok.get("offsets", {}),
                    "t_dtw": tok.get("t_dtw", -1)
                })

        if not text_tokens:
            continue

        # Check if DTW is present on text tokens
        valid_dtw_tokens = [t for t in text_tokens if t["t_dtw"] is not None and t["t_dtw"] >= 0]
        has_dtw = len(valid_dtw_tokens) >= len(text_tokens) * 0.5  # at least 50% valid DTW

        # Group tokens into words
        word_groups = []
        curr_group = []

        for tok in text_tokens:
            cleaned = tok["cleaned"]
            is_new_word = cleaned.startswith(" ") or cleaned.startswith("\n")
            content = cleaned.lstrip(" \n")
            is_punct_only = bool(re.match(r"^[.,!?;:\"')\]}]+$", content))

            if (is_punct_only or not is_new_word) and curr_group:
                curr_group.append(tok)
            else:
                if curr_group:
                    word_groups.append(curr_group)
                curr_group = [tok]

        if curr_group:
            word_groups.append(curr_group)

        # Assemble words with timestamps
        seg_words = []
        for i, group in enumerate(word_groups):
            word_text = "".join(t["cleaned"] for t in group).strip()
            if not word_text:
                continue

            # Determine timing
            if has_dtw:
                # Find first valid t_dtw in the word's tokens
                first_dtw = next((t["t_dtw"] for t in group if t["t_dtw"] is not None and t["t_dtw"] >= 0), None)
                last_dtw = next((t["t_dtw"] for t in reversed(group) if t["t_dtw"] is not None and t["t_dtw"] >= 0), None)

                if first_dtw is not None:
                    w_t0 = first_dtw * 10
                    # For w_t1: check next word's first DTW, or last token's DTW + estimate
                    if i + 1 < len(word_groups):
                        next_dtw = next((t["t_dtw"] for t in word_groups[i + 1] if t["t_dtw"] is not None and t["t_dtw"] >= 0), None)
                        if next_dtw is not None and next_dtw * 10 > w_t0:
                            w_t1 = min(next_dtw * 10, w_t0 + 1500)
                        else:
                            w_t1 = w_t0 + max(60, len(word_text) * 35)
                    else:
                        # Last word in segment
                        if last_dtw is not None and last_dtw * 10 > w_t0:
                            w_t1 = min(seg_to, last_dtw * 10 + 100)
                        else:
                            w_t1 = min(seg_to, w_t0 + max(60, len(word_text) * 35))
                    
                    w_t1 = max(w_t0 + 30, w_t1)
                    seg_words.append({
                        "t0": w_t0,
                        "t1": w_t1,
                        "text": word_text,
                        "timingSource": "dtw"
                    })
                    continue

            # Fallback: mark for interpolation
            seg_words.append({
                "t0": -1,
                "t1": -1,
                "text": word_text,
                "timingSource": "interpolated"
            })

        # If any words need interpolation, perform character-proportional interpolation
        needs_interp = [w for w in seg_words if w["timingSource"] == "interpolated"]
        if needs_interp:
            total_chars = sum(len(w["text"]) for w in seg_words)
            dur = max(seg_to - seg_from, len(seg_words) * 50)
            curr_t = seg_from
            for w in seg_words:
                if w["timingSource"] == "interpolated":
                    w_dur = int(dur * (len(w["text"]) / max(1, total_chars)))
                    w_dur = max(50, w_dur)
                    w["t0"] = curr_t
                    w["t1"] = curr_t + w_dur
                    curr_t += w_dur
                else:
                    curr_t = max(curr_t, w["t1"])

        all_words.extend(seg_words)

    # Post-process: ensure monotonic timestamps and minimum duration
    clean_words = []
    for i, w in enumerate(all_words):
        t0 = max(0, w["t0"])
        t1 = max(t0 + 20, w["t1"])
        clean_words.append({
            "t0": t0,
            "t1": t1,
            "text": w["text"],
            "timingSource": w["timingSource"]
        })

    return clean_words

if __name__ == "__main__":
    with open("results/matrix/model_small-q5_1_gaming_slang.json", "r", encoding="utf-8") as f:
        data = json.load(f)
    words = convert_whisper_json_to_words_v2(data)
    print(f"Total words: {len(words)}")
    dtw_count = sum(1 for w in words if w["timingSource"] == "dtw")
    interp_count = sum(1 for w in words if w["timingSource"] == "interpolated")
    seg_count = sum(1 for w in words if w["timingSource"] == "segment")
    whole_sec = sum(1 for w in words if w["t0"] % 1000 == 0)
    print(f"timingSource: dtw={dtw_count}, interpolated={interp_count}, segment={seg_count}")
    print(f"Whole second t0: {whole_sec}/{len(words)} ({whole_sec/len(words)*100:.1f}%)")
    print("\nFirst 10 words:")
    for w in words[:10]:
        print(f"  {w['text']:15} | t0={w['t0']:6d}ms | t1={w['t1']:6d}ms | dur={w['t1']-w['t0']:4d}ms | {w['timingSource']}")
