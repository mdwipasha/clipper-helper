import unittest
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from whisper_progress_parser import WhisperProgressParser

class TestWhisperProgressParser(unittest.TestCase):

    def test_parse_real_progress_lines(self):
        parser = WhisperProgressParser(total_audio_seconds=60.0, prior_rtf=1.0)
        
        # Test non-progress lines
        self.assertIsNone(parser.parse_line("whisper_init: loading model..."))
        self.assertIsNone(parser.parse_line("[00:00:00.000 --> 00:00:02.000] Halo dunia"))

        # Test progress line
        ev1 = parser.parse_line("whisper_print_progress_callback: progress =  10%", current_time=100.0)
        self.assertIsNotNone(ev1)
        self.assertEqual(ev1["percent"], 10)
        self.assertEqual(ev1["audio_seconds_done"], 6.0)
        self.assertEqual(ev1["total_audio_seconds"], 60.0)
        self.assertGreater(ev1["eta_seconds"], 0)

        ev2 = parser.parse_line("whisper_print_progress_callback: progress =  50%", current_time=115.0)
        self.assertIsNotNone(ev2)
        self.assertEqual(ev2["percent"], 50)
        self.assertEqual(ev2["audio_seconds_done"], 30.0)

        ev3 = parser.parse_line("whisper_print_progress_callback: progress = 100%", current_time=130.0)
        self.assertIsNotNone(ev3)
        self.assertEqual(ev3["percent"], 100)
        self.assertEqual(ev3["audio_seconds_done"], 60.0)
        self.assertEqual(ev3["eta_seconds"], 0.0)

    def test_monotonic_progress(self):
        parser = WhisperProgressParser(total_audio_seconds=60.0)
        ev1 = parser.parse_line("progress = 40%", current_time=10.0)
        self.assertEqual(ev1["percent"], 40)
        
        # Lower percent should be ignored
        ev_lower = parser.parse_line("progress = 30%", current_time=12.0)
        self.assertIsNone(ev_lower)
        self.assertEqual(parser.current_percent, 40)

    def test_prior_rate_blending(self):
        # Prior RTF 0.5 means processing 2.0 audio seconds per wall second
        parser = WhisperProgressParser(total_audio_seconds=60.0, prior_rtf=0.5)
        self.assertAlmostEqual(parser.prior_rate, 2.0)
        
        ev = parser.parse_line("progress = 5%", current_time=1.0)
        # At 5%, D/W = 3/60 = 0.05. w_blend = 0.05 / 0.15 = 0.333
        self.assertGreater(ev["rate_audio_sec_per_wall_sec"], 0)
        self.assertGreater(ev["eta_seconds"], 0)

if __name__ == "__main__":
    unittest.main()
