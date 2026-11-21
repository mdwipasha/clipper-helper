import re
import json

SPECIAL_TOKEN_REGEX = re.compile(r"^(\[_.*\]|<\|.*\|>)$")

def is_special_token(text):
    if not text:
        return True
    return bool(SPECIAL_TOKEN_REGEX.match(text.strip()))

def clean_token_text(text):
    # Strip carriage returns, replace non-breaking spaces
    return text.replace('\r', '').replace('\u00a0', ' ')

def convert_whisper_json_to_words(whisper_data):
    """
    Converts whisper.cpp JSON output (-oj or -ojf) into the canonical
    Composition JSON captions format:
      words: Array<{t0: number, t1: number, text: string}>
    All timestamps are integer milliseconds.
    Sub-words are merged, Indonesian punctuation and apostrophes preserved,
    and special tokens removed.
    """
    words = []
    
    transcription = whisper_data.get("transcription", [])
    if not transcription and "result" in whisper_data:
        # Fallback if structure differs
        transcription = whisper_data.get("result", {}).get("transcription", [])

    for seg in transcription:
        tokens = seg.get("tokens", [])
        
        # If tokens are not present (e.g. standard -oj without tokens),
        # estimate word timings from segment timestamps and text
        if not tokens:
            seg_text = seg.get("text", "").strip()
            if not seg_text:
                continue
            seg_t0 = int(seg.get("offsets", {}).get("from", 0))
            seg_t1 = int(seg.get("offsets", {}).get("to", seg_t0 + 1000))
            seg_words = seg_text.split()
            if not seg_words:
                continue
            dur = max(seg_t1 - seg_t0, len(seg_words) * 50)
            step = dur / len(seg_words)
            for idx, w in enumerate(seg_words):
                w_t0 = int(seg_t0 + idx * step)
                w_t1 = int(seg_t0 + (idx + 1) * step)
                words.append({"t0": w_t0, "t1": max(w_t1, w_t0 + 50), "text": w})
            continue

        curr_word = None

        for tok in tokens:
            raw_text = tok.get("text", "")
            if is_special_token(raw_text):
                continue
            
            cleaned = clean_token_text(raw_text)
            if not cleaned:
                continue

            offsets = tok.get("offsets", {})
            t0 = int(offsets.get("from", 0))
            t1 = int(offsets.get("to", t0 + 50))
            if t1 <= t0:
                t1 = t0 + 50

            # Does this token start a new word?
            # In Whisper BPE, a leading space indicates the beginning of a word.
            # Pure punctuation marks attached without space (or trailing closing marks) merge with previous.
            is_new_word = cleaned.startswith(" ") or cleaned.startswith("\n")
            content = cleaned.lstrip(" \n")

            # Check if this token is just punctuation
            is_punct_only = bool(re.match(r"^[.,!?;:\"')\]}]+$", content))
            is_opening_punct = bool(re.match(r"^[\"'({\[-]+$", content))

            if is_punct_only and curr_word is not None and not is_new_word:
                # Attach to existing word (e.g. '?', '.', ',')
                curr_word["text"] += content
                curr_word["t1"] = max(curr_word["t1"], t1)
            elif curr_word is not None and not is_new_word:
                # Continuation sub-word (e.g. 'id', 'ak', 'nya', '-anak')
                curr_word["text"] += content
                curr_word["t1"] = max(curr_word["t1"], t1)
            else:
                # Start a new word
                if curr_word is not None:
                    # Finalize current word
                    if curr_word["text"].strip():
                        words.append(curr_word)
                
                curr_word = {
                    "t0": t0,
                    "t1": t1,
                    "text": content,
                }

        if curr_word is not None and curr_word["text"].strip():
            words.append(curr_word)

    # Post-process: ensure monotonic timestamps and no 0-duration words
    clean_words = []
    for w in words:
        text = w["text"].strip()
        if not text:
            continue
        t0 = max(0, w["t0"])
        t1 = max(t0 + 20, w["t1"])
        clean_words.append({
            "t0": t0,
            "t1": t1,
            "text": text
        })

    return clean_words
