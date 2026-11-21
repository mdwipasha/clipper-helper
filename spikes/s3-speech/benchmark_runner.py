import os
import sys
import time
import subprocess
import threading
import json
import ctypes
from ctypes import wintypes

# Win32 definitions for memory and CPU time measurement
class PROCESS_MEMORY_COUNTERS_EX(ctypes.Structure):
    _fields_ = [
        ('cb', wintypes.DWORD),
        ('PageFaultCount', wintypes.DWORD),
        ('PeakWorkingSetSize', ctypes.c_uint64),
        ('WorkingSetSize', ctypes.c_uint64),
        ('QuotaPeakPagedPoolUsage', ctypes.c_uint64),
        ('QuotaPagedPoolUsage', ctypes.c_uint64),
        ('QuotaPeakNonPagedPoolUsage', ctypes.c_uint64),
        ('QuotaNonPagedPoolUsage', ctypes.c_uint64),
        ('PagefileUsage', ctypes.c_uint64),
        ('PeakPagefileUsage', ctypes.c_uint64),
        ('PrivateUsage', ctypes.c_uint64),
    ]

class FILETIME(ctypes.Structure):
    _fields_ = [
        ('dwLowDateTime', wintypes.DWORD),
        ('dwHighDateTime', wintypes.DWORD),
    ]

psapi = ctypes.WinDLL('psapi', use_last_error=True)
kernel32 = ctypes.WinDLL('kernel32', use_last_error=True)

GetProcessMemoryInfo = psapi.GetProcessMemoryInfo
GetProcessMemoryInfo.argtypes = [wintypes.HANDLE, ctypes.POINTER(PROCESS_MEMORY_COUNTERS_EX), wintypes.DWORD]
GetProcessMemoryInfo.restype = wintypes.BOOL

GetProcessTimes = kernel32.GetProcessTimes
GetProcessTimes.argtypes = [
    wintypes.HANDLE,
    ctypes.POINTER(FILETIME),
    ctypes.POINTER(FILETIME),
    ctypes.POINTER(FILETIME),
    ctypes.POINTER(FILETIME),
]
GetProcessTimes.restype = wintypes.BOOL

PROCESS_QUERY_INFORMATION = 0x0400
PROCESS_VM_READ = 0x0010

def filetime_to_seconds(ft):
    val = (ft.dwHighDateTime << 32) | ft.dwLowDateTime
    return val / 10_000_000.0

def run_whisper_benchmark(
    whisper_exe,
    model_path,
    audio_path,
    output_prefix,
    threads=4,
    language="id",
    vad=False,
    vad_model=None,
    dtw=None,
    suppress_nst=True,
    extra_args=None,
    audio_duration_s=60.0,
):
    cmd = [
        whisper_exe,
        "-m", model_path,
        "-f", audio_path,
        "-t", str(threads),
        "-pp",                # print progress
        "-ojf",               # JSON full output with token details
        "-of", output_prefix, # output file prefix
    ]
    if language:
        cmd.extend(["-l", language])
    if vad and vad_model:
        cmd.extend(["--vad", "-vm", vad_model])
    if dtw:
        cmd.extend(["-dtw", dtw])
    if suppress_nst:
        cmd.append("-sns")
    if extra_args:
        cmd.extend(extra_args)

    peak_working_set_bytes = 0
    done_event = threading.Event()

    t_start = time.perf_counter()
    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace"
    )

    h_process = kernel32.OpenProcess(
        PROCESS_QUERY_INFORMATION | PROCESS_VM_READ,
        False,
        proc.pid
    )

    def monitor_memory():
        nonlocal peak_working_set_bytes
        pmc = PROCESS_MEMORY_COUNTERS_EX()
        pmc.cb = ctypes.sizeof(PROCESS_MEMORY_COUNTERS_EX)
        while not done_event.is_set():
            if h_process and GetProcessMemoryInfo(h_process, ctypes.byref(pmc), pmc.cb):
                if pmc.WorkingSetSize > peak_working_set_bytes:
                    peak_working_set_bytes = pmc.WorkingSetSize
                if pmc.PeakWorkingSetSize > peak_working_set_bytes:
                    peak_working_set_bytes = pmc.PeakWorkingSetSize
            time.sleep(0.015)

    monitor_thread = threading.Thread(target=monitor_memory, daemon=True)
    monitor_thread.start()

    stdout, stderr = proc.communicate()
    t_end = time.perf_counter()
    done_event.set()
    monitor_thread.join(timeout=1.0)

    wall_time_s = t_end - t_start

    # Get CPU times
    cpu_util_pct = 0.0
    if h_process:
        creation = FILETIME()
        exit_t = FILETIME()
        kernel_t = FILETIME()
        user_t = FILETIME()
        if GetProcessTimes(h_process, ctypes.byref(creation), ctypes.byref(exit_t), ctypes.byref(kernel_t), ctypes.byref(user_t)):
            k_sec = filetime_to_seconds(kernel_t)
            u_sec = filetime_to_seconds(user_t)
            total_cpu_sec = k_sec + u_sec
            if wall_time_s > 0:
                cpu_util_pct = (total_cpu_sec / wall_time_s) * 100.0
        kernel32.CloseHandle(h_process)

    rtf = wall_time_s / audio_duration_s if audio_duration_s > 0 else 0.0
    peak_ram_mb = peak_working_set_bytes / (1024 * 1024)

    json_file = f"{output_prefix}.json"
    result_data = None
    if os.path.exists(json_file):
        try:
            with open(json_file, "r", encoding="utf-8") as f:
                result_data = json.load(f)
        except Exception as e:
            print(f"Warning: Failed to read {json_file}: {e}")

    return {
        "returncode": proc.returncode,
        "wall_time_s": round(wall_time_s, 3),
        "rtf": round(rtf, 3),
        "peak_ram_mb": round(peak_ram_mb, 2),
        "cpu_util_pct": round(cpu_util_pct, 1),
        "cmd": cmd,
        "stdout": stdout,
        "stderr": stderr,
        "result_data": result_data,
        "json_path": json_file if os.path.exists(json_file) else None,
    }

if __name__ == "__main__":
    print("benchmark_runner module loaded successfully.")
