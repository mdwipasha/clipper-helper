import os
import sys
import urllib.request
import hashlib

MODELS_DIR = os.path.abspath("spikes/s3-speech/models")
os.makedirs(MODELS_DIR, exist_ok=True)

MODELS = [
    ("ggml-silero-v6.2.0.bin", "https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v6.2.0.bin"),
    ("ggml-base.bin", "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin"),
    ("ggml-small.bin", "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"),
    ("ggml-small-q5_1.bin", "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin"),
    ("ggml-medium.bin", "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin"),
]

def sha256_file(filepath):
    h = hashlib.sha256()
    with open(filepath, "rb") as f:
        while chunk := f.read(1048576):
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
        while chunk := resp.read(1048576):
            f.write(chunk)
            downloaded += len(chunk)
            if total > 0:
                percent = (downloaded / total) * 100
                print(f"\r  {percent:5.1f}% ({downloaded / 1024 / 1024:.1f}/{total / 1024 / 1024:.1f} MB)", end="", flush=True)
        print()

def main():
    metadata = {}
    for filename, url in MODELS:
        dest = os.path.join(MODELS_DIR, filename)
        download_file(url, dest)
        size = os.path.getsize(dest)
        sha = sha256_file(dest)
        metadata[filename] = {"size": size, "sha256": sha, "url": url}
        print(f"Verified: {filename} ({size / 1024 / 1024:.2f} MB) SHA-256: {sha}")

    import json
    with open(os.path.join(MODELS_DIR, "models_manifest.json"), "w") as f:
        json.dump(metadata, f, indent=2)
    print("Saved models_manifest.json")

if __name__ == "__main__":
    main()
