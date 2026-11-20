"""
FFmpeg Progress Parser for Clipper Helper (Spike S4)
Parses '-progress pipe:1' key=value output stream according to PROGRESS_AND_ETA §4.
Calculates frames, percentage, fps, speed multiplier, elapsed time, and ETA.
"""

from typing import Any, Dict, Optional
import unittest


class RenderProgressParser:
    def __init__(self, total_frames: int, expected_duration_s: float):
        self.total_frames = max(1, total_frames)
        self.expected_duration_us = int(expected_duration_s * 1_000_000)
        self.current_state: Dict[str, Any] = {
            "frame": 0,
            "fps": 0.0,
            "speed": 0.0,
            "out_time_us": 0,
            "percent": 0.0,
            "bitrate_kbits": 0.0,
            "progress": "starting",
            "eta_s": None
        }

    def parse_line(self, line: str) -> Optional[Dict[str, Any]]:
        line = line.strip()
        if not line or "=" not in line:
            return None

        key, val = line.split("=", 1)
        key = key.strip()
        val = val.strip()

        if key == "frame":
            try:
                self.current_state["frame"] = int(val)
                self.current_state["percent"] = min(100.0, round((self.current_state["frame"] / self.total_frames) * 100.0, 1))
            except ValueError:
                pass
        elif key == "fps":
            try:
                self.current_state["fps"] = float(val)
            except ValueError:
                pass
        elif key == "speed":
            try:
                # Value is typically '1.52x' or 'N/A'
                val_clean = val.rstrip("x")
                self.current_state["speed"] = float(val_clean)
            except ValueError:
                self.current_state["speed"] = 0.0
        elif key == "out_time_us":
            try:
                self.current_state["out_time_us"] = int(val)
            except ValueError:
                pass
        elif key == "bitrate":
            try:
                # e.g. '4320.5kbits/s'
                val_clean = val.replace("kbits/s", "").strip()
                self.current_state["bitrate_kbits"] = float(val_clean)
            except ValueError:
                pass
        elif key == "progress":
            self.current_state["progress"] = val
            # Calculate simple ETA when progress packet is complete
            rem_frames = max(0, self.total_frames - self.current_state["frame"])
            fps = self.current_state["fps"]
            if fps > 0.5:
                self.current_state["eta_s"] = round(rem_frames / fps, 1)
            elif self.current_state["speed"] > 0.05:
                rem_us = max(0, self.expected_duration_us - self.current_state["out_time_us"])
                rem_time_s = rem_us / 1_000_000.0
                self.current_state["eta_s"] = round(rem_time_s / self.current_state["speed"], 1)
            else:
                self.current_state["eta_s"] = None

            # Return copy of complete snapshot
            return dict(self.current_state)

        return None


class TestProgressParser(unittest.TestCase):
    def test_parse_progress_block(self):
        parser = RenderProgressParser(total_frames=1800, expected_duration_s=60.0)

        raw_block = """
frame=450
fps=60.0
stream_0_0_q=-0.0
bitrate=4450.2kbits/s
total_size=12543200
out_time_us=15000000
out_time_ms=15000000
out_time=00:00:15.000000
dup_frames=0
drop_frames=0
speed=2.00x
progress=continue
"""
        snapshot = None
        for line in raw_block.strip().splitlines():
            res = parser.parse_line(line)
            if res:
                snapshot = res

        self.assertIsNotNone(snapshot)
        self.assertEqual(snapshot["frame"], 450)
        self.assertEqual(snapshot["percent"], 25.0)
        self.assertEqual(snapshot["fps"], 60.0)
        self.assertEqual(snapshot["speed"], 2.0)
        self.assertEqual(snapshot["progress"], "continue")
        # 1800 - 450 = 1350 frames remaining / 60 fps = 22.5 s
        self.assertEqual(snapshot["eta_s"], 22.5)

    def test_end_progress(self):
        parser = RenderProgressParser(total_frames=1800, expected_duration_s=60.0)
        raw_end = """
frame=1800
fps=58.5
speed=1.95x
out_time_us=60000000
progress=end
"""
        snapshot = None
        for line in raw_end.strip().splitlines():
            res = parser.parse_line(line)
            if res:
                snapshot = res

        self.assertIsNotNone(snapshot)
        self.assertEqual(snapshot["frame"], 1800)
        self.assertEqual(snapshot["percent"], 100.0)
        self.assertEqual(snapshot["progress"], "end")
        self.assertEqual(snapshot["eta_s"], 0.0)


if __name__ == "__main__":
    unittest.main()
