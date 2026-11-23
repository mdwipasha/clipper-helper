"""Unit tests for colloquial spelling normalization in accuracy_eval.py"""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(__file__))
from accuracy_eval import (
    normalize_colloquial, normalize_text_for_eval, get_colloquial_map,
    compute_wer, compute_cer, compute_wer_lenient, compute_cer_lenient,
)


class TestColloquialNormalization(unittest.TestCase):

    def test_pronoun_gue_variants(self):
        self.assertEqual(normalize_colloquial("gw ga bohong"), "gue enggak bohong")
        self.assertEqual(normalize_colloquial("gua lagi main"), "gue lagi main")

    def test_pronoun_lu_variants(self):
        self.assertEqual(normalize_colloquial("lo mau kemana"), "lu mau kemana")

    def test_negation_variants(self):
        self.assertEqual(normalize_colloquial("gak tau"), "enggak tau")
        self.assertEqual(normalize_colloquial("ga bisa"), "enggak bisa")
        self.assertEqual(normalize_colloquial("nggak mau"), "enggak mau")

    def test_completion_variants(self):
        self.assertEqual(normalize_colloquial("udh selesai"), "udah selesai")

    def test_conditional_variants(self):
        self.assertEqual(normalize_colloquial("kalo mau"), "kalau mau")

    def test_intensifier_variants(self):
        self.assertEqual(normalize_colloquial("bagus bgt"), "bagus banget")

    def test_canonical_unchanged(self):
        """Words that are already canonical should not change."""
        self.assertEqual(normalize_colloquial("gue enggak mau"), "gue enggak mau")

    def test_non_mapped_words_unchanged(self):
        """Words not in the map should pass through."""
        self.assertEqual(normalize_colloquial("rumah besar"), "rumah besar")

    def test_map_completeness(self):
        """Check that all documented variants are in the map."""
        m = get_colloquial_map()
        self.assertIn("gw", m)
        self.assertIn("gua", m)
        self.assertIn("lo", m)
        self.assertIn("nggak", m)
        self.assertIn("gak", m)
        self.assertIn("ga", m)
        self.assertIn("udh", m)
        self.assertIn("kalo", m)
        self.assertIn("bgt", m)
        self.assertIn("gmn", m)

    def test_lenient_wer_lower_than_strict(self):
        """Lenient WER should be <= strict WER when variants differ."""
        ref = "Gue gak tau, lo mau apa?"
        hyp = "Gw ga tau lu mo apa"
        strict = compute_wer(ref, hyp)
        lenient = compute_wer_lenient(ref, hyp)
        self.assertLessEqual(lenient, strict)

    def test_lenient_cer_lower_than_strict(self):
        ref = "Gue nggak bisa"
        hyp = "Gw ga bisa"
        strict = compute_cer(ref, hyp)
        lenient = compute_cer_lenient(ref, hyp)
        self.assertLessEqual(lenient, strict)

    def test_lenient_wer_zero_on_equivalent(self):
        """If ref and hyp differ only in colloquial spelling, lenient WER = 0."""
        ref = "gue gak tau lo mau apa"
        hyp = "gw ga tau lu mo apa"
        lenient = compute_wer_lenient(ref, hyp)
        self.assertAlmostEqual(lenient, 0.0)

    def test_pipeline_integration(self):
        """Full pipeline: normalize_text_for_eval then normalize_colloquial."""
        ref = "A***** gue gak bohong!"
        hyp = "anjing gw ga bohong"
        ref_norm = normalize_colloquial(normalize_text_for_eval(ref))
        hyp_norm = normalize_colloquial(normalize_text_for_eval(hyp))
        self.assertEqual(ref_norm, hyp_norm)


if __name__ == "__main__":
    unittest.main()
