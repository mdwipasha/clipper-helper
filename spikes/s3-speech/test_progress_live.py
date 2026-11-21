import os
import sys
import subprocess
import time
import json

sys.path.insert(0, os.path.dirname(__file__))
from whisper_progress_parser import WhisperProgressParser

BASE_DIR = os.path.abspath("spikes/s3-speech")
WHISPER_EXE = os.path.join(BASE_DIR, "bin", "whisper-bin-x64", "Release", "whisper-cli.exe")
MODEL_PATH = os.path.join(BASE_DIR, "models", "ggml-base.bin")
AUDIO_PATH = os.path.join(BASE_DIR, "audio", "podcast_mixed.wav")
RESULTS_DIR = os.path.join(BASE_DIR, "results", "progress_eval")
os.makedirs(RESULTS_DIR, exist_ok=True)

def run_progress_test(run_index):
    cmd = [
        WHISPER_EXE,
        "-m", MODEL_PATH,
        "-f", AUDIO_PATH,
        "-t", "4",
        "-pp",
        "-np",
        "-l", "id",
    ]
    
    parser = WhisperProgressParser(total_audio_seconds=60.0, prior_rtf=0.25)
    captured_events = []
    captured_lines = []
    
    t_start = time.perf_counter()
    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace"
    )

    eta_at_50pct = None
    time_at_50pct = None

    # Read stderr where whisper prints progress callback
    for line in proc.stderr:
        line_clean = line.strip()
        captured_lines.append(line_clean)
        t_now = time.perf_counter()
        ev = parser.parse_line(line_clean, current_time=t_now)
        if ev:
            captured_events.append(ev)
            if ev["percent"] >= 50 and eta_at_50pct is None:
                eta_at_50pct = ev["eta_seconds"]
                time_at_50pct = t_now

    proc.wait()
    t_end = time.perf_counter()
    total_wall_s = t_end - t_start

    actual_remaining_from_50pct = (t_end - time_at_50pct) if time_at_50pct else None
    eta_error_s = (eta_at_50pct - actual_remaining_from_50pct) if (eta_at_50pct and actual_remaining_from_50pct) else None
    events_per_sec = len(captured_events) / max(0.001, total_wall_s)

    return {
        "run_index": run_index,
        "total_wall_s": round(total_wall_s, 3),
        "events_count": len(captured_events),
        "events_per_sec": round(events_per_sec, 2),
        "eta_at_50pct": round(eta_at_50pct, 2) if eta_at_50pct else None,
        "actual_remaining_from_50pct": round(actual_remaining_from_50pct, 2) if actual_remaining_from_50pct else None,
        "eta_error_s": round(eta_error_s, 2) if eta_error_s is not None else None,
        "captured_events": captured_events,
        "sample_lines": captured_lines[:15],
    }

def main():
    print("Testing Progress Parser over 3 runs...")
    runs = []
    for i in range(1, 4):
        print(f"  Starting Run {i}/3...", end="", flush=True)
        res = run_progress_test(i)
        runs.append(res)
        print(f" Wall: {res['total_wall_s']}s | Events: {res['events_count']} ({res['events_per_sec']} ev/s) | ETA error @50%: {res['eta_error_s']}s")

    summary_file = os.path.join(RESULTS_DIR, "progress_parser_metrics.json")
    with open(summary_file, "w", encoding="utf-8") as f:
        json.dump(runs, f, indent=2)
    print(f"Saved progress parser evaluation to {summary_file}")

if __name__ == "__main__":
    main()
