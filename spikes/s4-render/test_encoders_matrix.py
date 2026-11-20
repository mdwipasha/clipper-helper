"""
Encoder Matrix & Option Audit for AMD Hardware (R1 Testbed)
Tests h264_amf and h264_mf against rate-control, profile, level, color tags, and presets.
"""

import os
import subprocess
import json

FFMPEG_EXE = os.path.abspath(r"spikes\s5-ffmpeg\bin\ffmpeg-release-shared\ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0\bin\ffmpeg.exe")
FFPROBE_EXE = os.path.abspath(r"spikes\s5-ffmpeg\bin\ffmpeg-release-shared\ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0\bin\ffprobe.exe")
RESULTS_DIR = os.path.abspath("spikes/s4-render/results")
os.makedirs(RESULTS_DIR, exist_ok=True)


def probe_file(file_path: str) -> dict:
    cmd = [
        FFPROBE_EXE, "-v", "quiet", "-print_format", "json",
        "-show_streams", "-show_format", file_path
    ]
    res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if res.returncode == 0:
        return json.loads(res.stdout)
    return {}


def test_encoder_configuration(name: str, encoder: str, extra_args: list[str]) -> dict:
    out_file = os.path.join(RESULTS_DIR, f"test_enc_{name}.mp4")
    cmd = [
        FFMPEG_EXE, "-y", "-hide_banner",
        "-f", "lavfi", "-i", "testsrc=size=1080x1920:rate=60:duration=2",
        "-f", "lavfi", "-i", "sine=frequency=1000:duration=2",
        "-vf", "fps=30,format=yuv420p",
        "-c:v", encoder
    ]
    cmd.extend(extra_args)
    cmd.extend([
        "-c:a", "aac", "-b:a", "192k",
        "-color_primaries", "bt709",
        "-color_trc", "bt709",
        "-colorspace", "bt709",
        "-color_range", "tv",
        "-movflags", "+faststart",
        out_file
    ])

    print(f"Testing {name} ({encoder})...")
    res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    success = (res.returncode == 0 and os.path.exists(out_file) and os.path.getsize(out_file) > 1000)

    probe = {}
    if success:
        probe = probe_file(out_file)
        v_stream = next((s for s in probe.get("streams", []) if s.get("codec_type") == "video"), {})
        a_stream = next((s for s in probe.get("streams", []) if s.get("codec_type") == "audio"), {})

        size_kb = os.path.getsize(out_file) / 1024.0
        bitrate_kbps = int(probe.get("format", {}).get("bit_rate", 0)) / 1000.0

        return {
            "name": name,
            "encoder": encoder,
            "success": True,
            "size_kb": round(size_kb, 1),
            "bitrate_kbps": round(bitrate_kbps, 1),
            "profile": v_stream.get("profile"),
            "level": v_stream.get("level"),
            "pix_fmt": v_stream.get("pix_fmt"),
            "color_primaries": v_stream.get("color_primaries"),
            "color_trc": v_stream.get("color_trc"),
            "colorspace": v_stream.get("color_space"),
            "color_range": v_stream.get("color_range"),
            "fps": v_stream.get("r_frame_rate"),
            "audio_codec": a_stream.get("codec_name"),
            "output_file": out_file
        }
    else:
        return {
            "name": name,
            "encoder": encoder,
            "success": False,
            "error": res.stderr[-500:] if res.stderr else "Unknown failure"
        }


def run_matrix():
    configs = [
        # h264_amf configurations
        ("amf_standard", "h264_amf", [
            "-usage", "transcoding", "-rc", "vbr_latency", "-b:v", "4500k",
            "-maxrate", "6000k", "-bufsize", "9000k", "-quality", "quality",
            "-profile:v", "high", "-level", "4.2", "-g", "30"
        ]),
        ("amf_small_file", "h264_amf", [
            "-usage", "transcoding", "-rc", "cbr", "-b:v", "2500k",
            "-maxrate", "2500k", "-bufsize", "5000k",
            "-profile:v", "high", "-level", "4.2", "-g", "30"
        ]),
        ("amf_cqp", "h264_amf", [
            "-usage", "transcoding", "-rc", "cqp", "-qp_i", "22", "-qp_p", "24",
            "-profile:v", "high", "-level", "4.2", "-g", "30"
        ]),

        # h264_mf configurations (h264_mf does not take -profile)
        ("mf_standard", "h264_mf", [
            "-b:v", "4500k", "-rate_control", "pc_vbr", "-g", "30"
        ]),
        ("mf_small_file", "h264_mf", [
            "-b:v", "2500k", "-rate_control", "cbr", "-g", "30"
        ]),
        ("mf_quality_mode", "h264_mf", [
            "-rate_control", "quality", "-quality", "75", "-g", "30"
        ]),

        # Software reference encoder (for baseline quality comparison)
        ("openh264_reference", "libopenh264", [
            "-b:v", "4500k", "-profile:v", "high", "-g", "30"
        ])
    ]

    results = []
    for name, enc, args in configs:
        res = test_encoder_configuration(name, enc, args)
        results.append(res)
        print(f"  Result: Success={res['success']} | Profile={res.get('profile')} | Bitrate={res.get('bitrate_kbps')} kbps")

    summary_file = os.path.join(RESULTS_DIR, "encoder_matrix_results.json")
    with open(summary_file, "w", encoding="utf-8") as f:
        json.dump(results, f, indent=2)

    print(f"\nSaved matrix results to {summary_file}")


if __name__ == "__main__":
    run_matrix()
