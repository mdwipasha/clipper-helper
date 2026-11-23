import subprocess
import json
import os
import sys

YTDLP_BIN = r"F:\coding\clipper-helper\spikes\s1-downloader\bin\yt-dlp.exe"
QJS_BIN = r"F:\coding\clipper-helper\spikes\s1-downloader\bin\qjs.exe"

CANDIDATES = [
    {"group": "(a)", "url": "https://youtu.be/WufkeDaJisQ", "label": "Short clip / standard"},
    {"group": "(a)", "url": "https://youtu.be/3snfJrlSyqo", "label": "Standard gaming / music video"},
    {"group": "(a)", "url": "https://www.youtube.com/live/XwQwhjzAc_k", "label": "Live stream URL format"},
    {"group": "(c)(d)", "url": "https://www.youtube.com/watch?v=9s6imDGjy08", "label": "Long monetized VOD (~3h47m)"},
    {"group": "(d)", "url": "https://www.youtube.com/watch?v=QOJIILaJKNc", "label": "Long VOD from S1 (~2h22m)"},
    {"group": "(e)", "url": "https://www.youtube.com/watch?v=KBdGeakRgqg", "label": "Members-only test"},
    {"group": "(e)", "url": "https://www.youtube.com/watch?v=6kLq3WMV1nU", "label": "Age-restricted test"},
]

def probe_url(cand):
    url = cand["url"]
    cmd = [
        YTDLP_BIN,
        "--js-runtimes", f"quickjs:{QJS_BIN}",
        "-J",
        "--no-warnings",
        url
    ]
    print(f"\n[Probing {cand['group']}]: {url} ...", flush=True)
    try:
        proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=60)
        if proc.returncode == 0:
            info = json.loads(proc.stdout)
            return {
                "group": cand["group"],
                "url": url,
                "label": cand["label"],
                "status": "OK",
                "id": info.get("id"),
                "title": info.get("title"),
                "duration": info.get("duration"),
                "live_status": info.get("live_status"),
                "availability": info.get("availability"),
                "playable_in_embed": info.get("playable_in_embed"),
                "is_live": info.get("is_live"),
                "was_live": info.get("was_live"),
                "channel": info.get("channel"),
                "view_count": info.get("view_count"),
                "age_limit": info.get("age_limit"),
                "error": None
            }
        else:
            # Check stderr for specific failure
            err_msg = proc.stderr.strip()
            # Try to extract clean error line
            lines = [l for l in err_msg.splitlines() if "ERROR:" in l]
            clean_err = lines[0] if lines else err_msg[:200]
            return {
                "group": cand["group"],
                "url": url,
                "label": cand["label"],
                "status": "ERROR",
                "id": None,
                "title": None,
                "duration": None,
                "live_status": None,
                "availability": "unavailable / blocked",
                "playable_in_embed": False,
                "error": clean_err
            }
    except Exception as e:
        return {
            "group": cand["group"],
            "url": url,
            "label": cand["label"],
            "status": "EXCEPTION",
            "id": None,
            "title": None,
            "duration": None,
            "live_status": None,
            "availability": "exception",
            "playable_in_embed": None,
            "error": str(e)
        }

def main():
    results = []
    for cand in CANDIDATES:
        res = probe_url(cand)
        results.append(res)
        print(f"  -> Title: {res.get('title')}")
        print(f"  -> Duration: {res.get('duration')} s | Live: {res.get('live_status')} | Availability: {res.get('availability')} | Embeddable: {res.get('playable_in_embed')}")
        if res.get("error"):
            print(f"  -> Error: {res['error']}")

    out_file = os.path.join(os.path.dirname(__file__), "candidates_probe.json")
    with open(out_file, "w", encoding="utf-8") as f:
        json.dump(results, f, indent=2, ensure_ascii=False)
    print(f"\nAll results saved to {out_file}")

if __name__ == "__main__":
    main()
