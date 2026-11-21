import re

def normalize_text_for_eval(text: str) -> str:
    """
    Standard text normalization for ASR evaluation (WER/CER):
    - Lowercase
    - Replace hyphens with spaces
    - Remove punctuation
    - Collapse multiple whitespace
    """
    text = text.lower()
    text = text.replace("-", " ")
    text = re.sub(r"[^\w\s]", "", text)
    text = re.sub(r"\s+", " ", text).strip()
    return text

def levenshtein_distance(seq1, seq2):
    """
    Computes Levenshtein edit distance between two sequences (words or characters).
    """
    n, m = len(seq1), len(seq2)
    if n == 0:
        return m
    if m == 0:
        return n

    # Two rows DP to save memory
    prev = list(range(m + 1))
    curr = [0] * (m + 1)

    for i in range(1, n + 1):
        curr[0] = i
        for j in range(1, m + 1):
            cost = 0 if seq1[i - 1] == seq2[j - 1] else 1
            curr[j] = min(
                prev[j] + 1,      # deletion
                curr[j - 1] + 1,  # insertion
                prev[j - 1] + cost # substitution
            )
        prev = list(curr)

    return prev[m]

def compute_wer(ref_text: str, hyp_text: str, normalize: bool = True):
    if normalize:
        ref_text = normalize_text_for_eval(ref_text)
        hyp_text = normalize_text_for_eval(hyp_text)
    ref_words = ref_text.split()
    hyp_words = hyp_text.split()
    if not ref_words:
        return 0.0 if not hyp_words else 1.0
    dist = levenshtein_distance(ref_words, hyp_words)
    return dist / len(ref_words)

def compute_cer(ref_text: str, hyp_text: str, normalize: bool = True):
    if normalize:
        ref_text = normalize_text_for_eval(ref_text)
        hyp_text = normalize_text_for_eval(hyp_text)
    ref_chars = list(ref_text)
    hyp_chars = list(hyp_text)
    if not ref_chars:
        return 0.0 if not hyp_chars else 1.0
    dist = levenshtein_distance(ref_chars, hyp_chars)
    return dist / len(ref_chars)

if __name__ == "__main__":
    t1 = "Tidak ada ya? Tidak ada lah."
    t2 = "tidak ada ya tidak ada lah"
    print("WER:", compute_wer(t1, t2))
    print("CER:", compute_cer(t1, t2))
