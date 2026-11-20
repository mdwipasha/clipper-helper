"""
ASS Caption Generator & Typography Suite for Clipper Helper (Spike S4)
Converts word-level transcription timings into styled Advanced SubStation Alpha (.ass) scripts.
Supports word_highlight (karaoke), pop animation, and safe-zone positioning.
"""

import os
from typing import Any, Dict, List, Optional

VALID_BUNDLED_FONTS = {
    "Montserrat": "Montserrat-Bold.ttf",
    "Poppins": "Poppins-Bold.ttf",
    "Inter": "Inter-Bold.ttf",
    "Anton": "Anton-Regular.ttf",
}


def format_ass_time(ms: int) -> str:
    """Formats milliseconds into ASS timestamp format: H:MM:SS.cs"""
    hours = ms // 3600000
    ms %= 3600000
    minutes = ms // 60000
    ms %= 60000
    seconds = ms // 1000
    centiseconds = (ms % 1000) // 10
    return f"{hours}:{minutes:02d}:{seconds:02d}.{centiseconds:02d}"


def color_to_ass(hex_color: str) -> str:
    """Converts #RRGGBB or #AARRGGBB hex color to ASS &HAABBGGRR format."""
    hex_color = hex_color.lstrip("#")
    if len(hex_color) == 6:
        r, g, b = hex_color[0:2], hex_color[2:4], hex_color[4:6]
        return f"&H00{b}{g}{r}&".upper()
    elif len(hex_color) == 8:
        a, r, g, b = hex_color[0:2], hex_color[2:4], hex_color[4:6], hex_color[6:8]
        return f"&H{a}{b}{g}{r}&".upper()
    # Default white
    return "&H00FFFFFF&"


def generate_ass_script(
    words: List[Dict[str, Any]],
    style: Dict[str, Any],
    fonts_dir: str,
    canvas_w: int = 1080,
    canvas_h: int = 1920,
    title: str = "ClipperHelper Captions"
) -> str:
    """
    Generates ASS script string from word timings and caption style.
    Validates font availability and fails loudly if missing.
    """
    font_family = style.get("fontFamily", "Montserrat")
    if font_family not in VALID_BUNDLED_FONTS:
        raise ValueError(
            f"Unsupported font family '{font_family}'. Must be one of {list(VALID_BUNDLED_FONTS.keys())}."
        )

    expected_font_file = os.path.join(fonts_dir, VALID_BUNDLED_FONTS[font_family])
    if not os.path.exists(expected_font_file):
        raise FileNotFoundError(
            f"Bundled font file '{expected_font_file}' not found in fonts directory '{fonts_dir}'. "
            f"Cannot render captions."
        )

    # Style parameters
    font_size_pct = style.get("fontSizePct", 0.045)  # 4.5% of canvas height ≈ 86px
    font_size = int(canvas_h * font_size_pct)
    font_weight = style.get("weight", 700)
    bold = -1 if font_weight >= 600 else 0

    primary_color = color_to_ass(style.get("color", "#FFFFFF"))
    highlight_color = color_to_ass(style.get("highlightColor", "#FFD700"))  # Yellow
    outline_color = color_to_ass(style.get("outlineColor", "#000000"))
    outline_px = style.get("outlinePx", 4)
    shadow_px = 2 if style.get("shadow", True) else 0

    position_anchor = style.get("positionAnchor", "bottom")
    offset_y_pct = style.get("offsetYPct", 0.15)
    margin_v = int(canvas_h * offset_y_pct)
    margin_l = int(canvas_w * 0.08)
    margin_r = margin_l

    # Alignment code: 2 = bottom-center, 5 = top-center, 8 = center
    if position_anchor == "top":
        alignment = 8
    elif position_anchor == "center":
        alignment = 5
    else:
        alignment = 2

    animation = style.get("animation", "word_highlight")
    uppercase = style.get("uppercase", False)
    max_words_per_line = style.get("maxWordsPerLine", 4)

    # Group words into subtitle lines (chunks of max_words_per_line)
    lines: List[List[Dict[str, Any]]] = []
    current_line: List[Dict[str, Any]] = []

    for w in words:
        current_line.append(w)
        if len(current_line) >= max_words_per_line or w.get("text", "").endswith((".", "!", "?")):
            lines.append(current_line)
            current_line = []
    if current_line:
        lines.append(current_line)

    dialogue_events: List[str] = []

    for line in lines:
        if not line:
            continue
        line_start = line[0]["t0"]
        line_end = line[-1]["t1"]

        # If animation is none, display static phrase
        if animation == "none":
            text_tokens = [w["text"].upper() if uppercase else w["text"] for w in line]
            text_str = " ".join(text_tokens)
            dialogue_events.append(
                f"Dialogue: 0,{format_ass_time(line_start)},{format_ass_time(line_end)},Default,,0,0,0,,{text_str}"
            )

        elif animation == "word_highlight":
            # For each word in the line, emit a segment where that word is highlighted
            for i, current_word in enumerate(line):
                w_start = current_word["t0"]
                w_end = current_word["t1"]
                # Form formatted text
                parts = []
                for j, w in enumerate(line):
                    t = w["text"].upper() if uppercase else w["text"]
                    if j == i:
                        # Highlighted word
                        parts.append(f"{{\\c{highlight_color}\\fscx105\\fscy105}}{t}{{\\c{primary_color}\\fscx100\\fscy100}}")
                    else:
                        parts.append(t)
                text_str = " ".join(parts)
                dialogue_events.append(
                    f"Dialogue: 0,{format_ass_time(w_start)},{format_ass_time(w_end)},Default,,0,0,0,,{text_str}"
                )

        elif animation == "pop":
            # Pop animation: current word pops in scale
            for i, current_word in enumerate(line):
                w_start = current_word["t0"]
                w_end = current_word["t1"]
                parts = []
                for j, w in enumerate(line):
                    t = w["text"].upper() if uppercase else w["text"]
                    if j == i:
                        parts.append(f"{{\\t(0,100,\\fscx115\\fscy115)\\t(100,200,\\fscx100\\fscy100)\\c{highlight_color}}}{t}{{\\c{primary_color}}}")
                    else:
                        parts.append(t)
                text_str = " ".join(parts)
                dialogue_events.append(
                    f"Dialogue: 0,{format_ass_time(w_start)},{format_ass_time(w_end)},Default,,0,0,0,,{text_str}"
                )

    ass_content = f"""[Script Info]
; Script generated by Clipper Helper (Spike S4)
Title: {title}
ScriptType: v4.00+
WrapStyle: 0
ScaledBorderAndShadow: yes
YCbCr Matrix: TV.709
PlayResX: {canvas_w}
PlayResY: {canvas_h}

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,{font_family},{font_size},{primary_color},&H000000FF&,{outline_color},&H80000000&,{bold},0,0,0,100,100,0,0,1,{outline_px},{shadow_px},{alignment},{margin_l},{margin_r},{margin_v},1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
""" + "\n".join(dialogue_events) + "\n"

    return ass_content
