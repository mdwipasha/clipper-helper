"""
Golden-File & Snapshot Tests for render_builder.py
Validates CLI argument generation, filtergraph construction, and Windows path escaping.
"""

import json
import os
import unittest
from render_builder import build_ffmpeg_args, escape_filter_path


class TestRenderBuilder(unittest.TestCase):
    def setUp(self):
        self.dummy_video = "C:/test/sample.mp4"
        self.dummy_logo = "C:/test/logo.png"
        self.dummy_ass = "C:/test/caption.ass"
        self.dummy_fonts = "C:/test/fonts"

        self.base_comp = {
            "schemaVersion": 1,
            "id": "proj-001",
            "canvas": {"width": 1080, "height": 1920, "fps": 30},
            "sources": [
                {
                    "id": "src-1",
                    "provider": "youtube",
                    "videoId": "abc12345",
                    "path": self.dummy_video,
                    "absoluteStartMs": 1800000,
                    "durationMs": 60000,
                    "width": 1920,
                    "height": 1080,
                    "fps": 60,
                }
            ],
            "timeline": [{"id": "seg-1", "sourceId": "src-1", "inMs": 0, "outMs": 60000}],
            "layout": {"mode": "fill", "crop": {"x": 0.25, "y": 0.0, "w": 0.5, "h": 1.0}},
            "captions": {"enabled": False, "language": "id", "words": []},
            "overlays": [],
            "audio": {"gainDb": 0.0, "normalize": {"enabled": True, "targetLufs": -16.0}},
            "export": {"preset": "standard", "audioKbps": 192},
        }

    def test_windows_path_escaping(self):
        raw_path = r"F:\coding\clipper-helper\fonts\Montserrat.ttf"
        escaped = escape_filter_path(raw_path)
        self.assertEqual(escaped, r"F\:/coding/clipper-helper/fonts/Montserrat.ttf")
        self.assertNotIn("\\", escaped.replace(r"\:", ""))

    def test_layout_fill(self):
        comp = dict(self.base_comp)
        comp["layout"] = {"mode": "fill", "crop": {"x": 0.25, "y": 0.0, "w": 0.5, "h": 1.0}}
        args = build_ffmpeg_args(comp, encoder="h264_mf", output_path="out.mp4")

        args_str = " ".join(args)
        self.assertIn("crop=w=iw*0.5000:h=ih*1.0000:x=iw*0.2500:y=ih*0.0000,scale=1080:1920", args_str)
        self.assertIn("-c:v h264_mf", args_str)
        self.assertIn("-color_primaries bt709", args_str)
        self.assertIn("-movflags +faststart", args_str)

    def test_layout_blur_fit_uses_gblur_not_boxblur(self):
        comp = dict(self.base_comp)
        comp["layout"] = {"mode": "blur_fit", "blur": 3.0}
        args = build_ffmpeg_args(comp, encoder="h264_amf", output_path="out.mp4")

        args_str = " ".join(args)
        # Must contain gblur, strictly must not contain boxblur (GPL-only)
        self.assertIn("gblur=sigma=30:steps=2", args_str)
        self.assertNotIn("boxblur", args_str)
        self.assertIn("overlay=(W-w)/2:(H-h)/2", args_str)

    def test_layout_split(self):
        comp = dict(self.base_comp)
        comp["layout"] = {
            "mode": "split",
            "top": {"x": 0.0, "y": 0.0, "w": 1.0, "h": 0.5},
            "bottom": {"x": 0.0, "y": 0.5, "w": 1.0, "h": 0.5},
            "ratio": 0.55,
        }
        args = build_ffmpeg_args(comp, encoder="h264_mf", output_path="out.mp4")

        args_str = " ".join(args)
        self.assertIn("vstack", args_str)
        self.assertIn("scale=1080:1056", args_str)
        self.assertIn("scale=1080:864", args_str)

    def test_multi_segment_timeline_concat(self):
        comp = dict(self.base_comp)
        comp["timeline"] = [
            {"id": "seg-1", "sourceId": "src-1", "inMs": 5000, "outMs": 15000},
            {"id": "seg-2", "sourceId": "src-1", "inMs": 25000, "outMs": 35000},
        ]
        args = build_ffmpeg_args(comp, encoder="h264_mf", output_path="out.mp4")

        args_str = " ".join(args)
        self.assertIn("trim=start=5.000:end=15.000,setpts=PTS-STARTPTS", args_str)
        self.assertIn("trim=start=25.000:end=35.000,setpts=PTS-STARTPTS", args_str)
        self.assertIn("concat=n=2:v=1:a=1", args_str)

    def test_captions_and_overlays(self):
        comp = dict(self.base_comp)
        comp["captions"] = {"enabled": True, "language": "id", "words": []}
        comp["overlays"] = [
            {"type": "logo", "anchor": "top_right", "marginPct": 0.05, "widthPct": 0.15, "opacity": 0.9},
            {"type": "text", "text": "Highlight Hari Ini", "anchor": "top_center", "marginPct": 0.08, "sizePct": 0.035, "color": "yellow"},
        ]

        # Mock existing files
        real_ass = os.path.abspath("spikes/s5-ffmpeg/results/test_caption.ass")
        real_fonts = os.path.abspath("spikes/s4-render/fonts")

        args = build_ffmpeg_args(
            comp,
            encoder="h264_amf",
            output_path="out.mp4",
            caption_ass_path=real_ass,
            fonts_dir=real_fonts,
        )

        args_str = " ".join(args)
        self.assertIn("ass=filename=", args_str)
        self.assertIn("fontsdir=", args_str)
        self.assertIn("drawtext=text='Highlight Hari Ini'", args_str)
        self.assertIn("loudnorm=I=-16.0:TP=-1.5:LRA=11", args_str)


if __name__ == "__main__":
    unittest.main()
