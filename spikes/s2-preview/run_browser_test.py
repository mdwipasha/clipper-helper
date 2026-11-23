import subprocess
import os
import sys
import time
import json
import shutil
import tempfile
import urllib.request

RESULTS_FILE = os.path.join(os.path.dirname(__file__), "results", "matrix_results.json")

def check_server():
    try:
        with urllib.request.urlopen("http://127.0.0.1:8765", timeout=5) as resp:
            return resp.status == 200
    except Exception as e:
        return False

def find_edge():
    paths = [
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        os.path.expandvars(r"%LOCALAPPDATA%\Microsoft\Edge\Application\msedge.exe")
    ]
    for p in paths:
        if os.path.exists(p):
            return p
    return None

def main():
    if not check_server():
        print("ERROR: Test server is not running on http://127.0.0.1:8765", file=sys.stderr)
        sys.exit(1)

    edge_exe = find_edge()
    if not edge_exe:
        print("ERROR: msedge.exe not found", file=sys.stderr)
        sys.exit(1)

    if os.path.exists(RESULTS_FILE):
        os.remove(RESULTS_FILE)

    temp_profile = tempfile.mkdtemp(prefix="edge_preview_test_")
    print(f"Starting Edge with profile at {temp_profile}...")
    
    cmd = [
        edge_exe,
        f"--user-data-dir={temp_profile}",
        "--no-first-run",
        "--no-default-browser-check",
        "--autoplay-policy=no-user-gesture-required",
        "http://127.0.0.1:8765/?autotest=1"
    ]

    proc = subprocess.Popen(cmd)
    print(f"Edge process launched (PID {proc.pid}). Waiting for matrix results...")

    start_time = time.time()
    max_wait = 90  # 90 seconds timeout
    results = None

    try:
        while time.time() - start_time < max_wait:
            time.sleep(2)
            if os.path.exists(RESULTS_FILE) and os.path.getsize(RESULTS_FILE) > 50:
                try:
                    with open(RESULTS_FILE, "r", encoding="utf-8") as f:
                        data = json.load(f)
                    if data and "results" in data and len(data["results"]) >= 7:
                        results = data
                        print("\nMatrix testing completed successfully!")
                        break
                except Exception:
                    pass
            print(f"Elapsed: {int(time.time() - start_time)}s...", end="\r", flush=True)

        if not results:
            print("\nTimeout waiting for all candidates to complete.")
            if os.path.exists(RESULTS_FILE):
                with open(RESULTS_FILE, "r", encoding="utf-8") as f:
                    results = json.load(f)

    finally:
        print("\nTerminating Edge process...")
        try:
            subprocess.run(["taskkill", "/F", "/T", "/PID", str(proc.pid)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        except Exception:
            proc.kill()
        time.sleep(1)
        shutil.rmtree(temp_profile, ignore_errors=True)

    if results:
        print("\n--- MATRIX TEST RESULTS ---")
        print(json.dumps(results, indent=2))
        return 0
    else:
        print("No results collected.")
        return 1

if __name__ == "__main__":
    sys.exit(main())
