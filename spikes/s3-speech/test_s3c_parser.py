"""Unit tests for s3c_reference_parser.py"""
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(__file__))
from s3c_reference_parser import parse_reference_file, get_reference_text


class TestReferenceParser(unittest.TestCase):

    def _write_temp(self, content: str) -> str:
        f = tempfile.NamedTemporaryFile(mode='w', suffix='.txt', delete=False, encoding='utf-8')
        f.write(content)
        f.close()
        return f.name

    def test_review_tool_format(self):
        content = "[00:03.500] Gw ga bohong\n[00:05.100] Oke oke oke\n"
        path = self._write_temp(content)
        try:
            result = parse_reference_file(path)
            self.assertEqual(result["line_count"], 2)
            self.assertEqual(result["full_text"], "Gw ga bohong Oke oke oke")
            self.assertAlmostEqual(result["max_time_s"], 5.1, places=1)
            self.assertEqual(result["unknown_count"], 0)
        finally:
            os.unlink(path)

    def test_unknown_tokens_excluded(self):
        content = "[00:01.000] Gw ga [?] bohong\n[00:03.000] Oke [?] [?] oke\n"
        path = self._write_temp(content)
        try:
            result = parse_reference_file(path)
            self.assertEqual(result["unknown_count"], 3)
            self.assertNotIn("[?]", result["full_text"])
            self.assertEqual(result["full_text"], "Gw ga bohong Oke oke")
        finally:
            os.unlink(path)

    def test_blank_and_header_lines_skipped(self):
        content = "SAMPLE: Gaming\nMODEL: Medium\nRTF: 1.5\n\n--- SEGMENTS ---\n[00:01.000] Hello\n"
        path = self._write_temp(content)
        try:
            result = parse_reference_file(path)
            self.assertEqual(result["line_count"], 1)
            self.assertEqual(result["full_text"], "Hello")
            self.assertGreater(result["skipped_lines"], 0)
        finally:
            os.unlink(path)

    def test_draft_segment_format(self):
        content = "--- SEGMENTS ---\n[00:00:03,000 --> 00:00:05,000] Suamran tuh kayak gimana\n"
        path = self._write_temp(content)
        try:
            result = parse_reference_file(path)
            self.assertEqual(result["line_count"], 1)
            self.assertAlmostEqual(result["lines"][0]["time_s"], 3.0)
        finally:
            os.unlink(path)

    def test_full_text_block_skipped_when_segments_present(self):
        content = (
            "SAMPLE: Test\n"
            "--- FULL TEXT ---\n"
            "This whole block should be skipped.\n"
            "--- SEGMENTS ---\n"
            "[00:00:01,000 --> 00:00:02,000] Actual segment text\n"
        )
        path = self._write_temp(content)
        try:
            result = parse_reference_file(path)
            self.assertEqual(result["line_count"], 1)
            self.assertEqual(result["full_text"], "Actual segment text")
        finally:
            os.unlink(path)

    def test_time_coverage_detection(self):
        """If timestamps only go to 28 s, max_time_s should reflect that."""
        content = "[00:10.000] Early\n[00:28.500] Late\n"
        path = self._write_temp(content)
        try:
            result = parse_reference_file(path)
            self.assertAlmostEqual(result["max_time_s"], 28.5)
        finally:
            os.unlink(path)

    def test_file_not_found(self):
        with self.assertRaises(FileNotFoundError):
            parse_reference_file("/nonexistent/path.txt")


if __name__ == "__main__":
    unittest.main()
