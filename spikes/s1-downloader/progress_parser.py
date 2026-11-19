"""
Spike S1b — progress_parser.py
Pure function progress line parser for yt-dlp & FFmpeg progress output.
Handles:
- Standard FFmpeg output: time=HH:MM:SS.ss, speed=X.XXx, frame=..., size=..., bitrate=...
- yt-dlp download lines: [download] XX% of ~XXMiB at XXMiB/s ETA MM:SS
- Edge cases: time=N/A, speed=N/A, missing speed, carriage return '\r' in chunk
- Durations > 10 hours (e.g. 12:34:56.78)
- Percentage and ETA computation relative to target duration
"""
import re
from dataclasses import dataclass
from typing import Optional

@dataclass
class ProgressUpdate:
    time_s: Optional[float] = None
    speed_ratio: Optional[float] = None
    percent: Optional[float] = None
    eta_s: Optional[float] = None
    frame: Optional[int] = None
    size_bytes: Optional[int] = None
    bitrate_kbps: Optional[float] = None
    fps: Optional[float] = None
    raw_line: str = ""

def parse_time_str(time_str: str) -> Optional[float]:
    """Parse HH:MM:SS.ffffff or MM:SS.ffffff to float seconds."""
    if not time_str or time_str.strip() in ("N/A", "none", "unknown"):
        return None
    time_str = time_str.strip()
    parts = time_str.split(":")
    try:
        if len(parts) == 3:
            h = float(parts[0])
            m = float(parts[1])
            s = float(parts[2])
            return round(h * 3600 + m * 60 + s, 3)
        elif len(parts) == 2:
            m = float(parts[0])
            s = float(parts[1])
            return round(m * 60 + s, 3)
        elif len(parts) == 1:
            return round(float(parts[0]), 3)
    except (ValueError, IndexError):
        return None
    return None

def parse_speed_str(speed_str: str) -> Optional[float]:
    """Parse speed string like '1.76x' or '0.5x' to float multiplier."""
    if not speed_str or speed_str.strip() in ("N/A", "none", ""):
        return None
    s = speed_str.strip().rstrip("xX").strip()
    try:
        val = float(s)
        return val if val > 0 else None
    except ValueError:
        return None

def parse_size_str(size_str: str, unit_str: str = "") -> Optional[int]:
    """Parse size like '3584KiB' or '6.16MiB' to bytes."""
    if not size_str:
        return None
    m = re.match(r"^([0-9.]+)\s*([A-Za-z]+)?$", size_str.strip())
    if not m:
        return None
    val = float(m.group(1))
    unit = (m.group(2) or unit_str).lower()
    multipliers = {
        "b": 1,
        "kib": 1024,
        "kb": 1000,
        "mib": 1024 ** 2,
        "mb": 1000 ** 2,
        "gib": 1024 ** 3,
        "gb": 1000 ** 3,
    }
    mult = multipliers.get(unit, 1)
    return int(val * mult)

def parse_progress_line(line: str, target_duration_s: Optional[float] = None) -> Optional[ProgressUpdate]:
    """
    Parse a single progress line (or line containing \r updates).
    Returns ProgressUpdate dataclass or None if no progress found.
    """
    if not line:
        return None

    # Handle carriage return updates: take the last non-empty segment
    if "\r" in line:
        segments = [s.strip() for s in line.split("\r") if s.strip()]
        if not segments:
            return None
        line = segments[-1]

    line = line.strip()

    # Pattern 1: FFmpeg output
    # e.g.: frame= 1850 fps= 98 q=31.0 size= 3584KiB time=00:03:08.50 bitrate= 844.7kbits/s speed=1.76x
    if "time=" in line:
        update = ProgressUpdate(raw_line=line)

        # Parse time
        m_time = re.search(r"time=\s*([0-9:.]+)", line)
        if m_time:
            update.time_s = parse_time_str(m_time.group(1))

        # Parse speed
        m_speed = re.search(r"speed=\s*([0-9.]+x|N/A)", line)
        if m_speed:
            update.speed_ratio = parse_speed_str(m_speed.group(1))

        # Parse frame
        m_frame = re.search(r"frame=\s*([0-9]+)", line)
        if m_frame:
            update.frame = int(m_frame.group(1))

        # Parse fps
        m_fps = re.search(r"fps=\s*([0-9.]+)", line)
        if m_fps:
            try:
                update.fps = float(m_fps.group(1))
            except ValueError:
                pass

        # Parse size
        m_size = re.search(r"size=\s*([0-9.]+[A-Za-z]+)", line)
        if m_size:
            update.size_bytes = parse_size_str(m_size.group(1))

        # Parse bitrate
        m_br = re.search(r"bitrate=\s*([0-9.]+)\s*kbits/s", line)
        if m_br:
            try:
                update.bitrate_kbps = float(m_br.group(1))
            except ValueError:
                pass

        # Derived metrics
        if update.time_s is not None and target_duration_s and target_duration_s > 0:
            pct = (update.time_s / target_duration_s) * 100.0
            update.percent = round(min(max(pct, 0.0), 100.0), 1)

            if update.speed_ratio and update.speed_ratio > 0 and update.time_s < target_duration_s:
                rem = (target_duration_s - update.time_s) / update.speed_ratio
                update.eta_s = round(max(rem, 0.0), 1)

        return update

    # Pattern 2: yt-dlp native download line
    # e.g.: [download]  42.0% of ~118.00MiB at  6.20MiB/s ETA 00:12
    # e.g.: [download] 100% of    6.16MiB in 00:00:37 at 167.00KiB/s
    if "[download]" in line and "%" in line:
        update = ProgressUpdate(raw_line=line)
        m_pct = re.search(r"\[download\]\s+([0-9.]+)%", line)
        if m_pct:
            update.percent = float(m_pct.group(1))

        m_eta = re.search(r"ETA\s+([0-9:]+)", line)
        if m_eta:
            update.eta_s = parse_time_str(m_eta.group(1))

        m_sz = re.search(r"of\s+~?([0-9.]+[A-Za-z]+)", line)
        if m_sz:
            update.size_bytes = parse_size_str(m_sz.group(1))

        return update

    return None
