import unittest
import os
import sys
import json

sys.path.insert(0, os.path.dirname(__file__))
from whisper_to_words import convert_whisper_json_to_words

class TestWhisperToWords(unittest.TestCase):

    def test_subword_merging(self):
        sample = {
            "transcription": [
                {
                    "offsets": {"from": 0, "to": 720},
                    "text": " Tidak ada ya?",
                    "tokens": [
                        {"id": 50364, "text": "[_BEG_]", "offsets": {"from": 0, "to": 0}},
                        {"id": 314, "text": " T", "offsets": {"from": 10, "to": 60}},
                        {"id": 327, "text": "id", "offsets": {"from": 60, "to": 180}},
                        {"id": 514, "text": "ak", "offsets": {"from": 190, "to": 310}},
                        {"id": 11063, "text": " ada", "offsets": {"from": 320, "to": 520}},
                        {"id": 2478, "text": " ya", "offsets": {"from": 520, "to": 680}},
                        {"id": 30, "text": "?", "offsets": {"from": 680, "to": 720}},
                        {"id": 50400, "text": "[_TT_36]", "offsets": {"from": 720, "to": 720}},
                    ]
                }
            ]
        }
        words = convert_whisper_json_to_words(sample)
        self.assertEqual(len(words), 3)
        self.assertEqual(words[0], {"t0": 10, "t1": 310, "text": "Tidak"})
        self.assertEqual(words[1], {"t0": 320, "t1": 520, "text": "ada"})
        self.assertEqual(words[2], {"t0": 520, "t1": 720, "text": "ya?"})

    def test_indonesian_hyphenation_and_apostrophe(self):
        sample = {
            "transcription": [
                {
                    "offsets": {"from": 1000, "to": 2500},
                    "text": " anak-anak Jum'at",
                    "tokens": [
                        {"id": 1, "text": " anak", "offsets": {"from": 1000, "to": 1200}},
                        {"id": 2, "text": "-anak", "offsets": {"from": 1200, "to": 1500}},
                        {"id": 3, "text": " Jum", "offsets": {"from": 1600, "to": 1800}},
                        {"id": 4, "text": "'at", "offsets": {"from": 1800, "to": 2100}},
                        {"id": 5, "text": "!", "offsets": {"from": 2100, "to": 2200}},
                    ]
                }
            ]
        }
        words = convert_whisper_json_to_words(sample)
        self.assertEqual(len(words), 2)
        self.assertEqual(words[0]["text"], "anak-anak")
        self.assertEqual(words[0]["t0"], 1000)
        self.assertEqual(words[0]["t1"], 1500)
        self.assertEqual(words[1]["text"], "Jum'at!")
        self.assertEqual(words[1]["t0"], 1600)
        self.assertEqual(words[1]["t1"], 2200)

    def test_special_tokens_stripping(self):
        sample = {
            "transcription": [
                {
                    "tokens": [
                        {"text": "<|startoftranscript|>", "offsets": {"from": 0, "to": 0}},
                        {"text": "<|id|>", "offsets": {"from": 0, "to": 0}},
                        {"text": "<|transcribe|>", "offsets": {"from": 0, "to": 0}},
                        {"text": " Halo", "offsets": {"from": 100, "to": 400}},
                        {"text": "<|notimestamps|>", "offsets": {"from": 400, "to": 400}},
                    ]
                }
            ]
        }
        words = convert_whisper_json_to_words(sample)
        self.assertEqual(len(words), 1)
        self.assertEqual(words[0], {"t0": 100, "t1": 400, "text": "Halo"})

    def test_duration_floor_positive(self):
        # zero duration token
        sample = {
            "transcription": [
                {
                    "tokens": [
                        {"text": " Cepat", "offsets": {"from": 500, "to": 500}},
                    ]
                }
            ]
        }
        words = convert_whisper_json_to_words(sample)
        self.assertEqual(len(words), 1)
        self.assertGreater(words[0]["t1"], words[0]["t0"])

    def test_real_json_file(self):
        path = "spikes/s3-speech/results/test_run/podcast_small.json"
        if os.path.exists(path):
            with open(path, "r", encoding="utf-8") as f:
                d = json.load(f)
            words = convert_whisper_json_to_words(d)
            self.assertGreater(len(words), 10)
            for w in words:
                self.assertIn("t0", w)
                self.assertIn("t1", w)
                self.assertIn("text", w)
                self.assertGreater(w["t1"], w["t0"])
                self.assertTrue(len(w["text"]) > 0)

if __name__ == "__main__":
    unittest.main()
