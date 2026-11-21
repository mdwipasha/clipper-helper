import re
import time

PROGRESS_REGEX = re.compile(r"progress\s*=\s*(\d+)%", re.IGNORECASE)

class WhisperProgressParser:
    """
    Pure parser for whisper.cpp stdout/stderr lines with --print-progress.
    Follows PROGRESS_AND_ETA.md contract:
    - Audio duration is known in advance (W = total_audio_seconds)
    - whisper.cpp emits lines like:
        "whisper_print_progress_callback: progress =  10%"
    - Emits progress events with:
        percent (0..100)
        audio_seconds_done
        rate (audio_seconds / wall_second, smoothed)
        eta_seconds
    """
    def __init__(self, total_audio_seconds=60.0, prior_rtf=1.0):
        self.total_audio_seconds = float(total_audio_seconds)
        # prior rate in audio_seconds per wall_second = 1.0 / prior_rtf
        self.prior_rate = 1.0 / max(0.01, float(prior_rtf))
        self.current_percent = 0
        self.audio_seconds_done = 0.0
        self.start_time = None
        self.last_update_time = None
        self.last_audio_seconds_done = 0.0
        self.smoothed_rate = self.prior_rate
        self.alpha = 0.2  # EMA factor

    def parse_line(self, line: str, current_time: float = None):
        """
        Parses a single line from whisper.cpp.
        Returns a progress event dict if progress was updated, or None.
        """
        match = PROGRESS_REGEX.search(line)
        if not match:
            return None

        pct = int(match.group(1))
        # Ensure monotonic percent
        if pct < self.current_percent:
            return None
        self.current_percent = pct

        now = current_time if current_time is not None else time.time()
        if self.start_time is None:
            self.start_time = now
            self.last_update_time = now

        self.audio_seconds_done = (pct / 100.0) * self.total_audio_seconds
        elapsed = max(0.001, now - self.start_time)

        # Compute live instantaneous rate over this step
        dt = now - self.last_update_time
        if dt >= 0.5:
            d_work = self.audio_seconds_done - self.last_audio_seconds_done
            inst_rate = d_work / dt
            self.smoothed_rate = self.alpha * inst_rate + (1.0 - self.alpha) * self.smoothed_rate
            self.last_update_time = now
            self.last_audio_seconds_done = self.audio_seconds_done

        # Blend with prior per PROGRESS_AND_ETA §4.2:
        # w = clamp((D / W) / 0.15, 0, 1)
        # r = w * r_live + (1 - w) * r_prior
        w_blend = min(1.0, max(0.0, (self.audio_seconds_done / max(0.001, self.total_audio_seconds)) / 0.15))
        effective_rate = w_blend * self.smoothed_rate + (1.0 - w_blend) * self.prior_rate

        remaining_work = max(0.0, self.total_audio_seconds - self.audio_seconds_done)
        eta_seconds = remaining_work / max(0.01, effective_rate)

        return {
            "percent": self.current_percent,
            "audio_seconds_done": round(self.audio_seconds_done, 2),
            "total_audio_seconds": round(self.total_audio_seconds, 2),
            "rate_audio_sec_per_wall_sec": round(effective_rate, 2),
            "rtf_equivalent": round(1.0 / max(0.01, effective_rate), 3),
            "eta_seconds": round(eta_seconds, 1),
            "elapsed_seconds": round(elapsed, 2),
        }
