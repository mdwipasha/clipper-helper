import subprocess
import os
import json

BASE_DIR = os.path.abspath("spikes/s3-speech")
S5_DIR = os.path.abspath("spikes/s5-ffmpeg")
FFMPEG_EXE = os.path.join(S5_DIR, "bin", "ffmpeg-release-shared", "ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0", "bin", "ffmpeg.exe")
AUDIO_FILE = os.path.join(BASE_DIR, "audio", "podcast_mixed.wav")

slices = [
    ("Word 0: Gak", 0.010, 0.140),
    ("Silence between 2 & 3", 0.580, 0.610),
    ("Word 5: lah.", 0.840, 0.260),
    ("Word 10: Kayaknya", 2.110, 0.470),
    ("Word 20: yang", 5.030, 0.150),
]

for label, start, dur in slices:
    cmd = [
        FFMPEG_EXE,
        "-ss", str(start),
        "-t", str(dur),
        "-i", AUDIO_FILE,
        "-af", "volumedetect",
        "-f", "null",
        "-"
    ]
    res = subprocess.run(cmd, capture_output=True, text=True, errors="replace")
    mean_vol = "N/A"
    max_vol = "N/A"
    for line in res.stderr.splitlines():
        if "mean_volume" in line:
            mean_vol = line.strip().split("mean_volume: ")[-1]
        if "max_volume" in line:
            max_vol = line.strip().split("max_volume: ")[-1]
    print(f"{label:25} [{start:5.3f}s + {dur:5.3f}s] -> Mean: {mean_vol:10} Max: {max_vol}")
