import subprocess
import os
import json

BASE_DIR = os.path.abspath("spikes/s3-speech")
S5_DIR = os.path.abspath("spikes/s5-ffmpeg")
FFMPEG_EXE = os.path.join(S5_DIR, "bin", "ffmpeg-release-shared", "ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0", "bin", "ffmpeg.exe")
AUDIO_FILE = os.path.join(BASE_DIR, "audio", "podcast_mixed.wav")
DTW_JSON = os.path.join(BASE_DIR, "results", "matrix", "dtw_True.json")

from whisper_to_words import convert_whisper_json_to_words

with open(DTW_JSON, "r", encoding="utf-8") as f:
    words = convert_whisper_json_to_words(json.load(f))

# Spot-check 10 words at different intervals
spot_indices = [0, 1, 2, 5, 10, 15, 20, 25]
print("DTW Spot-check timestamps:")
for idx in spot_indices:
    if idx < len(words):
        w = words[idx]
        t0_s = w["t0"] / 1000.0
        t1_s = w["t1"] / 1000.0
        dur_s = t1_s - t0_s
        print(f"Word {idx:2} '{w['text']:12}': {t0_s:6.3f}s - {t1_s:6.3f}s (dur: {dur_s:5.3f}s)")
