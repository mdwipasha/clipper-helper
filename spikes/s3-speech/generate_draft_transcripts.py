import os
import sys
import json
from benchmark_runner import run_whisper_benchmark
from whisper_to_words import convert_whisper_json_to_words

BASE_DIR = os.path.abspath("spikes/s3-speech")
WHISPER_EXE = os.path.join(BASE_DIR, "bin", "whisper-bin-x64", "Release", "whisper-cli.exe")
MODEL_PATH = os.path.join(BASE_DIR, "models", "ggml-medium.bin")
AUDIO_DIR = os.path.join(BASE_DIR, "audio")
DRAFTS_DIR = os.path.join(BASE_DIR, "results", "drafts")
os.makedirs(DRAFTS_DIR, exist_ok=True)

SAMPLES = [
    ("gaming_slang", "Gaming (Deceit 2) [01:00 - 02:00]"),
    ("podcast_mixed", "Podcast Mixed ID/EN (Jonathan Liandi) [05:00 - 06:00]"),
    ("gaming_live", "Gaming Livestream (Windah Basudara GTA 6) [01:00 - 02:00]"),
]

def main():
    drafts = {}
    for sid, desc in SAMPLES:
        audio_path = os.path.join(AUDIO_DIR, f"{sid}.wav")
        out_prefix = os.path.join(DRAFTS_DIR, sid)
        print(f"\n==========================================")
        print(f"Generating draft for: {desc}")
        print(f"==========================================")
        
        res = run_whisper_benchmark(
            whisper_exe=WHISPER_EXE,
            model_path=MODEL_PATH,
            audio_path=audio_path,
            output_prefix=out_prefix,
            threads=6,
            language="id",
            vad=False,
            dtw="medium",
            extra_args=["-nfa"],
        )
        print(f"Wall time: {res['wall_time_s']}s (RTF: {res['rtf']}x), Peak RAM: {res['peak_ram_mb']} MB")
        
        words = []
        full_text = ""
        segments_summary = []
        if res["result_data"]:
            words = convert_whisper_json_to_words(res["result_data"])
            transcription = res["result_data"].get("transcription", [])
            for seg in transcription:
                t_from = seg.get("timestamps", {}).get("from", "")
                t_to = seg.get("timestamps", {}).get("to", "")
                text = seg.get("text", "").strip()
                if text:
                    segments_summary.append(f"[{t_from} --> {t_to}] {text}")
            full_text = " ".join(s.split("] ", 1)[-1] for s in segments_summary)

        drafts[sid] = {
            "description": desc,
            "full_text": full_text,
            "segments": segments_summary,
            "words_count": len(words),
            "wall_time_s": res["wall_time_s"],
            "rtf": res["rtf"],
        }
        
        # Write human-readable draft text file
        with open(os.path.join(DRAFTS_DIR, f"{sid}_draft.txt"), "w", encoding="utf-8") as f:
            f.write(f"SAMPLE: {desc}\n")
            f.write(f"MODEL: Medium (ggml-medium.bin)\n")
            f.write(f"RTF: {res['rtf']}x, Wall time: {res['wall_time_s']}s\n\n")
            f.write("--- FULL TEXT ---\n")
            f.write(full_text + "\n\n")
            f.write("--- SEGMENTS ---\n")
            for seg_line in segments_summary:
                f.write(seg_line + "\n")
        
        # Save words JSON
        with open(os.path.join(DRAFTS_DIR, f"{sid}_words.json"), "w", encoding="utf-8") as f:
            json.dump(words, f, indent=2, ensure_ascii=False)

    with open(os.path.join(DRAFTS_DIR, "all_drafts.json"), "w", encoding="utf-8") as f:
        json.dump(drafts, f, indent=2, ensure_ascii=False)
    print("\nAll draft transcripts generated!")

if __name__ == "__main__":
    main()
