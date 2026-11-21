"""
Spike S4b: Operational Validations & Two-Pass Loudness Pipeline
1. Demonstrates automatic encoder fallback chain (NVENC -> QSV -> AMF -> MF).
2. Demonstrates mid-render cancellation and immediate .tmp artifact cleanup.
3. Demonstrates pre-render low disk space check (E_DISK_LOW simulation).
4. Implements two-pass loudness normalization (analysis -> parse -> linear loudnorm)
   and benchmarks against single-pass loudnorm for accuracy and speed.
"""

import json
import os
import re
import shutil
import subprocess
import time
import unittest

FFMPEG_EXE = os.path.abspath(r"spikes\s5-ffmpeg\bin\ffmpeg-release-shared\ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0\bin\ffmpeg.exe")
FFPROBE_EXE = os.path.abspath(r"spikes\s5-ffmpeg\bin\ffmpeg-release-shared\ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0\bin\ffprobe.exe")
SRC_VIDEO = os.path.abspath(r"spikes\s1-downloader\results\s1c\baseline_5min_r1.mp4")
RESULTS_DIR = os.path.abspath(r"spikes\s4-render\results\operational_s4b")
os.makedirs(RESULTS_DIR, exist_ok=True)


# ---------------------------------------------------------------------------
# 1. Automatic Fallback Chain Demonstration
# ---------------------------------------------------------------------------
def demonstrate_encoder_fallback() -> dict:
    """
    Attempts rendering using encoder priority:
    h264_nvenc -> h264_qsv -> h264_amf -> h264_mf -> E_ENCODER_UNAVAILABLE
    Captures stderr and timing at each stage.
    """
    priority_list = ["h264_nvenc", "h264_qsv", "h264_amf", "h264_mf"]
    history = []
    final_output = None
    successful_encoder = None
    
    out_file = os.path.join(RESULTS_DIR, "fallback_test_output.mp4")
    if os.path.exists(out_file):
        os.remove(out_file)
        
    t_start = time.time()
    
    for enc in priority_list:
        t0 = time.time()
        print(f"\n[Fallback Chain] Attempting encoder: {enc}...", end="", flush=True)
        
        # Test 5-second encode to verify pipeline
        cmd = [
            FFMPEG_EXE, "-y", "-hide_banner",
            "-ss", "0", "-t", "5",
            "-i", SRC_VIDEO,
            "-vf", "crop=w=iw*0.5:h=ih:x=iw*0.25:y=0,scale=1080:1920,fps=30,format=yuv420p",
            "-c:v", enc,
            "-b:v", "4500k",
            "-c:a", "aac", "-b:a", "192k",
            out_file
        ]
        
        proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        t_stage = time.time() - t0
        
        entry = {
            "encoder": enc,
            "duration_s": round(t_stage, 3),
            "returncode": proc.returncode,
            "success": (proc.returncode == 0 and os.path.exists(out_file) and os.path.getsize(out_file) > 1000)
        }
        
        if not entry["success"]:
            # Extract key stderr lines indicating failure
            lines = proc.stderr.splitlines()
            err_lines = [l.strip() for l in lines if any(k in l.lower() for k in ["error", "failed", "invalid", "not found", "cannot", "could not"])]
            entry["error_summary"] = err_lines[-3:] if err_lines else lines[-3:]
            print(f" FAILED (code {proc.returncode}) in {entry['duration_s']}s -> Falling back to next...")
            history.append(entry)
        else:
            print(f" SUCCESS in {entry['duration_s']}s!")
            successful_encoder = enc
            final_output = out_file
            history.append(entry)
            break
            
    total_time = time.time() - t_start
    return {
        "priority_list": priority_list,
        "history": history,
        "successful_encoder": successful_encoder,
        "total_fallback_time_s": round(total_time, 3),
        "output_file": final_output
    }


