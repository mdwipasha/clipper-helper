import re
from typing import List, Tuple

def normalize_text_for_eval(text: str, unmask_asterisks: bool = True) -> str:
    """
    Standard text normalization for Indonesian ASR evaluation (WER/CER):
    1. Lowercase text.
    2. Optional: unmask censored words (e.g., 'a*****' -> 'anjing', or strip asterisks)
       If asterisks remain, normalize them so 'a*****' becomes 'anjing' if known,
       or collapses 'a*****' to a clean token.
    3. Hyphens: Indonesian reduplications (e.g., 'anak-anak', 'bener-bener', 'masalah-masalah')
       can be written with hyphen or space. To avoid unfair penalties where Whisper predicts
       'anak anak' instead of 'anak-anak', hyphens are converted to spaces.
    4. Apostrophes: Glottal stops or colloquial words (e.g., 'jum\\'at', 'ng\\'gak')
       have apostrophes removed or normalized.
    5. Punctuation: Strip all standard ASCII and Unicode punctuation (?, !, ., ,, ;, :, etc.)
    6. Numbers: Standardized; digits are preserved as tokens.
    7. Whitespace: Multiple whitespace characters collapsed into single space, leading/trailing stripped.
    """
    if not text:
        return ""

    text = text.lower()

    if unmask_asterisks:
        # Gaming slang censor patterns
        text = re.sub(r"(?:\b|(?<=[\s]))a\*{2,}(?=[\s\.,!?;:]|$)", "anjing", text)
        text = re.sub(r"\*{2,}", "", text)

    # Convert hyphens to spaces (Indonesian reduplication consistency)
    text = text.replace("-", " ")
    
    # Remove apostrophes inside words (e.g., jum'at -> jumat, ng'gak -> nggak)
    text = text.replace("'", "").replace("’", "").replace("`", "")

    # Remove all punctuation except alphanumeric and whitespace
    text = re.sub(r"[^\w\s]", " ", text)

    # Collapse multiple whitespace
    text = re.sub(r"\s+", " ", text).strip()
    return text

def levenshtein_distance(seq1: List, seq2: List) -> int:
    """
    Computes Levenshtein edit distance between two sequences (words or characters).
    Uses memory-efficient two-row DP.
    """
    n, m = len(seq1), len(seq2)
    if n == 0:
        return m
    if m == 0:
        return n

    prev = list(range(m + 1))
    curr = [0] * (m + 1)

    for i in range(1, n + 1):
        curr[0] = i
        s1 = seq1[i - 1]
        for j in range(1, m + 1):
            cost = 0 if s1 == seq2[j - 1] else 1
            curr[j] = min(
                prev[j] + 1,        # deletion
                curr[j - 1] + 1,    # insertion
                prev[j - 1] + cost  # substitution
            )
        prev = list(curr)

    return prev[m]

def compute_wer(ref_text: str, hyp_text: str, normalize: bool = True) -> float:
    """
    Computes Word Error Rate: (Substitutions + Deletions + Insertions) / N_ref_words
    """
    if normalize:
        ref_text = normalize_text_for_eval(ref_text)
        hyp_text = normalize_text_for_eval(hyp_text)
    ref_words = ref_text.split()
    hyp_words = hyp_text.split()
    if not ref_words:
        return 0.0 if not hyp_words else 1.0
    dist = levenshtein_distance(ref_words, hyp_words)
    return dist / len(ref_words)

def compute_cer(ref_text: str, hyp_text: str, normalize: bool = True) -> float:
    """
    Computes Character Error Rate: (Substitutions + Deletions + Insertions) / N_ref_chars
    """
    if normalize:
        ref_text = normalize_text_for_eval(ref_text)
        hyp_text = normalize_text_for_eval(hyp_text)
    ref_chars = list(ref_text.replace(" ", ""))
    hyp_chars = list(hyp_text.replace(" ", ""))
    if not ref_chars:
        return 0.0 if not hyp_chars else 1.0
    dist = levenshtein_distance(ref_chars, hyp_chars)
    return dist / len(ref_chars)

if __name__ == "__main__":
    t1 = "Tidak ada ya? Tidak ada lah."
    t2 = "tidak ada ya tidak ada lah"
    print("WER:", compute_wer(t1, t2))
    print("CER:", compute_cer(t1, t2))
