import re
from typing import List, Tuple

# ──────────────────────────────────────────────────────────────────────
# Indonesian colloquial spelling normalization map (S3c)
# Canonical form is the FIRST entry in each group.
# Applied AFTER standard normalization (lowercase, punctuation stripped).
# Mapping rationale: these variants are phonologically identical in
# informal Indonesian speech; penalizing ASR for picking one variant
# over another inflates WER on casual/gaming audio.
# ──────────────────────────────────────────────────────────────────────
COLLOQUIAL_GROUPS = [
    # Pronoun variants
    ["gue", "gw", "gua", "gwe"],           # 1st person singular informal
    ["lu",  "lo"],                           # 2nd person singular informal
    # Negation variants
    ["enggak", "nggak", "gak", "ga", "ngga", "nggk"],
    # Already / completion
    ["udah", "udh", "uda"],
    # Conditional
    ["kalau", "kalo", "klo"],
    # Intensifier
    ["banget", "bgt", "bngt"],
    # Question "how"
    ["gimana", "gmn", "gmna"],
    # "indeed" / emphasis
    ["emang", "emg"],
    # "like" (similative)
    ["kayak", "kyk"],
    # Demonstrative particles
    ["tuh", "tu"],
    ["nih", "ni"],
    # Softening particles
    ["aja", "aj"],
    ["deh", "de"],
    ["dong", "donk", "dng"],
    ["sih", "si"],
    # "correct" / "true"
    ["bener", "bner"],
    # "want"
    ["mau", "mo", "mw"],
    # "which" / "what"
    ["yang", "yg"],
    # "with"
    ["sama", "sm"],
    # "already" formal→informal
    ["sudah", "sdh"],
    # "so" / "then"
    ["jadi", "jd"],
    # "if" / conditional
    ["kalau", "kl"],
    # "can" / "could"
    ["bisa", "bs"],
    # "that" (demonstrative)
    ["itu", "tu"],  # note: "tu" maps here AND to "tuh" — context-free, we pick the first match
]

# Build lookup: variant → canonical
_COLLOQUIAL_MAP = {}
for group in COLLOQUIAL_GROUPS:
    canonical = group[0]
    for variant in group[1:]:
        if variant not in _COLLOQUIAL_MAP:
            _COLLOQUIAL_MAP[variant] = canonical


def normalize_colloquial(text: str) -> str:
    """
    Apply Indonesian colloquial spelling normalization.
    Input should already be lowercased and punctuation-stripped.
    Returns text with spelling variants mapped to canonical forms.
    """
    words = text.split()
    normalized = []
    for w in words:
        normalized.append(_COLLOQUIAL_MAP.get(w, w))
    return " ".join(normalized)


def get_colloquial_map() -> dict:
    """Return a copy of the colloquial normalization map for inspection."""
    return dict(_COLLOQUIAL_MAP)


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

def compute_wer_lenient(ref_text: str, hyp_text: str) -> float:
    """WER with standard normalization + colloquial spelling normalization."""
    ref_norm = normalize_colloquial(normalize_text_for_eval(ref_text))
    hyp_norm = normalize_colloquial(normalize_text_for_eval(hyp_text))
    ref_words = ref_norm.split()
    hyp_words = hyp_norm.split()
    if not ref_words:
        return 0.0 if not hyp_words else 1.0
    dist = levenshtein_distance(ref_words, hyp_words)
    return dist / len(ref_words)


def compute_cer_lenient(ref_text: str, hyp_text: str) -> float:
    """CER with standard normalization + colloquial spelling normalization."""
    ref_norm = normalize_colloquial(normalize_text_for_eval(ref_text))
    hyp_norm = normalize_colloquial(normalize_text_for_eval(hyp_text))
    ref_chars = list(ref_norm.replace(" ", ""))
    hyp_chars = list(hyp_norm.replace(" ", ""))
    if not ref_chars:
        return 0.0 if not hyp_chars else 1.0
    dist = levenshtein_distance(ref_chars, hyp_chars)
    return dist / len(ref_chars)


if __name__ == "__main__":
    t1 = "Tidak ada ya? Tidak ada lah."
    t2 = "tidak ada ya tidak ada lah"
    print("WER:", compute_wer(t1, t2))
    print("CER:", compute_cer(t1, t2))
    print("\nColloquial map:")
    for variant, canonical in sorted(get_colloquial_map().items()):
        print(f"  {variant} -> {canonical}")
