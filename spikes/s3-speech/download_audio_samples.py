import os
import sys
import subprocess
import json

BASE_DIR = os.path.abspath("spikes/s3-speech")
S1_DIR = os.path.abspath("spikes/s1-downloader")
S5_DIR = os.path.abspath("spikes/s5-ffmpeg")

YTDLP_BIN = os.path.join(S1_DIR, "bin", "yt-dlp.exe")
QJS_BIN = os.path.join(S1_DIR, "bin", "qjs.exe")
FFMPEG_BIN_DIR = os.path.join(S5_DIR, "bin", "ffmpeg-release-shared", "ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0", "bin")
FFMPEG_EXE = os.path.join(FFMPEG_BIN_DIR, "ffmpeg.exe")

AUDIO_DIR = os.path.join(BASE_DIR, "audio")
DOWNLOADS_DIR = os.path.join(BASE_DIR, "downloads")
os.makedirs(AUDIO_DIR, exist_ok=True)
os.makedirs(DOWNLOADS_DIR, exist_ok=True)

SAMPLES = [
    {
        "id": "gaming_slang",
        "url": "https://www.youtube.com/watch?v=WufkeDaJisQ",
        "start": "01:00",
        "end": "02:00",
        "description": "Gaming with slang / loud game audio (Deceit 2)",
    },
    {
        "id": "podcast_mixed",
        "url": "https://www.youtube.com/watch?v=3snfJrlSyqo",
        "start": "05:00",
        "end": "06:00",
        "description": "Podcast mixed Indonesian-English (Jonathan Liandi)",
    },
    {
        "id": "gaming_live",
        "url": "https://www.youtube.com/watch?v=XwQwhjzAc_k",
        "start": "01:00",
        "end": "02:00",
        "description": "Livestream gaming (Windah Basudara GTA 6)",
    },
]

def download_and_extract(sample):
    sid = sample["id"]
    raw_output = os.path.join(DOWNLOADS_DIR, f"{sid}.%(ext)s")
    wav_output = os.path.join(AUDIO_DIR, f"{sid}.wav")
    
    section_arg = f"*{sample['start']}-{sample['end']}"
    print(f"\n==========================================")
    print(f"Processing sample: {sid} ({sample['description']})")
    print(f"Range: {sample['start']} - {sample['end']}")
    print(f"==========================================")
    
    # 1. Download range using yt-dlp
    dl_cmd = [
        YTDLP_BIN,
        "--no-playlist",
        "--no-warnings",
        "--ignore-config",
        "--js-runtimes", f"quickjs:{QJS_BIN}",
        "--ffmpeg-location", FFMPEG_BIN_DIR,
        "--download-sections", section_arg,
        "-f", "ba/b",
        "-o", raw_output,
        sample["url"],
    ]
    print(f"Running yt-dlp...")
    res = subprocess.run(dl_cmd, capture_output=True, text=True)
    if res.returncode != 0:
        print(f"Error downloading {sid}: {res.stderr}")
        return False
    print("Download finished.")

    # Find the downloaded file
    downloaded_files = [
        os.path.join(DOWNLOADS_DIR, f)
        for f in os.listdir(DOWNLOADS_DIR)
        if f.startswith(sid + ".") and not f.endswith(".wav")
    ]
    if not downloaded_files:
        print(f"Could not find downloaded file for {sid}")
        return False
    
    raw_file = downloaded_files[0]
    print(f"Downloaded raw file: {raw_file} ({os.path.getsize(raw_file)} bytes)")

    # 2. Extract 16 kHz mono WAV via FFmpeg
    ff_cmd = [
        FFMPEG_EXE,
        "-y",
        "-i", raw_file,
        "-vn",
        "-ar", "16000",
        "-ac", "1",
        "-c:a", "pcm_s16le",
        "-t", "60",
        wav_output,
    ]
    print(f"Extracting 16kHz mono WAV via FFmpeg...")
    ff_res = subprocess.run(ff_cmd, capture_output=True, text=True)
    if ff_res.returncode != 0:
        print(f"FFmpeg error: {ff_res.stderr}")
        return False
    
    print(f"Saved WAV: {wav_output} ({os.path.getsize(wav_output)} bytes)")
    return True

def main():
    for s in SAMPLES:
        ok = download_and_extract(s)
        if not ok:
            print(f"FAILED on sample {s['id']}")
            sys.exit(1)
    print("\nAll 3 samples downloaded and extracted to audio/*.wav successfully!")

if __name__ == "__main__":
    main()