# ---------------------------------------------------------------------------
# 2. Mid-Render Cancellation & Temp Cleanup
# ---------------------------------------------------------------------------
def demonstrate_cancellation_cleanup() -> dict:
    """
    Spawns FFmpeg writing to a temporary file `<job_id>.tmp`.
    Terminates the process mid-stream (kill signal).
    Asserts that the partial `.tmp` file is immediately cleaned up.
    """
    job_id = "job_cancel_test_54321"
    tmp_output = os.path.join(RESULTS_DIR, f"{job_id}.tmp")
    
    if os.path.exists(tmp_output):
        os.remove(tmp_output)
        
    cmd = [
        FFMPEG_EXE, "-y", "-hide_banner",
        "-ss", "0", "-t", "60",
        "-i", SRC_VIDEO,
        "-vf", "crop=w=iw*0.5:h=ih:x=iw*0.25:y=0,scale=1080:1920,fps=30,format=yuv420p",
        "-f", "mp4",
        "-c:v", "h264_amf", "-b:v", "4500k",
        "-c:a", "aac", "-b:a", "192k",
        tmp_output
    ]
    
    print("\n[Cancellation Test] Starting long FFmpeg render writing to .tmp file...")
    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    
    # Wait for process to begin writing partial data
    file_observed_during_run = False
    bytes_written_during_run = 0
    for _ in range(50):
        time.sleep(0.1)
        if os.path.exists(tmp_output) and os.path.getsize(tmp_output) > 10000:
            file_observed_during_run = True
            bytes_written_during_run = os.path.getsize(tmp_output)
            break
            
    print(f"[Cancellation Test] Active render observed: {bytes_written_during_run} bytes written. Sending termination kill signal...")
    
    # Terminate the process (simulating Rust Job Object kill on cancellation)
    proc.terminate()
    proc.wait()
    
    # Simulate supervisor cleanup logic per ARCHITECTURE §7
    cleanup_successful = False
    if os.path.exists(tmp_output):
        os.remove(tmp_output)
        cleanup_successful = not os.path.exists(tmp_output)
    else:
        cleanup_successful = True
        
    exists_after_cleanup = os.path.exists(tmp_output)
    
    return {
        "job_id": job_id,
        "tmp_file_path": tmp_output,
        "file_observed_during_run": file_observed_during_run,
        "bytes_written_before_kill": bytes_written_during_run,
        "process_terminated": (proc.returncode is not None),
        "cleanup_successful": cleanup_successful,
        "orphaned_tmp_exists_after": exists_after_cleanup
    }


# ---------------------------------------------------------------------------
# 3. Low Disk Space Simulation (E_DISK_LOW)
# ---------------------------------------------------------------------------
def check_disk_space_guard(target_dir: str, duration_s: float, bitrate_kbps: float = 4500.0, simulated_free_bytes: int = None) -> dict:
    """
    Implements the pre-render disk space check from PROGRESS_AND_ETA §8.
    Safety formula: required_bytes = 1.5 * estimated_output_bytes + 500 MB headroom.
    If free_bytes < required_bytes, raises E_DISK_LOW without invoking FFmpeg.
    """
    estimated_output_bytes = int((bitrate_kbps * 1000.0 / 8.0) * duration_s)
    headroom_bytes = 500 * 1024 * 1024  # 500 MB
    required_bytes = int(1.5 * estimated_output_bytes + headroom_bytes)
    
    if simulated_free_bytes is not None:
        free_bytes = simulated_free_bytes
    else:
        usage = shutil.disk_usage(target_dir)
        free_bytes = usage.free
        
    if free_bytes < required_bytes:
        return {
            "status": "BLOCKED",
            "error_code": "E_DISK_LOW",
            "message": f"Ruang disk tidak mencukupi untuk export ({free_bytes / (1024*1024):.1f} MB bebas, diperlukan minimal {required_bytes / (1024*1024):.1f} MB)",
            "free_mb": round(free_bytes / (1024 * 1024), 1),
            "required_mb": round(required_bytes / (1024 * 1024), 1),
            "estimated_output_mb": round(estimated_output_bytes / (1024 * 1024), 1)
        }
    else:
        return {
            "status": "PROCEED",
            "free_mb": round(free_bytes / (1024 * 1024), 1),
            "required_mb": round(required_bytes / (1024 * 1024), 1),
            "estimated_output_mb": round(estimated_output_bytes / (1024 * 1024), 1)
        }


