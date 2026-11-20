"""
Test Suite for Captions, Typography, Safe Zones, and Emoji Rendering (Spike S4)
"""

import os
import subprocess
from caption_generator import generate_ass_script, VALID_BUNDLED_FONTS

FONTS_DIR = os.path.abspath("spikes/s4-render/fonts")
RESULTS_DIR = os.path.abspath("spikes/s4-render/results")
os.makedirs(RESULTS_DIR, exist_ok=True)

FFMPEG_EXE = os.path.abspath(r"spikes\s5-ffmpeg\bin\ffmpeg-release-shared\ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0\bin\ffmpeg.exe")


def test_fonts_and_animations():
    words = [
        {"t0": 0, "t1": 400, "text": "Halo"},
        {"t0": 400, "t1": 800, "text": "teman-teman,"},
        {"t0": 800, "t1": 1300, "text": "selamat"},
        {"t0": 1300, "t1": 1700, "text": "datang"},
        {"t0": 1700, "t1": 2100, "text": "di"},
        {"t0": 2100, "t1": 2600, "text": "livestream"},
        {"t0": 2600, "t1": 3100, "text": "hari"},
        {"t0": 3100, "t1": 3500, "text": "ini!"},
    ]

    for font_name in VALID_BUNDLED_FONTS.keys():
        for anim in ["word_highlight", "pop", "none"]:
            style = {
                "fontFamily": font_name,
                "fontSizePct": 0.045,
                "weight": 700,
                "color": "#FFFFFF",
                "highlightColor": "#FFDD00",
                "outlineColor": "#000000",
                "outlinePx": 4,
                "shadow": True,
                "positionAnchor": "bottom",
                "offsetYPct": 0.18,
                "animation": anim,
                "uppercase": True,
                "maxWordsPerLine": 4,
            }

            ass_str = generate_ass_script(words, style, fonts_dir=FONTS_DIR)
            ass_path = os.path.join(RESULTS_DIR, f"caption_{font_name}_{anim}.ass")
            with open(ass_path, "w", encoding="utf-8") as f:
                f.write(ass_str)

            # Test render in FFmpeg
            posix_ass = ass_path.replace("\\", "/").replace(":", r"\:")
            posix_fonts = FONTS_DIR.replace("\\", "/").replace(":", r"\:")
            vf = f"ass=filename='{posix_ass}':fontsdir='{posix_fonts}'"

            cmd = [
                FFMPEG_EXE, "-y", "-hide_banner",
                "-f", "lavfi", "-i", "color=c=black:s=1080x1920:d=1:r=30",
                "-vf", vf,
                "-c:v", "h264_mf", "-pix_fmt", "yuv420p",
                "-f", "null", "-"
            ]
            res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            if res.returncode != 0:
                print(f"FAILED for {font_name} with {anim}: {res.stderr[-300:]}")
                return False
            else:
                print(f"PASS: Font {font_name:12} | Animation: {anim:15} | Render Verified")

    return True


def test_missing_font_fails_loudly():
    words = [{"t0": 0, "t1": 500, "text": "Test"}]
    style = {"fontFamily": "ComicSans"}
    try:
        generate_ass_script(words, style, fonts_dir=FONTS_DIR)
        print("FAIL: Expected ValueError for unsupported font, but none raised!")
        return False
    except ValueError as e:
        print(f"PASS: Missing font failed loudly with ValueError: {e}")
        return True


def test_emoji_rendering():
    words = [
        {"t0": 0, "t1": 600, "text": "Momen"},
        {"t0": 600, "t1": 1200, "text": "tergokil"},
        {"t0": 1200, "t1": 1800, "text": "🔥"},
        {"t0": 1800, "t1": 2400, "text": "ngakak"},
        {"t0": 2400, "t1": 3000, "text": "😂"},
    ]
    style = {
        "fontFamily": "Montserrat",
        "fontSizePct": 0.05,
        "weight": 700,
        "color": "#FFFFFF",
        "highlightColor": "#FFDD00",
        "outlineColor": "#000000",
        "outlinePx": 4,
        "shadow": True,
        "positionAnchor": "bottom",
        "offsetYPct": 0.20,
        "animation": "none",
        "uppercase": False,
        "maxWordsPerLine": 5,
    }
    ass_str = generate_ass_script(words, style, fonts_dir=FONTS_DIR)
    ass_path = os.path.join(RESULTS_DIR, "caption_emoji_test.ass")
    with open(ass_path, "w", encoding="utf-8") as f:
        f.write(ass_str)

    posix_ass = ass_path.replace("\\", "/").replace(":", r"\:")
    posix_fonts = FONTS_DIR.replace("\\", "/").replace(":", r"\:")
    vf = f"ass=filename='{posix_ass}':fontsdir='{posix_fonts}'"

    cmd = [
        FFMPEG_EXE, "-y", "-hide_banner",
        "-f", "lavfi", "-i", "color=c=black:s=1080x1920:d=1:r=30",
        "-vf", vf,
        "-c:v", "h264_mf", "-pix_fmt", "yuv420p",
        "-f", "null", "-"
    ]
    res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    print("Emoji render exit code:", res.returncode)
    # Check stderr for font warnings or glyph missing
    font_warnings = [line for line in res.stderr.splitlines() if "glyph" in line.lower() or "font" in line.lower() or "missing" in line.lower()]
    print("Emoji render warnings/notes in stderr:", font_warnings[:5])
    return res.returncode == 0


if __name__ == "__main__":
    print("--- Running Font & Animation Matrix Tests ---")
    ok1 = test_fonts_and_animations()
    print("\n--- Running Missing Font Loud Failure Test ---")
    ok2 = test_missing_font_fails_loudly()
    print("\n--- Running Emoji Behavior Test ---")
    ok3 = test_emoji_rendering()

    if ok1 and ok2 and ok3:
        print("\nALL CAPTION & TYPOGRAPHY TESTS PASSED!")
    else:
        print("\nSOME TESTS FAILED!")
