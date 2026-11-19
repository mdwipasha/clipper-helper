#!/usr/bin/env python3
"""
Spike S1 — fetch-binaries.py
Downloads PINNED yt-dlp, QuickJS-NG, Deno, and FFmpeg (Windows x64) into bin/.
Verifies SHA-256 hashes against hardcoded pinned checksums, prints versions and sizes.
"""
import hashlib
import os
import shutil
import subprocess
import sys
import urllib.request
import zipfile

BIN_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "bin")

# Pinned versions & exact SHA-256 hashes verified on Windows x64
COMPONENTS = {
    "yt-dlp": {
        "url": "https://github.com/yt-dlp/yt-dlp/releases/download/2026.10.26/yt-dlp.exe",
        "filename": "yt-dlp.exe",
        "version_tag": "2026.10.26",
        "sha256": "f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d",
    },
    "quickjs": {
        "url": "https://github.com/quickjs-ng/quickjs/releases/download/v0.18.0/qjs-windows-x86_64.exe",
        "filename": "qjs.exe",
        "version_tag": "v0.18.0",
        "sha256": "ff03398d74ea6e6bd3a8e58058f7003225abcac229d7d57775b7aaae812a49c7",
    },
    "deno": {
        "url": "https://github.com/denoland/deno/releases/download/v2.9.8/deno-x86_64-pc-windows-msvc.zip",
        "filename": "deno-x86_64-pc-windows-msvc.zip",
        "extract": "deno.exe",
        "version_tag": "v2.9.8",
        "sha256": "cba84b26e0df5ae7f74fc66b86a95b866872a86292ebd84fa1e959927edd93bf",
    },
    "ffmpeg": {
        "url": "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip",
        "filename": "ffmpeg-release-essentials.zip",
        "extract_dir": True,
        "version_tag": "9.0.2-essentials",
        "sha256": "60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba",
    },
}


def sha256_file(path: str) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def download(url: str, dest: str) -> None:
    if os.path.exists(dest):
        print(f"  [skip] already exists: {dest}")
        return
    print(f"  [download] {url}")
    print(f"             → {dest}")
    req = urllib.request.Request(url, headers={"User-Agent": "ClipperHelper-S1-Spike/1.0"})
    with urllib.request.urlopen(req, timeout=600) as resp, open(dest + ".tmp", "wb") as f:
        total = resp.headers.get("Content-Length")
        downloaded = 0
        while True:
            chunk = resp.read(1 << 20)
            if not chunk:
                break
            f.write(chunk)
            downloaded += len(chunk)
            if total:
                pct = downloaded * 100 / int(total)
                print(f"\r  [{pct:5.1f}%] {downloaded / 1e6:.1f} / {int(total) / 1e6:.1f} MB", end="", flush=True)
            else:
                print(f"\r  {downloaded / 1e6:.1f} MB", end="", flush=True)
        print()
    os.replace(dest + ".tmp", dest)


def extract_zip(zip_path: str, member: str | None, extract_dir: bool = False) -> str:
    with zipfile.ZipFile(zip_path) as zf:
        if member:
            info = zf.getinfo(member)
            target = os.path.join(BIN_DIR, member)
            with zf.open(info) as src, open(target, "wb") as dst:
                shutil.copyfileobj(src, dst)
            return target
        elif extract_dir:
            zf.extractall(BIN_DIR)
            for root, dirs, files in os.walk(BIN_DIR):
                if "ffmpeg.exe" in files:
                    return root
    return ""


def get_version(binary_path: str, name: str) -> str:
    try:
        if name == "yt-dlp":
            r = subprocess.run([binary_path, "--version"], capture_output=True, text=True, timeout=15)
            return r.stdout.strip()
        elif name == "deno":
            r = subprocess.run([binary_path, "--version"], capture_output=True, text=True, timeout=15)
            return r.stdout.strip().split("\n")[0]
        elif name == "quickjs":
            r = subprocess.run([binary_path, "-e", "print('ok')"], capture_output=True, text=True, timeout=15)
            return f"QuickJS-NG v0.18.0 ({r.stdout.strip()})" if r.returncode == 0 else "error"
        elif name == "ffmpeg":
            r = subprocess.run([binary_path, "-version"], capture_output=True, text=True, timeout=15)
            return r.stdout.strip().split("\n")[0]
    except Exception as e:
        return f"error: {e}"
    return "unknown"


def main():
    os.makedirs(BIN_DIR, exist_ok=True)
    results = {}

    for name, info in COMPONENTS.items():
        print(f"\n{'='*60}")
        print(f"Component: {name} ({info['version_tag']})")
        print(f"{'='*60}")

        dest = os.path.join(BIN_DIR, info["filename"])
        download(info["url"], dest)

        actual_hash = sha256_file(dest)
        expected = info["sha256"]
        if actual_hash != expected:
            print(f"  *** SHA-256 MISMATCH ***")
            print(f"      expected: {expected}")
            print(f"      actual:   {actual_hash}")
            sys.exit(1)
        print(f"  SHA-256 OK: {actual_hash}")

        size = os.path.getsize(dest)
        print(f"  Size: {size:,} bytes ({size / 1e6:.1f} MB)")

        binary_path = dest
        if "extract" in info:
            extracted_bin = os.path.join(BIN_DIR, info["extract"])
            if not os.path.exists(extracted_bin):
                print(f"  Extracting {info['extract']}...")
                binary_path = extract_zip(dest, info["extract"])
            else:
                binary_path = extracted_bin
        elif info.get("extract_dir"):
            # Check if ffmpeg.exe already exists
            found = None
            for root, dirs, files in os.walk(BIN_DIR):
                if "ffmpeg.exe" in files:
                    found = os.path.join(root, "ffmpeg.exe")
                    break
            if not found:
                print(f"  Extracting zip contents...")
                ffmpeg_dir = extract_zip(dest, None, extract_dir=True)
                binary_path = os.path.join(ffmpeg_dir, "ffmpeg.exe")
            else:
                binary_path = found

        ver = get_version(binary_path, name)
        print(f"  Version: {ver}")

        results[name] = {
            "version": ver,
            "sha256": actual_hash,
            "size_mb": round(size / 1e6, 1),
            "binary_path": binary_path,
        }

    print(f"\n{'='*60}")
    print("ALL PINNED BINARIES VERIFIED")
    print(f"{'='*60}")
    for name, r in results.items():
        print(f"  {name:<12} {r['size_mb']:>6.1f} MB  {r['sha256'][:16]}...  {r['version']}")


if __name__ == "__main__":
    main()
