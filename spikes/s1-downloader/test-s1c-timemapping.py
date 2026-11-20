#!/usr/bin/env python3
"""
Spike S1c — test-s1c-timemapping.py
Part B: Time-Mapping Accuracy & absoluteStartMs Verification
- For ~30 min, ~1.5 h, and ~3 h offsets:
  Download two ranges of the same moment starting at T and T-7s (ending at the same point).
  (Uses existing S1b offset files for T, downloads T-7s).
- Cross-correlates extracted audio (16 kHz mono WAV) with sub-millisecond precision
  to measure the true acoustic offset between the files vs the requested 7000 ms.
- Inspects ffprobe start_time, first-packet pts/dts/flags (video & audio), pre-roll packets.
- Determines what t=0 means in a browser <video> vs FFmpeg decoding.
- Outputs precise definition of absoluteStartMs and observed error in ms.
"""
import json
import os
import subprocess
import time
import wave
import numpy as np

SPIKE_DIR = os.path.dirname(os.path.abspath(__file__))
BIN_DIR = os.path.join(SPIKE_DIR, "bin")
FFMPEG_DIR = os.path.join(BIN_DIR, "ffmpeg-lgpl", "bin")
RESULTS_DIR = os.path.join(SPIKE_DIR, "results", "s1c")
RESULTS_S1B = os.path.join(SPIKE_DIR, "results", "s1b")
os.makedirs(RESULTS_DIR, exist_ok=True)

YTDLP = os.path.join(BIN_DIR, "yt-dlp.exe")
QJS = os.path.join(BIN_DIR, "qjs.exe")
FFMPEG = os.path.join(FFMPEG_DIR, "ffmpeg.exe")
FFPROBE = os.path.join(FFMPEG_DIR, "ffprobe.exe")

TARGET_URL = "https://www.youtube.com/watch?v=QOJIILaJKNc"
FORMAT_SELECTOR = "bv*[vcodec^=avc1][height<=1080]+ba[acodec^=mp4a]/b[vcodec^=avc1][height<=1080]/bv*[height<=1080]+ba/b[height<=1080]"

OFFSETS = [
    {
        "label": "30min",
        "file_T": os.path.join(RESULTS_S1B, "offset_30min.mp4"),
        "req_T_start_s": 1800,  # 00:30:00
        "req_T_end_s": 1860,    # 00:31:00
        "minus7_range": "*00:29:53-00:31:00",
        "req_minus7_start_s": 1793,
        "req_minus7_end_s": 1860,
    },
    {
        "label": "90min",
        "file_T": os.path.join(RESULTS_S1B, "offset_90min.mp4"),
        "req_T_start_s": 5400,  # 01:30:00
        "req_T_end_s": 5460,    # 01:31:00
        "minus7_range": "*01:29:53-01:31:00",
        "req_minus7_start_s": 5393,
        "req_minus7_end_s": 5460,
    },
    {
        "label": "180min",
        "file_T": os.path.join(RESULTS_S1B, "offset_180min.mp4"),
        "req_T_start_s": 10800, # 03:00:00
        "req_T_end_s": 10860,   # 03:01:00
        "minus7_range": "*02:59:53-03:01:00",
        "req_minus7_start_s": 10793,
        "req_minus7_end_s": 10860,
    },
]

def load_wav_mono_16k(wav_path):
    with wave.open(wav_path, "rb") as w:
        sr = w.getframerate()
        n = w.getnframes()
        data = w.readframes(n)
        samples = np.frombuffer(data, dtype=np.int16).astype(np.float32)
        return samples, sr

def extract_audio(video_path, wav_path):
    cmd = [
        FFMPEG, "-y",
        "-i", video_path,
        "-vn",
        "-acodec", "pcm_s16le",
        "-ac", "1",
        "-ar", "16000",
        wav_path
    ]
    subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)

def probe_packet_headers(video_path):
    cmd_fmt = [
        FFPROBE, "-v", "error",
        "-show_entries", "format=start_time,duration:stream=index,codec_type,codec_name,start_time,duration",
        "-of", "json",
        video_path
    ]
    fmt_data = json.loads(subprocess.check_output(cmd_fmt).decode())

    # Get first 15 video packets
    cmd_v_pkts = [
        FFPROBE, "-v", "error",
        "-select_streams", "v:0",
        "-show_packets",
        "-show_entries", "packet=pts_time,dts_time,flags,duration_time",
        "-read_intervals", "%+#15",
        "-of", "json",
        video_path
    ]
    v_pkts = json.loads(subprocess.check_output(cmd_v_pkts).decode()).get("packets", [])

    # Get first 15 audio packets
    cmd_a_pkts = [
        FFPROBE, "-v", "error",
        "-select_streams", "a:0",
        "-show_packets",
        "-show_entries", "packet=pts_time,dts_time,flags,duration_time",
        "-read_intervals", "%+#15",
        "-of", "json",
        video_path
    ]
    a_pkts = json.loads(subprocess.check_output(cmd_a_pkts).decode()).get("packets", [])

    return {
        "format": fmt_data.get("format", {}),
        "streams": fmt_data.get("streams", []),
        "first_video_packets": v_pkts[:5],
        "first_audio_packets": a_pkts[:5],
    }

