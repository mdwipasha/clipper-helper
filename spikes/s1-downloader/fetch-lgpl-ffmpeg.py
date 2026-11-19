#!/usr/bin/env python3
"""
Download BtbN FFmpeg LGPL Windows x64 build for Spike S1b Item 3.
"""
import os
import shutil
import subprocess
import urllib.request
import zipfile

BIN_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "bin")
LGPL_DIR = os.path.join(BIN_DIR, "ffmpeg-lgpl")
ZIP_DEST = os.path.join(BIN_DIR, "ffmpeg-win64-lgpl.zip")
URL = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-lgpl.zip"

def main():
    os.makedirs(BIN_DIR, exist_ok=True)
    if not os.path.exists(ZIP_DEST):
        print(f"Downloading BtbN LGPL FFmpeg from {URL}...")
        req = urllib.request.Request(URL, headers={"User-Agent": "ClipperHelper-S1b/1.0"})
        with urllib.request.urlopen(req, timeout=300) as resp, open(ZIP_DEST + ".tmp", "wb") as f:
            shutil.copyfileobj(resp, f)
        os.replace(ZIP_DEST + ".tmp", ZIP_DEST)
        print("Download complete.")
    else:
        print(f"Archive already exists at {ZIP_DEST}")

    # Extract
    if not os.path.exists(LGPL_DIR):
        print(f"Extracting to {LGPL_DIR}...")
        os.makedirs(LGPL_DIR, exist_ok=True)
        with zipfile.ZipFile(ZIP_DEST) as zf:
            for member in zf.namelist():
                # Strip top level dir
                parts = member.split("/", 1)
                if len(parts) > 1 and parts[1]:
                    target = os.path.join(LGPL_DIR, parts[1].replace("/", os.sep))
                    if member.endswith("/"):
                        os.makedirs(target, exist_ok=True)
                    else:
                        os.makedirs(os.path.dirname(target), exist_ok=True)
                        with zf.open(member) as src, open(target, "wb") as dst:
                            shutil.copyfileobj(src, dst)
        print("Extracted.")

    ffmpeg_exe = os.path.join(LGPL_DIR, "bin", "ffmpeg.exe")
    if os.path.exists(ffmpeg_exe):
        r = subprocess.run([ffmpeg_exe, "-version"], capture_output=True, text=True)
        print("Version line:", r.stdout.splitlines()[0])
        print("Has GPL?", "--enable-gpl" in r.stdout)
        enc = subprocess.run([ffmpeg_exe, "-encoders"], capture_output=True, text=True)
        print("Has libx264?", "libx264" in enc.stdout)
        print("Has h264_mf?", "h264_mf" in enc.stdout)
        print("Has h264_nvenc?", "h264_nvenc" in enc.stdout)
        print("Has h264_qsv?", "h264_qsv" in enc.stdout)
        print("Has h264_amf?", "h264_amf" in enc.stdout)
    else:
        print("ffmpeg.exe not found in extracted folder.")

if __name__ == "__main__":
    main()
