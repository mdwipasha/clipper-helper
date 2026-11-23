"""
S3c Reference Parser — tolerant parser for human-corrected final_<sample>.txt files.

Format: [mm:ss.sss] text   (one line per utterance, UTF-8)
Headers: lines like "SAMPLE:", "MODEL:", "RTF:", "--- FULL TEXT ---", "--- SEGMENTS ---"
Segments: [hh:mm:ss,sss --> hh:mm:ss,sss] text  (draft format, also handled)

Rules:
- Strip optional leading [mm:ss.sss] or [hh:mm:ss,sss --> ...] markers
- Ignore blank lines, header lines (SAMPLE:, MODEL:, RTF:, --- ... ---)
- Exclude [?] tokens from scoring (count and report excluded)
- If reference covers only first 30 s, detect and return max_time_s for hypothesis truncation
"""
import re
import os
from typing import List, Tuple, Optional


# Pattern: [mm:ss.sss] text  (review tool output)
RE_REVIEW_TS = re.compile(r'^\[(\d{1,2}):(\d{2})\.(\d{3})\]\s*(.*)')

# Pattern: [hh:mm:ss,sss --> hh:mm:ss,sss] text  (draft/SRT-like)
RE_SEGMENT_TS = re.compile(r'^\[(\d{2}):(\d{2}):(\d{2}),(\d{3})\s*-->\s*\d{2}:\d{2}:\d{2},\d{3}\]\s*(.*)')

# Header lines to skip
RE_HEADER = re.compile(r'^(SAMPLE:|MODEL:|RTF:|---\s)')

# [?] token pattern
RE_UNINTELLIGIBLE = re.compile(r'\[\?\]')


def parse_timestamp_seconds(match_review=None, match_segment=None) -> float:
    """Convert matched timestamp groups to seconds."""
    if match_review:
        mm, ss, ms = int(match_review.group(1)), int(match_review.group(2)), int(match_review.group(3))
        return mm * 60.0 + ss + ms / 1000.0
    if match_segment:
        hh, mm, ss, ms = (int(match_segment.group(1)), int(match_segment.group(2)),
                          int(match_segment.group(3)), int(match_segment.group(4)))
        return hh * 3600.0 + mm * 60.0 + ss + ms / 1000.0
    return 0.0


def parse_reference_file(filepath: str) -> dict:
    """
    Parse a final_<sample>.txt or draft_<sample>.txt file.

    Returns:
        {
            "lines": [{"time_s": float|None, "text": str}, ...],
            "full_text": str,          # all text joined, [?] excluded
            "full_text_with_unknowns": str,  # all text joined, [?] kept
            "unknown_count": int,       # number of [?] tokens excluded
            "max_time_s": float|None,   # last timestamp seen (for coverage detection)
            "line_count": int,          # lines with actual text
            "skipped_lines": int,       # blank/header lines skipped
        }
    """
    if not os.path.exists(filepath):
        raise FileNotFoundError(f"Reference file not found: {filepath}")

    with open(filepath, "r", encoding="utf-8") as f:
        raw_lines = f.readlines()

    parsed_lines = []
    skipped = 0
    max_time_s = None
    in_segments = False
    in_full_text = False

    for raw in raw_lines:
        line = raw.strip()

        # Skip blank lines
        if not line:
            skipped += 1
            continue

        # Skip header lines
        if RE_HEADER.match(line):
            skipped += 1
            # Detect section markers
            if line.startswith("--- SEGMENTS"):
                in_segments = True
                in_full_text = False
            elif line.startswith("--- FULL TEXT"):
                in_full_text = True
                in_segments = False
            continue

        # Try review tool format: [mm:ss.sss] text
        m_review = RE_REVIEW_TS.match(line)
        if m_review:
            t = parse_timestamp_seconds(match_review=m_review)
            text = m_review.group(4).strip()
            if text:
                parsed_lines.append({"time_s": t, "text": text})
                max_time_s = t if max_time_s is None else max(max_time_s, t)
            else:
                skipped += 1
            continue

        # Try segment format: [hh:mm:ss,sss --> hh:mm:ss,sss] text
        m_seg = RE_SEGMENT_TS.match(line)
        if m_seg:
            t = parse_timestamp_seconds(match_segment=m_seg)
            text = m_seg.group(5).strip()
            if text:
                parsed_lines.append({"time_s": t, "text": text})
                max_time_s = t if max_time_s is None else max(max_time_s, t)
            else:
                skipped += 1
            continue

        # Plain text line (e.g., full text block or headerless line)
        if in_full_text:
            # Full text block — single blob, skip for segment-level parsing
            # We prefer segment lines if available
            skipped += 1
            continue

        # Treat as plain text line (no timestamp)
        parsed_lines.append({"time_s": None, "text": line})

    # Count and strip [?] tokens
    unknown_count = 0
    clean_texts = []
    for pl in parsed_lines:
        text = pl["text"]
        matches = RE_UNINTELLIGIBLE.findall(text)
        unknown_count += len(matches)
        clean = RE_UNINTELLIGIBLE.sub("", text).strip()
        clean = re.sub(r'\s+', ' ', clean).strip()
        if clean:
            clean_texts.append(clean)

    full_text = " ".join(clean_texts)
    full_text_with_unknowns = " ".join(pl["text"] for pl in parsed_lines)

    return {
        "lines": parsed_lines,
        "full_text": full_text,
        "full_text_with_unknowns": full_text_with_unknowns,
        "unknown_count": unknown_count,
        "max_time_s": max_time_s,
        "line_count": len(parsed_lines),
        "skipped_lines": skipped,
    }


def get_reference_text(sample_id: str, refs_dir: str = None) -> Tuple[str, str, int, Optional[float]]:
    """
    Get reference text for a sample, preferring final_*.txt over draft_*.txt.

    Returns: (text, source_label, unknown_count, max_time_s)
    """
    if refs_dir is None:
        refs_dir = os.path.join(os.path.dirname(__file__), "references")

    final_path = os.path.join(refs_dir, f"final_{sample_id}.txt")
    draft_path = os.path.join(refs_dir, f"draft_{sample_id}.txt")

    if os.path.exists(final_path):
        result = parse_reference_file(final_path)
        return result["full_text"], "human_reference", result["unknown_count"], result["max_time_s"]

    if os.path.exists(draft_path):
        result = parse_reference_file(draft_path)
        return result["full_text"], "medium_draft", result["unknown_count"], result["max_time_s"]

    raise FileNotFoundError(f"No reference file found for sample '{sample_id}'")


def truncate_hypothesis_to_time(words: list, max_time_s: float) -> list:
    """
    If reference covers only partial time, truncate hypothesis words
    to those starting before max_time_s + a small buffer.
    """
    buffer = 2.0  # 2 s grace
    return [w for w in words if w["t0"] / 1000.0 <= max_time_s + buffer]


if __name__ == "__main__":
    refs_dir = os.path.join(os.path.dirname(__file__), "references")
    for sample in ["gaming_live", "gaming_slang", "podcast_mixed"]:
        try:
            text, source, unknowns, max_t = get_reference_text(sample, refs_dir)
            words = text.split()
            print(f"\n{sample} ({source}):")
            print(f"  Words: {len(words)}, [?] excluded: {unknowns}, max_time: {max_t:.1f}s" if max_t else f"  Words: {len(words)}, [?] excluded: {unknowns}, max_time: None")
            print(f"  First 50 chars: {text[:50]}...")
        except FileNotFoundError as e:
            print(f"{sample}: {e}")
