"""
Spike S4b: Objective Quality Evaluation vs Scaled Source & AMF Rate Control A/B
Compares h264_amf (vbr_latency vs vbr_peak) and h264_mf for standard and small_file presets.
Measures each encode directly against SCALED SOURCE (1080x1920 @ 30 fps, crop 0.25..0.75).
Computes SSIM, PSNR, and VMAF (via libvmaf).
"""

import json
import os
import re
import subprocess
import time

FFMPEG_EXE = os.path.abspath(r"spikes\s5-ffmpeg\bin\ffmpeg-release-shared\ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0\bin\ffmpeg.exe")
FFPROBE_EXE = os.path.abspath(r"spikes\s5-ffmpeg\bin\ffmpeg-release-shared\ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0\bin\ffprobe.exe")
SRC_VIDEO = os.path.abspath(r"spikes\s1-downloader\results\s1c\baseline_5min_r1.mp4")
RESULTS_DIR = os.path.abspath(r"spikes\s4-render\results\quality_s4b")
os.makedirs(RESULTS_DIR, exist_ok=True)


def encode_clip(name: str, encoder: str, preset: str, rc_mode: str = None) -> dict:
    """Encodes a 60s vertical clip (1080x1920 @ 30 fps) from source."""
    out_file = os.path.join(RESULTS_DIR, f"{name}.mp4")
    
    # Base video filter: 60s trim, center crop 0.5 width, scale to 1080x1920 @ 30 fps
    vf = "trim=start=0:end=60,setpts=PTS-STARTPTS,crop=w=iw*0.5:h=ih:x=iw*0.25:y=0,scale=1080:1920,fps=30,format=yuv420p"
    
    cmd = [
        FFMPEG_EXE, "-y", "-hide_banner",
        "-ss", "0", "-i", SRC_VIDEO,
        "-vf", vf,
        "-an" # omit audio to isolate video encoder quality and speed
    ]
    
    if encoder == "h264_amf":
        cmd += ["-c:v", "h264_amf", "-usage", "transcoding", "-profile:v", "high", "-level", "4.2", "-g", "30"]
        if preset == "standard":
            rc = rc_mode if rc_mode else "vbr_latency"
            cmd += ["-rc", rc, "-b:v", "4500k", "-maxrate", "6000k", "-bufsize", "9000k", "-quality", "quality"]
        elif preset == "small_file":
            cmd += ["-rc", "cbr", "-b:v", "2500k", "-maxrate", "2500k", "-bufsize", "5000k"]
    elif encoder == "h264_mf":
        cmd += ["-c:v", "h264_mf", "-g", "30"]
        if preset == "standard":
            cmd += ["-rate_control", "pc_vbr", "-b:v", "4500k"]
        elif preset == "small_file":
            cmd += ["-rate_control", "cbr", "-b:v", "2500k"]
            
    cmd += ["-color_primaries", "bt709", "-color_trc", "bt709", "-colorspace", "bt709", "-color_range", "tv", "-movflags", "+faststart", out_file]
    
    t0 = time.time()
    res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    t1 = time.time()
    wall_time = t1 - t0
    
    if res.returncode != 0:
        raise RuntimeError(f"Encoding failed for {name}: {res.stderr[-500:]}")
        
    size_mb = os.path.getsize(out_file) / (1024.0 * 1024.0)
    bitrate_kbps = (size_mb * 8192.0) / 60.0
    speed_x = 60.0 / wall_time if wall_time > 0 else 0.0
    
    return {
        "name": name,
        "encoder": encoder,
        "preset": preset,
        "rc_mode": rc_mode,
        "output_file": out_file,
        "wall_time_s": round(wall_time, 2),
        "speed_x": round(speed_x, 2),
        "file_size_mb": round(size_mb, 2),
        "bitrate_kbps": round(bitrate_kbps, 1)
    }


