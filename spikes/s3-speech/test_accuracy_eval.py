import unittest
from accuracy_eval import normalize_text_for_eval, levenshtein_distance, compute_wer, compute_cer

class TestAccuracyEval(unittest.TestCase):
    def test_normalization_basic(self):
        raw = "Halo, Dunia! Apa kabar?"
        expected = "halo dunia apa kabar"
        self.assertEqual(normalize_text_for_eval(raw), expected)

    def test_normalization_hyphens_reduplication(self):
        raw = "anak-anak bermain bersama-sama"
        expected = "anak anak bermain bersama sama"
        self.assertEqual(normalize_text_for_eval(raw), expected)

    def test_normalization_apostrophes(self):
        raw = "Hari Jum'at ng'gak masuk sekolah."
        expected = "hari jumat nggak masuk sekolah"
        self.assertEqual(normalize_text_for_eval(raw), expected)

    def test_normalization_asterisks(self):
        raw = "Kenapa sih a***** gua di bunuh?"
        expected = "kenapa sih anjing gua di bunuh"
        self.assertEqual(normalize_text_for_eval(raw), expected)

    def test_levenshtein_distance(self):
        self.assertEqual(levenshtein_distance([], []), 0)
        self.assertEqual(levenshtein_distance(["a"], []), 1)
        self.assertEqual(levenshtein_distance([], ["b"]), 1)
        self.assertEqual(levenshtein_distance(["a", "b", "c"], ["a", "b", "c"]), 0)
        self.assertEqual(levenshtein_distance(["a", "b", "c"], ["a", "d", "c"]), 1) # substitution
        self.assertEqual(levenshtein_distance(["a", "b", "c"], ["a", "c"]), 1)      # deletion
        self.assertEqual(levenshtein_distance(["a", "c"], ["a", "b", "c"]), 1)      # insertion

    def test_wer_identical(self):
        ref = "ini adalah tes transkrip"
        hyp = "ini adalah tes transkrip"
        self.assertAlmostEqual(compute_wer(ref, hyp), 0.0)

    def test_wer_substitution(self):
        ref = "ini adalah tes transkrip"
        hyp = "ini adalah uji transkrip"
        self.assertAlmostEqual(compute_wer(ref, hyp), 0.25)

    def test_wer_deletion(self):
        ref = "satu dua tiga empat"
        hyp = "satu dua empat"
        self.assertAlmostEqual(compute_wer(ref, hyp), 0.25)

    def test_wer_insertion(self):
        ref = "satu dua empat"
        hyp = "satu dua tiga empat"
        self.assertAlmostEqual(compute_wer(ref, hyp), 1.0 / 3.0)

    def test_wer_empty(self):
        self.assertAlmostEqual(compute_wer("", ""), 0.0)
        self.assertAlmostEqual(compute_wer("kata", ""), 1.0)
        self.assertAlmostEqual(compute_wer("", "kata"), 1.0)

    def test_cer(self):
        ref = "halo"
        hyp = "halo"
        self.assertAlmostEqual(compute_cer(ref, hyp), 0.0)
        hyp2 = "hallo"
        self.assertAlmostEqual(compute_cer(ref, hyp2), 0.25)

if __name__ == "__main__":
    unittest.main()