# ---------------------------------------------------------------------------
# 4. Two-Pass Loudness Pipeline & Target Justification
# ---------------------------------------------------------------------------
def run_loudness_benchmark(target_lufs: float = -14.0) -> dict:
    """
    Implements and compares:
    1. Single-pass dynamic loudnorm: loudnorm=I=<target>:TP=-1.5:LRA=11
    2. Two-pass linear loudnorm:
       Pass 1: loudnorm=I=<target>:TP=-1.5:LRA=11:print_format=json (measure)
       Pass 2: loudnorm=I=<target>:TP=-1.5:LRA=11:measured_I=...:measured_TP=...:linear=true
    Measures final integrated LUFS accuracy (via ebur128) and wall time.
    """
    raw_audio = os.path.join(RESULTS_DIR, "raw_60s_audio.wav")
    
    # Extract 60s source audio
    cmd_extract = [
        FFMPEG_EXE, "-y", "-hide_banner",
        "-ss", "0", "-t", "60",
        "-i", SRC_VIDEO,
        "-vn", "-c:a", "pcm_s16le",
        raw_audio
    ]
    subprocess.run(cmd_extract, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    
    # --- Single Pass ---
    out_single = os.path.join(RESULTS_DIR, f"audio_single_pass_{target_lufs}lufs.wav")
    t0 = time.time()
    cmd_single = [
        FFMPEG_EXE, "-y", "-hide_banner",
        "-i", raw_audio,
        "-af", f"loudnorm=I={target_lufs}:TP=-1.5:LRA=11",
        "-c:a", "pcm_s16le",
        out_single
    ]
    subprocess.run(cmd_single, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    single_time = time.time() - t0
    
    # --- Two Pass ---
    # Pass 1: Analysis
    t0 = time.time()
    cmd_pass1 = [
        FFMPEG_EXE, "-hide_banner",
        "-i", raw_audio,
        "-af", f"loudnorm=I={target_lufs}:TP=-1.5:LRA=11:print_format=json",
        "-f", "null", "-"
    ]
    res_pass1 = subprocess.run(cmd_pass1, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    pass1_time = time.time() - t0
    
    # Parse JSON from stderr
    stderr_text = res_pass1.stderr
    m = re.search(r"\{[\s\S]*\"input_i\"[\s\S]*\}", stderr_text)
    if not m:
        raise ValueError(f"Could not parse loudnorm pass 1 JSON from stderr:\n{stderr_text[-500:]}")
    p1_stats = json.loads(m.group(0))
    
    # Pass 2: Linear rendering
    out_two_pass = os.path.join(RESULTS_DIR, f"audio_two_pass_{target_lufs}lufs.wav")
    t0 = time.time()
    cmd_pass2 = [
        FFMPEG_EXE, "-y", "-hide_banner",
        "-i", raw_audio,
        "-af", (
            f"loudnorm=I={target_lufs}:TP=-1.5:LRA=11:"
            f"measured_I={p1_stats['input_i']}:"
            f"measured_TP={p1_stats['input_tp']}:"
            f"measured_LRA={p1_stats['input_lra']}:"
            f"measured_thresh={p1_stats['input_thresh']}:"
            f"offset={p1_stats['target_offset']}:"
            f"linear=true"
        ),
        "-c:a", "pcm_s16le",
        out_two_pass
    ]
    subprocess.run(cmd_pass2, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    pass2_time = time.time() - t0
    total_two_pass_time = pass1_time + pass2_time
    
    # Measure final integrated loudness using ebur128 filter
    def measure_lufs(path: str) -> dict:
        cmd_ebur = [
            FFMPEG_EXE, "-hide_banner",
            "-i", path,
            "-af", "ebur128=peak=true",
            "-f", "null", "-"
        ]
        res = subprocess.run(cmd_ebur, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        int_lufs = None
        true_peak = None
        in_summary = False
        for line in res.stderr.splitlines():
            if "Summary:" in line:
                in_summary = True
            if in_summary:
                m_i = re.search(r"^\s*I:\s*([-\d.]+)\s*LUFS", line)
                if m_i:
                    int_lufs = float(m_i.group(1))
                m_tp = re.search(r"^\s*Peak:\s*([-\d.]+)\s*dBFS", line)
                if m_tp:
                    true_peak = float(m_tp.group(1))
        return {"integrated_lufs": int_lufs, "true_peak_dbfs": true_peak}
        
    single_metrics = measure_lufs(out_single)
    two_pass_metrics = measure_lufs(out_two_pass)
    
    single_error = abs(single_metrics["integrated_lufs"] - target_lufs) if single_metrics["integrated_lufs"] is not None else None
    two_pass_error = abs(two_pass_metrics["integrated_lufs"] - target_lufs) if two_pass_metrics["integrated_lufs"] is not None else None
    
    return {
        "target_lufs": target_lufs,
        "pass1_analysis_stats": p1_stats,
        "single_pass": {
            "wall_time_s": round(single_time, 3),
            "measured_lufs": single_metrics["integrated_lufs"],
            "error_lufs": round(single_error, 2) if single_error is not None else None,
            "true_peak_dbfs": single_metrics["true_peak_dbfs"]
        },
        "two_pass": {
            "pass1_time_s": round(pass1_time, 3),
            "pass2_time_s": round(pass2_time, 3),
            "total_wall_time_s": round(total_two_pass_time, 3),
            "measured_lufs": two_pass_metrics["integrated_lufs"],
            "error_lufs": round(two_pass_error, 2) if two_pass_error is not None else None,
            "true_peak_dbfs": two_pass_metrics["true_peak_dbfs"]
        }
    }


def main():
    print("=================================================================")
    print("STARTING OPERATIONAL VALIDATIONS TEST SUITE")
    print("=================================================================")
    
    # 1. Fallback chain
    fallback_res = demonstrate_encoder_fallback()
    
    # 2. Cancellation and temp cleanup
    cleanup_res = demonstrate_cancellation_cleanup()
    
    # 3. Disk space check
    normal_disk = check_disk_space_guard(RESULTS_DIR, duration_s=60.0, bitrate_kbps=4500.0)
    low_disk_sim = check_disk_space_guard(RESULTS_DIR, duration_s=60.0, bitrate_kbps=4500.0, simulated_free_bytes=200 * 1024 * 1024)
    
    # 4. Two-pass Loudness benchmark (-14 vs -16 LUFS)
    print("\n[Loudness Benchmark] Testing target -14.0 LUFS...")
    loud_14 = run_loudness_benchmark(target_lufs=-14.0)
    print(f" -14 LUFS Single Pass: Measured {loud_14['single_pass']['measured_lufs']} LUFS in {loud_14['single_pass']['wall_time_s']}s")
    print(f" -14 LUFS Two Pass:    Measured {loud_14['two_pass']['measured_lufs']} LUFS in {loud_14['two_pass']['total_wall_time_s']}s")
    
    print("\n[Loudness Benchmark] Testing target -16.0 LUFS...")
    loud_16 = run_loudness_benchmark(target_lufs=-16.0)
    print(f" -16 LUFS Single Pass: Measured {loud_16['single_pass']['measured_lufs']} LUFS in {loud_16['single_pass']['wall_time_s']}s")
    print(f" -16 LUFS Two Pass:    Measured {loud_16['two_pass']['measured_lufs']} LUFS in {loud_16['two_pass']['total_wall_time_s']}s")
    
    summary = {
        "fallback_chain": fallback_res,
        "cancellation_cleanup": cleanup_res,
        "disk_space_check": {
            "real_hardware": normal_disk,
            "simulated_low_disk": low_disk_sim
        },
        "loudness_comparison": {
            "target_minus_14": loud_14,
            "target_minus_16": loud_16
        }
    }
    
    out_json = os.path.join(RESULTS_DIR, "operational_validations_summary.json")
    with open(out_json, "w", encoding="utf-8") as f:
        json.dump(summary, f, indent=2)
        
    print("\n=================================================================")
    print(f"OPERATIONAL VALIDATIONS COMPLETE! Saved summary to {out_json}")
    print("=================================================================")


if __name__ == "__main__":
    main()
