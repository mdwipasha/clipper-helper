import subprocess
import os

BASE_DIR = os.path.abspath("spikes/s3-speech")
WHISPER_EXE = os.path.join(BASE_DIR, "bin", "whisper-bin-x64", "Release", "whisper-cli.exe")
MODEL_PATH = os.path.join(BASE_DIR, "models", "ggml-base.bin")
AUDIO_PATH = os.path.join(BASE_DIR, "audio", "gaming_slang.wav")

cmd = [
    WHISPER_EXE,
    "-m", MODEL_PATH,
    "-f", AUDIO_PATH,
    "-t", "4",
    "-pp",
]

proc = subprocess.run(cmd, capture_output=True, text=True, errors="replace")
print("--- STDOUT (first 10) ---")
for line in proc.stdout.splitlines()[:10]:
    print("O:", line)
print("--- STDERR (lines with progress) ---")
progress_lines = [line for line in proc.stderr.splitlines() if "progress" in line.lower()]
print(f"Total progress lines in stderr: {len(progress_lines)}")
for line in progress_lines[:10]:
    print("E:", line)
