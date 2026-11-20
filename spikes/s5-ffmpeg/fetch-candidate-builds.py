#!/usr/bin/env python3
"""
Spike S5 — fetch-candidate-builds.py
Fetches and audits FFmpeg LGPL candidate builds from BtbN/FFmpeg-Builds:
1. Release branch static: ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-9.0.zip
2. Release branch shared: ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0.zip
3. Master snapshot static: (reused from spikes/s1-downloader/bin/ffmpeg-win64-lgpl.zip)
Computes sizes, sha256 checksums, unzips to bin/ for inspection.
"""
import hashlib
import json
import os
import shutil
import urllib.request
import zipfile

SPIKE_DIR = os.path.dirname(os.path.abspath(__file__))
BIN_DIR = os.path.join(SPIKE_DIR, "bin")
os.makedirs(BIN_DIR, exist_ok=True)

CANDIDATES = [
    {
        "id": "release_static",
        "name": "ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-9.0.zip",
        "url": "https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/autobuild-2026-11-19-12-39/ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-9.0.zip",
        "type": "Release Branch (Static)",
        "extract_folder": "ffmpeg-release-static",
    },
    {
        "id": "release_shared",
        "name": "ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0.zip",
        "url": "https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/autobuild-2026-11-19-12-39/ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0.zip",
        "type": "Release Branch (Shared)",
        "extract_folder": "ffmpeg-release-shared",
    },
    {
        "id": "master_snapshot_static",
        "name": "ffmpeg-N-129130-gcd39c36377-win64-lgpl.zip",
        "url": "https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/autobuild-2026-11-19-12-39/ffmpeg-N-129130-gcd39c36377-win64-lgpl.zip",
        "type": "Master Snapshot (Static)",
        "extract_folder": "ffmpeg-master-static",
        "local_source": os.path.join(SPIKE_DIR, "..", "s1-downloader", "bin", "ffmpeg-win64-lgpl.zip")
    }
]

def sha256_file(filepath):
    h = hashlib.sha256()
    with open(filepath, "rb") as f:
        while chunk := f.read(8192 * 1024):
            h.update(chunk)
    return h.hexdigest()

def download_file(url, target_path):
    print(f"Downloading {url} -> {os.path.basename(target_path)}...")
    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
    with urllib.request.urlopen(req) as resp, open(target_path, "wb") as out:
        shutil.copyfileobj(resp, out)
    print(f"  Finished download: {os.path.getsize(target_path) / 1024 / 1024:.2f} MB")

def main():
    results = []

    for item in CANDIDATES:
        zip_path = os.path.join(BIN_DIR, item["name"])
        extract_dir = os.path.join(BIN_DIR, item["extract_folder"])

        # Check local source or download
        if not os.path.exists(zip_path):
            if "local_source" in item and os.path.exists(item["local_source"]):
                print(f"Copying from local cache: {item['local_source']} -> {zip_path}")
                shutil.copy2(item["local_source"], zip_path)
            else:
                download_file(item["url"], zip_path)

        zip_size = os.path.getsize(zip_path)
        zip_hash = sha256_file(zip_path)

        # Extract
        if not os.path.exists(extract_dir):
            print(f"Extracting {item['name']} -> {extract_dir}...")
            with zipfile.ZipFile(zip_path, "r") as zf:
                zf.extractall(extract_dir)

        # Locate binaries and DLLs
        ffmpeg_exe = None
        ffprobe_exe = None
        dlls = []
        total_extracted_size = 0

        for root, dirs, files in os.walk(extract_dir):
            for file in files:
                fpath = os.path.join(root, file)
                fsize = os.path.getsize(fpath)
                total_extracted_size += fsize
                if file.lower() == "ffmpeg.exe":
                    ffmpeg_exe = fpath
                elif file.lower() == "ffprobe.exe":
                    ffprobe_exe = fpath
                elif file.lower().endswith(".dll"):
                    dlls.append({"name": file, "size": fsize, "path": fpath})

        ffmpeg_size = os.path.getsize(ffmpeg_exe) if ffmpeg_exe else 0
        ffprobe_size = os.path.getsize(ffprobe_exe) if ffprobe_exe else 0
        ffmpeg_hash = sha256_file(ffmpeg_exe) if ffmpeg_exe else None
        ffprobe_hash = sha256_file(ffprobe_exe) if ffprobe_exe else None

        results.append({
            "id": item["id"],
            "name": item["name"],
            "type": item["type"],
            "url": item["url"],
            "zip_size_bytes": zip_size,
            "zip_size_mb": round(zip_size / 1024 / 1024, 2),
            "zip_sha256": zip_hash,
            "ffmpeg_path": ffmpeg_exe,
            "ffmpeg_size_mb": round(ffmpeg_size / 1024 / 1024, 2),
            "ffmpeg_sha256": ffmpeg_hash,
            "ffprobe_path": ffprobe_exe,
            "ffprobe_size_mb": round(ffprobe_size / 1024 / 1024, 2),
            "ffprobe_sha256": ffprobe_hash,
            "dll_count": len(dlls),
            "dlls_size_mb": round(sum(d["size"] for d in dlls) / 1024 / 1024, 2),
            "total_extracted_mb": round(total_extracted_size / 1024 / 1024, 2),
        })

        print(f"\n--- {item['type']}: {item['name']} ---")
        print(f"  Zip size: {zip_size / 1024 / 1024:.2f} MB | SHA-256: {zip_hash[:16]}...")
        print(f"  ffmpeg.exe: {ffmpeg_size / 1024 / 1024:.2f} MB")
        print(f"  ffprobe.exe: {ffprobe_size / 1024 / 1024:.2f} MB")
        print(f"  DLL count: {len(dlls)} ({sum(d['size'] for d in dlls) / 1024 / 1024:.2f} MB)")
        print(f"  Total extracted footprint: {total_extracted_size / 1024 / 1024:.2f} MB")

    out_json = os.path.join(SPIKE_DIR, "results", "candidate_builds.json")
    os.makedirs(os.path.dirname(out_json), exist_ok=True)
    with open(out_json, "w", encoding="utf-8") as f:
        json.dump(results, f, indent=2)
    print(f"\nSaved build comparison to {out_json}")

if __name__ == "__main__":
    main()
