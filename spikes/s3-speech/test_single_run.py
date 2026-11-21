import os
import json
from benchmark_runner import run_whisper_benchmark

BASE_DIR = os.path.abspath("spikes/s3-speech")
WHISPER_EXE = os.path.join(BASE_DIR, "bin", "whisper-bin-x64", "Release", "whisper-cli.exe")
MODEL_PATH = os.path.join(BASE_DIR, "models", "ggml-small.bin")
AUDIO_PATH = os.path.join(BASE_DIR, "audio", "podcast_mixed.wav")
OUT_DIR = os.path.join(BASE_DIR, "results", "test_run")
os.makedirs(OUT_DIR, exist_ok=True)
OUT_PREFIX = os.path.join(OUT_DIR, "podcast_small")

res = run_whisper_benchmark(
    whisper_exe=WHISPER_EXE,
    model_path=MODEL_PATH,
    audio_path=AUDIO_PATH,
    output_prefix=OUT_PREFIX,
    threads=6,
    language="id",
    vad=False,
    dtw="small",
)

print(f"Return code: {res['returncode']}")
print(f"Wall time: {res['wall_time_s']}s")
print(f"RTF: {res['rtf']}x (realtime factor)")
print(f"Peak RAM: {res['peak_ram_mb']} MB")
print(f"CPU util: {res['cpu_util_pct']}%")

# Print first few lines of stdout
lines = res["stdout"].strip().split("\n")
print(f"\nStdout line count: {len(lines)}")
for line in lines[:10]:
    print("  OUT:", line)

# Check JSON output
if res["result_data"]:
    print(f"\nJSON keys: {list(res['result_data'].keys())}")
    transcription = res["result_data"].get("transcription", [])
    print(f"Segments count: {len(transcription)}")
    if transcription:
        print("First segment keys:", list(transcription[0].keys()))
        print("First segment text:", transcription[0].get("text"))
        print("First segment timestamps:", transcription[0].get("timestamps"))
        tokens = transcription[0].get("tokens", [])
        print(f"First segment tokens count: {len(tokens)}")
        if tokens:
            print("First 3 tokens:", tokens[:3])