def main():
    print("=== Spike S1c: Time-Mapping & absoluteStartMs Accuracy ===")
    results = []

    for item in OFFSETS:
        label = item["label"]
        file_T = item["file_T"]
        file_minus7 = os.path.join(RESULTS_DIR, f"offset_{label}_minus7s.mp4")

        print(f"\n--- Processing Offset {label} (Requested delta = 7000 ms) ---")
        if not os.path.exists(file_T):
            print(f"Error: file_T does not exist: {file_T}")
            continue

        # Download minus7s range if not present
        if not os.path.exists(file_minus7):
            print(f"Downloading range {item['minus7_range']} -> {os.path.basename(file_minus7)}...")
            dl_cmd = [
                YTDLP,
                "--no-playlist", "--no-warnings", "--newline", "--ignore-config",
                "--js-runtimes", f"quickjs:{QJS}",
                "--ffmpeg-location", FFMPEG_DIR,
                "-f", FORMAT_SELECTOR,
                "--download-sections", item["minus7_range"],
                "-o", file_minus7,
                TARGET_URL
            ]
            t0 = time.time()
            proc = subprocess.run(dl_cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            print(f"Download finished in {time.time() - t0:.2f} s (exit {proc.returncode})")
            if proc.returncode != 0:
                print(f"Error downloading {file_minus7}: {proc.stderr[:300]}")
                continue
        else:
            print(f"Reusing existing {os.path.basename(file_minus7)}")

        # Extract WAV audio for cross-correlation
        wav_T = os.path.join(RESULTS_DIR, f"audio_{label}_T.wav")
        wav_minus7 = os.path.join(RESULTS_DIR, f"audio_{label}_minus7s.wav")
        extract_audio(file_T, wav_T)
        extract_audio(file_minus7, wav_minus7)

        # Cross-correlate audio
        samples_T, sr_T = load_wav_mono_16k(wav_T)
        samples_m7, sr_m7 = load_wav_mono_16k(wav_minus7)
        assert sr_T == 16000 and sr_m7 == 16000

        # Template from file_T: seconds 5.0 to 25.0 (20 seconds duration)
        template_start_s = 5.0
        template_dur_s = 20.0
        t_start_idx = int(template_start_s * sr_T)
        t_end_idx = int((template_start_s + template_dur_s) * sr_T)
        template = samples_T[t_start_idx:t_end_idx]

        # Search window in file_minus7: seconds 0.0 to 40.0
        search_window = samples_m7[0:int(40.0 * sr_m7)]

        # Cross-correlation via numpy correlate
        corr = np.correlate(search_window, template, mode="valid")
        peak_idx = int(np.argmax(corr))

        # True position of template in file_minus7 in seconds:
        matched_time_in_minus7_s = peak_idx / sr_m7
        # Measured time shift between file_minus7 and file_T:
        actual_shift_s = matched_time_in_minus7_s - template_start_s
        actual_shift_ms = actual_shift_s * 1000.0

        nominal_shift_ms = 7000.0
        delta_error_ms = actual_shift_ms - nominal_shift_ms
        frame_error_60fps = delta_error_ms / (1000.0 / 60.0)

        print(f"  [Acoustic Cross-Correlation]")
        print(f"  Nominal delta: {nominal_shift_ms:.1f} ms")
        print(f"  Acoustic delta measured: {actual_shift_ms:.2f} ms")
        print(f"  Timing discrepancy (error): {delta_error_ms:+.2f} ms ({frame_error_60fps:+.2f} frames @ 60fps)")

        # Packet header inspection
        probe_T = probe_packet_headers(file_T)
        probe_m7 = probe_packet_headers(file_minus7)

        v0_T = probe_T["first_video_packets"][0] if probe_T["first_video_packets"] else {}
        a0_T = probe_T["first_audio_packets"][0] if probe_T["first_audio_packets"] else {}
        v0_m7 = probe_m7["first_video_packets"][0] if probe_m7["first_video_packets"] else {}
        a0_m7 = probe_m7["first_audio_packets"][0] if probe_m7["first_audio_packets"] else {}

        print(f"  [File_T Packet Headers]")
        print(f"    Format start_time: {probe_T['format'].get('start_time')}")
        print(f"    First video packet: pts={v0_T.get('pts_time')}s, dts={v0_T.get('dts_time')}s, flags={v0_T.get('flags')}")
        print(f"    First audio packet: pts={a0_T.get('pts_time')}s, dts={a0_T.get('dts_time')}s, flags={a0_T.get('flags')}")

        print(f"  [File_minus7 Packet Headers]")
        print(f"    Format start_time: {probe_m7['format'].get('start_time')}")
        print(f"    First video packet: pts={v0_m7.get('pts_time')}s, dts={v0_m7.get('dts_time')}s, flags={v0_m7.get('flags')}")
        print(f"    First audio packet: pts={a0_m7.get('pts_time')}s, dts={a0_m7.get('dts_time')}s, flags={a0_m7.get('flags')}")

        results.append({
            "offset_label": label,
            "req_T_start_s": item["req_T_start_s"],
            "req_minus7_start_s": item["req_minus7_start_s"],
            "nominal_delta_ms": nominal_shift_ms,
            "acoustic_delta_ms": round(actual_shift_ms, 2),
            "timing_error_ms": round(delta_error_ms, 2),
            "frame_error_60fps": round(frame_error_60fps, 2),
            "file_T_v0_pts": v0_T.get("pts_time"),
            "file_T_v0_flags": v0_T.get("flags"),
            "file_minus7_v0_pts": v0_m7.get("pts_time"),
            "file_minus7_v0_flags": v0_m7.get("flags"),
        })

    out_json = os.path.join(RESULTS_DIR, "time_mapping_results.json")
    with open(out_json, "w", encoding="utf-8") as f:
        json.dump(results, f, indent=2)
    print(f"\nSaved time mapping results to {out_json}")

if __name__ == "__main__":
    main()
