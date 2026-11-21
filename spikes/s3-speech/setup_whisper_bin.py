import os
import sys
import urllib.request
import zipfile
import hashlib
import json

BIN_DIR = os.path.abspath("spikes/s3-speech/bin")
MODELS_DIR = os.path.abspath("spikes/s3-speech/models")
os.makedirs(BIN_DIR, exist_ok=True)
os.makedirs(MODELS_DIR, exist_ok=True)

def sha256_file(filepath):
    h = hashlib.sha256()
    with open(filepath, "rb") as f:
        while chunk := f.read(65536):
            h.update(chunk)
    return h.hexdigest()

def download_file(url, dest_path):
    if os.path.exists(dest_path):
        print(f"Already exists: {dest_path}")
        return
    print(f"Downloading {url} -> {dest_path}...")
    headers = {'User-Agent': 'Mozilla/5.0'}
    req = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(req) as resp, open(dest_path, "wb") as f:
        total = int(resp.getheader("Content-Length", 0))
        downloaded = 0
        while chunk := resp.read(65536):
            f.write(chunk)
            downloaded += len(chunk)
            if total > 0:
                percent = (downloaded / total) * 100
                print(f"\r  {percent:5.1f}% ({downloaded / 1024 / 1024:.1f}/{total / 1024 / 1024:.1f} MB)", end="", flush=True)
        print()

def main():
    # 1. Download whisper-bin-x64.zip (b5454)
    zip_url = "https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-x64.zip"
    zip_path = os.path.join(BIN_DIR, "whisper-bin-x64.zip")
    download_file(zip_url, zip_path)
    print(f"whisper-bin-x64.zip SHA-256: {sha256_file(zip_path)}")
    print(f"whisper-bin-x64.zip Size: {os.path.getsize(zip_path)} bytes")

    extract_dir = os.path.join(BIN_DIR, "whisper-bin-x64")
    if not os.path.exists(extract_dir):
        print(f"Extracting {zip_path} -> {extract_dir}...")
        with zipfile.ZipFile(zip_path, 'r') as zf:
            zf.extractall(extract_dir)

    print("\nContents of whisper-bin-x64:")
    for root, dirs, files in os.walk(extract_dir):
        for f in files:
            fp = os.path.join(root, f)
            print(f"  {os.path.relpath(fp, extract_dir):30} {os.path.getsize(fp):10} bytes  SHA-256: {sha256_file(fp)[:16]}...")

if __name__ == "__main__":
    main()
