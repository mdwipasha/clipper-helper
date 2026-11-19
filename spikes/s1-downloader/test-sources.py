#!/usr/bin/env python3
"""
Spike S1b — test-sources.py
Item 4: Source Diversity:
- 3 different videos:
  1. 30 fps or 720p-only video (e.g. 1080p30 or 720p)
  2. Non-AVC1 at 1080p (VP9 or AV1 stream)
  3. Older VOD (classic upload, e.g. 2009)
- For non-avc1 case: test download and test proxy transcode fallback (for WebView2 preview).
"""
import json
import os
import subprocess
import time

SPIKE_DIR = os.path.dirname(os.path.abspath(__file__))
BIN_DIR = os.path.join(SPIKE_DIR, "bin")
FFMPEG_DIR = os.path.join(BIN_DIR, "ffmpeg-lgpl", "bin")
RESULTS_DIR = os.path.join(SPIKE_DIR, "results", "s1b")
os.makedirs(RESULTS_DIR, exist_ok=True)

YTDLP = os.path.join(BIN_DIR, "yt-dlp.exe")
QJS = os.path.join(BIN_DIR, "qjs.exe")
FFMPEG = os.path.join(FFMPEG_DIR, "ffmpeg.exe")
FFPROBE = os.path.join(FFMPEG_DIR, "ffprobe.exe")

SOURCES = [
    {
        "label": "30fps_video",
        "url": "https://www.youtube.com/watch?v=9bZkp7q19f0",  # Gangnam Style (1080p 30fps)
        "format": "bv*[vcodec^=avc1][height<=1080]+ba[acodec^=mp4a]/b[height<=1080]",
        "range": "*00:01:00-00:02:00",
        "ext": "mp4",
    },
    {
        "label": "vp9_1080p_video",
        "url": "https://www.youtube.com/watch?v=aqz-KE-bpKQ",  # Big Buck Bunny (VP9 1080p)
        "format": "bv*[vcodec^=vp09][height<=1080]+ba[acodec^=opus]/bv*[vcodec^=vp09][height<=1080]+ba/b",
        "range": "*00:01:00-00:02:00",
        "ext": "webm",
    },
    {
        "label": "older_vod_2009",
        "url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ",  # Rick Astley (Uploaded Oct 2009)
        "format": "bv*[vcodec^=avc1][height<=1080]+ba[acodec^=mp4a]/b[height<=1080]",
        "range": "*00:01:00-00:02:00",
        "ext": "mp4",
    },
]

def test_sources():
    print("=" * 70)
    print("Spike S1b: Source Diversity & Preview Proxy Transcode")
    print("=" * 70)

    results = {}

    for src in SOURCES:
        lbl = src["label"]
        print(f"\n--- Testing Source: {lbl} ({src['url']}) ---")
        out_file = os.path.join(RESULTS_DIR, f"source_{lbl}.{src['ext']}")
        if os.path.exists(out_file):
            os.remove(out_file)

        cmd = [
            YTDLP,
            "--no-playlist", "--no-warnings", "--newline", "--ignore-config",
            "--ffmpeg-location", FFMPEG_DIR,
            "--js-runtimes", f"quickjs:{QJS}",
            "-f", src["format"],
            "--download-sections", src["range"],
            "-o", out_file,
            src["url"],
        ]

        t0 = time.monotonic()
        p = subprocess.run(cmd, capture_output=True, text=True)
        wall_time = round(time.monotonic() - t0, 2)
        print(f"[{lbl}] Exit: {p.returncode} in {wall_time}s")

        res_entry = {
            "url": src["url"],
            "requested_format": src["format"],
            "exit_code": p.returncode,
            "wall_time_s": wall_time,
        }

        if os.path.exists(out_file):
            sz = os.path.getsize(out_file)
            res_entry["file_size_mb"] = round(sz / 1e6, 2)
            # Probe streams
            probe_cmd = [FFPROBE, "-v", "quiet", "-show_format", "-show_streams", "-print_format", "json", out_file]
            probe_p = subprocess.run(probe_cmd, capture_output=True, text=True)
            if probe_p.returncode == 0:
                pinfo = json.loads(probe_p.stdout)
                v_stream = next((s for s in pinfo.get("streams", []) if s["codec_type"] == "video"), {})
                a_stream = next((s for s in pinfo.get("streams", []) if s["codec_type"] == "audio"), {})
                res_entry["video_codec"] = v_stream.get("codec_name")
                res_entry["width"] = v_stream.get("width")
                res_entry["height"] = v_stream.get("height")
                res_entry["r_frame_rate"] = v_stream.get("r_frame_rate")
                res_entry["avg_frame_rate"] = v_stream.get("avg_frame_rate")
                res_entry["audio_codec"] = a_stream.get("codec_name")
                res_entry["format_name"] = pinfo.get("format", {}).get("format_name")
                print(f"  Specs: {res_entry['width']}x{res_entry['height']} @ {res_entry['r_frame_rate']}fps, {res_entry['video_codec']}+{res_entry['audio_codec']} ({res_entry['format_name']})")
        else:
            res_entry["error"] = p.stderr[-300:] if p.stderr else p.stdout[-300:]
            print(f"  Error: {res_entry['error']}")

        results[lbl] = res_entry

    # Test proxy transcode fallback for VP9 video
    vp9_file = os.path.join(RESULTS_DIR, "source_vp9_1080p_video.webm")
    if os.path.exists(vp9_file):
        print("\n--- Testing Proxy Transcode for VP9 -> H.264/AAC MP4 (for WebView2 Preview) ---")
        proxy_out = os.path.join(RESULTS_DIR, "source_vp9_proxy_preview.mp4")
        if os.path.exists(proxy_out):
            os.remove(proxy_out)

        # Test proxy transcode using Windows Media Foundation (h264_mf) hardware/platform encoder
        proxy_cmd = [
            FFMPEG,
            "-y",
            "-i", vp9_file,
            "-c:v", "h264_mf",
            "-b:v", "2000k",
            "-c:a", "aac",
            "-b:a", "128k",
            "-movflags", "+faststart",
            proxy_out,
        ]
        t0 = time.monotonic()
        p_proxy = subprocess.run(proxy_cmd, capture_output=True, text=True)
        proxy_wall = round(time.monotonic() - t0, 2)
        print(f"Proxy transcode (h264_mf) exit={p_proxy.returncode} in {proxy_wall}s")

        results["proxy_transcode"] = {
            "exit_code": p_proxy.returncode,
            "wall_time_s": proxy_wall,
            "encoder": "h264_mf",
        }
        if os.path.exists(proxy_out):
            results["proxy_transcode"]["proxy_size_mb"] = round(os.path.getsize(proxy_out) / 1e6, 2)
            print(f"Proxy output created: {results['proxy_transcode']['proxy_size_mb']} MB in {proxy_wall}s")
        else:
            print(f"Proxy transcode failed: {p_proxy.stderr[-300:]}")
            results["proxy_transcode"]["error"] = p_proxy.stderr[-300:]

    with open(os.path.join(RESULTS_DIR, "source_diversity_results.json"), "w") as f:
        json.dump(results, f, indent=2)

if __name__ == "__main__":
    test_sources()
