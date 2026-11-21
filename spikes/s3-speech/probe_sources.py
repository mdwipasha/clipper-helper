import subprocess
import json
import os
import sys

YTDLP_BIN = os.path.abspath("spikes/s1-downloader/bin/yt-dlp.exe")
QJS_BIN = os.path.abspath("spikes/s1-downloader/bin/qjs.exe")

urls = [
    ("gaming_slang", "https://www.youtube.com/watch?v=WufkeDaJisQ"),
    ("podcast_mixed", "https://www.youtube.com/watch?v=3snfJrlSyqo"),
    ("gaming_live", "https://www.youtube.com/watch?v=XwQwhjzAc_k"),
]

for tag, url in urls:
    print(f"\n--- Probing {tag}: {url} ---")
    cmd = [
        YTDLP_BIN,
        "--no-playlist",
        "--no-warnings",
        "--ignore-config",
        "--js-runtimes", f"quickjs:{QJS_BIN}",
        "-J",
        url
    ]
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, check=True)
        meta = json.loads(proc.stdout)
        print(f"Title: {meta.get('title')}")
        print(f"Duration: {meta.get('duration')}s ({meta.get('duration_string')})")
        print(f"is_live: {meta.get('is_live')}, was_live: {meta.get('was_live')}, live_status: {meta.get('live_status')}")
    except subprocess.CalledProcessError as e:
        print(f"ERROR: {e.stderr[:300]}")
    except Exception as e:
        print(f"EXCEPTION: {e}")
