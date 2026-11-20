"""
Fallback Behavior & Error Catalog Simulation for Spike S4
Tests:
1. Automatic fallback across encoder chain (nvenc -> qsv -> amf -> mf -> E_ENCODER_UNAVAILABLE).
2. Real stderr pattern capture for error catalog.
3. Mid-render cancellation / kill cleanup of temporary files.
4. Disk space guard validation (E_DISK_LOW / E_DISK_FULL).
"""

import json
import os
import shutil
import subprocess
import time
from typing import Any, Dict, List, Optional, Tuple

FFMPEG_EXE = os.path.abspath(r"spikes\s5-ffmpeg\bin\ffmpeg-release-shared\ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0\bin\ffmpeg.exe")
RESULTS_DIR = os.path.abspath("spikes/s4-render/results")
os.makedirs(RESULTS_DIR, exist_ok=True)


def test_encoder_probe(encoder: str) -> Tuple[bool, str]:
    """Tests if an encoder can initialize and encode a 1-second synthetic frame."""
    cmd = [
        FFMPEG_EXE, "-y", "-hide_banner",
        "-f", "lavfi", "-i", "color=c=black:s=256x256:d=1:r=30",
        "-c:v", encoder,
        "-f", "null", "-"
    ]
    res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    return (res.returncode == 0, res.stderr)


def run_fallback_chain(preferred_chain: List[str]) -> Dict[str, Any]:
    """
    Simulates the encoder selection pipeline:
    Tries each encoder in priority order. If it fails, records error and falls back to next.
    If all fail, triggers E_ENCODER_UNAVAILABLE.
    """
    report = {
        "chain_attempted": preferred_chain,
        "selected_encoder": None,
        "attempts": [],
        "status": "FAILED",
        "error_code": None,
        "user_message_id": None
    }

    for enc in preferred_chain:
        print(f"Probing encoder: {enc}...")
        ok, stderr_text = test_encoder_probe(enc)
        clean_stderr = "\n".join(stderr_text.splitlines()[-6:]) if stderr_text else ""
        report["attempts"].append({
            "encoder": enc,
            "success": ok,
            "stderr_snippet": clean_stderr
        })

        if ok:
            report["selected_encoder"] = enc
            report["status"] = "OK"
            print(f"  -> Encoder {enc} SUCCESS! Selected for render.")
            return report
        else:
            print(f"  -> Encoder {enc} FAILED. Trying next in chain...")

    # All failed
    report["status"] = "FAILED"
    report["error_code"] = "E_ENCODER_UNAVAILABLE"
    report["user_message_id"] = "err.E_ENCODER_UNAVAILABLE"
    return report


def test_mid_render_cancellation() -> Dict[str, Any]:
    """Simulates job cancellation or kill mid-render and verifies temp file cleanup."""
    temp_output = os.path.join(RESULTS_DIR, "cancel_test_render.tmp")
    final_output = os.path.join(RESULTS_DIR, "cancel_test_render.mp4")

    # Start long 30-second synthetic render with explicit -f mp4 for .tmp file
    cmd = [
        FFMPEG_EXE, "-y", "-hide_banner",
        "-f", "lavfi", "-i", "testsrc=size=1080x1920:rate=30:duration=30",
        "-c:v", "h264_mf", "-b:v", "2000k",
        "-f", "mp4", temp_output
    ]
    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(2.5)  # Let it run for 2.5 seconds so file is flushed to disk

    # Simulate cancellation: kill process
    proc.terminate()
    try:
        proc.wait(timeout=3)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()

    temp_exists_before_cleanup = os.path.exists(temp_output)

    # Job cleanup handler
    if os.path.exists(temp_output):
        os.remove(temp_output)
    if os.path.exists(final_output):
        os.remove(final_output)

    temp_exists_after_cleanup = os.path.exists(temp_output)

    return {
        "temp_file": temp_output,
        "temp_existed_at_kill": temp_exists_before_cleanup,
        "temp_cleaned_up": not temp_exists_after_cleanup,
        "pass": (temp_exists_before_cleanup and not temp_exists_after_cleanup)
    }


def test_disk_guard(free_bytes: int, estimated_output_bytes: int) -> Tuple[bool, Optional[str]]:
    """
    ARCHITECTURE §9 Disk Guard:
    Before render require free space >= 1.5 * estimated output + 500 MB.
    """
    required_bytes = int(1.5 * estimated_output_bytes) + (500 * 1024 * 1024)
    if free_bytes < required_bytes:
        return False, "E_DISK_LOW"
    return True, None


def execute_error_and_fallback_tests():
    print("=== Testing Encoder Fallback Chain ===")
    # Standard priority chain: nvenc -> qsv -> amf -> mf -> unapproved
    chain = ["h264_nvenc", "h264_qsv", "h264_amf", "h264_mf"]
    res_chain = run_fallback_chain(chain)

    print("\n=== Testing Complete Fallback Failure (Simulating E_ENCODER_UNAVAILABLE) ===")
    broken_chain = ["h264_nvenc", "h264_qsv", "non_existent_gpu_enc"]
    res_broken = run_fallback_chain(broken_chain)

    print("\n=== Testing Mid-Render Cancellation & Cleanup ===")
    res_cancel = test_mid_render_cancellation()
    print(f"Cancellation cleanup pass: {res_cancel['pass']}")

    print("\n=== Testing Disk Guard Logic ===")
    # Simulate 60s 1080p clip estimated at ~50 MB
    est_bytes = 50 * 1024 * 1024  # 50 MB
    # Case 1: 10 GB free -> pass
    ok1, code1 = test_disk_guard(free_bytes=10 * 1024 * 1024 * 1024, estimated_output_bytes=est_bytes)
    # Case 2: 400 MB free -> fail with E_DISK_LOW
    ok2, code2 = test_disk_guard(free_bytes=400 * 1024 * 1024, estimated_output_bytes=est_bytes)
    disk_guard_pass = (ok1 is True and code1 is None and ok2 is False and code2 == "E_DISK_LOW")
    print(f"Disk guard logic pass: {disk_guard_pass} (Low disk code: {code2})")

    summary = {
        "encoder_fallback_test": res_chain,
        "forced_encoder_failure_test": res_broken,
        "mid_render_cancellation": res_cancel,
        "disk_guard": {
            "pass": disk_guard_pass,
            "case_normal": {"free_gb": 10, "ok": ok1},
            "case_low": {"free_mb": 400, "ok": ok2, "code": code2}
        }
    }

    out_file = os.path.join(RESULTS_DIR, "fallback_error_test_results.json")
    with open(out_file, "w", encoding="utf-8") as f:
        json.dump(summary, f, indent=2)

    print(f"\nSaved fallback & error test results to {out_file}")


if __name__ == "__main__":
    execute_error_and_fallback_tests()
