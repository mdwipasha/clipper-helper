"""
Unit tests for progress_parser.py (Spike S1b Item 8).
Validates pure parsing function against real yt-dlp and FFmpeg progress lines,
including edge cases: time=N/A, speed=N/A, carriage returns, >10 hour durations.
"""
import unittest
from progress_parser import parse_progress_line, parse_time_str, parse_speed_str, parse_size_str, ProgressUpdate

class TestProgressParser(unittest.TestCase):

    def test_standard_ffmpeg_line(self):
        line = "frame= 1850 fps= 98.5 q=31.0 size= 3584KiB time=00:03:08.50 bitrate= 844.7kbits/s speed=1.76x"
        res = parse_progress_line(line, target_duration_s=300.0)
        self.assertIsNotNone(res)
        self.assertEqual(res.frame, 1850)
        self.assertEqual(res.fps, 98.5)
        self.assertEqual(res.size_bytes, 3584 * 1024)
        self.assertEqual(res.time_s, 188.5)
        self.assertEqual(res.bitrate_kbps, 844.7)
        self.assertEqual(res.speed_ratio, 1.76)
        # Percent: 188.5 / 300 * 100 = 62.8%
        self.assertEqual(res.percent, 62.8)
        # ETA: (300 - 188.5) / 1.76 = 63.35 -> 63.4s
        self.assertEqual(res.eta_s, 63.4)

    def test_time_na_speed_na(self):
        line = "frame=    0 fps=0.0 q=0.0 size=       0KiB time=N/A bitrate=N/A speed=N/A"
        res = parse_progress_line(line, target_duration_s=60.0)
        self.assertIsNotNone(res)
        self.assertEqual(res.frame, 0)
        self.assertIsNone(res.time_s)
        self.assertIsNone(res.speed_ratio)
        self.assertIsNone(res.percent)
        self.assertIsNone(res.eta_s)

    def test_carriage_return_multiline_chunk(self):
        # yt-dlp/ffmpeg outputs progress with carriage return '\r' in stdout/stderr
        chunk = "frame=  100 fps= 50 size= 500KiB time=00:00:10.00 speed=1.0x\rframe=  200 fps= 52 size= 1000KiB time=00:00:20.00 speed=1.2x"
        res = parse_progress_line(chunk, target_duration_s=60.0)
        self.assertIsNotNone(res)
        self.assertEqual(res.frame, 200)
        self.assertEqual(res.time_s, 20.0)
        self.assertEqual(res.speed_ratio, 1.2)
        self.assertEqual(res.percent, 33.3)

    def test_duration_over_10_hours(self):
        line = "frame=1234567 fps= 60 size= 123456KiB time=12:34:56.78 bitrate=1500.0kbits/s speed=2.50x"
        # 12*3600 + 34*60 + 56.78 = 43200 + 2040 + 56.78 = 45296.78
        res = parse_progress_line(line, target_duration_s=50000.0)
        self.assertIsNotNone(res)
        self.assertEqual(res.time_s, 45296.78)
        self.assertEqual(res.speed_ratio, 2.50)
        self.assertAlmostEqual(res.percent, round((45296.78 / 50000.0) * 100, 1))

    def test_ytdlp_native_download_line(self):
        line = "[download]  42.0% of ~118.00MiB at  6.20MiB/s ETA 00:12"
        res = parse_progress_line(line)
        self.assertIsNotNone(res)
        self.assertEqual(res.percent, 42.0)
        self.assertEqual(res.eta_s, 12.0)
        self.assertEqual(res.size_bytes, int(118.0 * 1024 * 1024))

    def test_non_progress_lines(self):
        lines = [
            "[youtube] Extracting URL: https://www.youtube.com/watch?v=QOJIILaJKNc",
            "[youtube] QOJIILaJKNc: Downloading webpage",
            "[download] Destination: output.mp4",
            "",
            "   ",
            "\r\r\n"
        ]
        for l in lines:
            res = parse_progress_line(l)
            self.assertIsNone(res)

    def test_helpers(self):
        self.assertEqual(parse_time_str("00:01:05.500"), 65.5)
        self.assertEqual(parse_time_str("01:05.500"), 65.5)
        self.assertIsNone(parse_time_str("N/A"))
        self.assertIsNone(parse_time_str(""))
        
        self.assertEqual(parse_speed_str("1.5x"), 1.5)
        self.assertEqual(parse_speed_str(" 2.4X "), 2.4)
        self.assertIsNone(parse_speed_str("N/A"))

        self.assertEqual(parse_size_str("1024KiB"), 1024 * 1024)
        self.assertEqual(parse_size_str("10MB"), 10 * 1000 * 1000)
        self.assertEqual(parse_size_str("1.5MiB"), int(1.5 * 1024 * 1024))

if __name__ == "__main__":
    unittest.main()