def measure_against_scaled_source(encoded_file: str) -> dict:
    """
    Measures SSIM, PSNR, and VMAF of encoded_file directly against SCALED SOURCE.
    Scaled source is generated on the fly via filtergraph from SRC_VIDEO.
    """
    # Stream 0: encoded_file (1080x1920 @ 30 fps)
    # Stream 1: SRC_VIDEO (1920x1080 source) -> cropped to center, scaled to 1080x1920, fps 30
    filter_complex = (
        "[1:v]trim=start=0:end=60,setpts=PTS-STARTPTS,"
        "crop=w=iw*0.5:h=ih:x=iw*0.25:y=0,scale=1080:1920,fps=30,format=yuv420p,split=3[r1][r2][r3];"
        "[0:v]format=yuv420p,split=3[e1][e2][e3];"
        "[e1][r1]ssim=stats_file=-[v_ssim];"
        "[e2][r2]psnr=stats_file=-[v_psnr];"
        "[e3][r3]libvmaf=n_threads=6[v_vmaf]"
    )
    
    cmd = [
        FFMPEG_EXE, "-y", "-hide_banner",
        "-i", encoded_file,
        "-ss", "0", "-i", SRC_VIDEO,
        "-filter_complex", filter_complex,
        "-map", "[v_ssim]", "-map", "[v_psnr]", "-map", "[v_vmaf]",
        "-f", "null", "-"
    ]
    
    res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    
    ssim = None
    psnr = None
    vmaf = None
    
    for line in res.stderr.splitlines():
        if "SSIM" in line and "All:" in line:
            m = re.search(r"All:([0-9.]+)", line)
            if m:
                ssim = float(m.group(1))
        if "PSNR" in line and "average:" in line:
            m = re.search(r"average:([0-9.]+)", line)
            if m:
                psnr = float(m.group(1))
        if "VMAF score:" in line:
            m = re.search(r"VMAF score:\s*([0-9.]+)", line)
            if m:
                vmaf = float(m.group(1))
                
    return {
        "ssim": round(ssim, 5) if ssim is not None else None,
        "psnr_db": round(psnr, 3) if psnr is not None else None,
        "vmaf": round(vmaf, 2) if vmaf is not None else None
    }


def main():
    configs = [
        # (name, encoder, preset, rc_mode)
        ("amf_standard_vbr_latency", "h264_amf", "standard", "vbr_latency"),
        ("amf_standard_vbr_peak", "h264_amf", "standard", "vbr_peak"),
        ("amf_small_file_cbr", "h264_amf", "small_file", None),
        ("mf_standard_pc_vbr", "h264_mf", "standard", None),
        ("mf_small_file_cbr", "h264_mf", "small_file", None),
    ]
    
    results = []
    print("=================================================================")
    print("STARTING OBJECTIVE QUALITY & AMF RATE CONTROL BENCHMARK")
    print("=================================================================")
    
    for name, enc, preset, rc in configs:
        print(f"\nEncoding: {name} (Enc={enc}, Preset={preset}, RC={rc})...", end="", flush=True)
        enc_info = encode_clip(name, enc, preset, rc)
        print(f" Done ({enc_info['wall_time_s']}s, {enc_info['speed_x']}x, {enc_info['bitrate_kbps']} kbps)")
        
        print(f"Measuring quality against SCALED SOURCE...", end="", flush=True)
        q_metrics = measure_against_scaled_source(enc_info["output_file"])
        print(f" SSIM: {q_metrics['ssim']} | PSNR: {q_metrics['psnr_db']} dB | VMAF: {q_metrics['vmaf']}")
        
        record = {
            **enc_info,
            **q_metrics
        }
        results.append(record)
        
    out_json = os.path.join(RESULTS_DIR, "quality_comparison_summary.json")
    with open(out_json, "w", encoding="utf-8") as f:
        json.dump(results, f, indent=2)
        
    print("\n=================================================================")
    print(f"BENCHMARK COMPLETE! Saved summary to {out_json}")
    print("=================================================================")


if __name__ == "__main__":
    main()
