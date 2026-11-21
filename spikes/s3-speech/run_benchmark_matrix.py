import os
import sys
import json
import time

sys.path.insert(0, os.path.dirname(__file__))
from benchmark_runner import run_whisper_benchmark
from whisper_to_words import convert_whisper_json_to_words
from accuracy_eval import compute_wer, compute_cer

BASE_DIR = os.path.abspath("spikes/s3-speech")
BIN_AVX2 = os.path.join(BASE_DIR, "bin", "whisper-bin-x64", "Release", "whisper-cli.exe")
BIN_BLAS = os.path.join(BASE_DIR, "bin", "whisper-blas-bin-x64", "Release", "whisper-cli.exe")

MODELS_DIR = os.path.join(BASE_DIR, "models")
AUDIO_DIR = os.path.join(BASE_DIR, "audio")
REFS_DIR = os.path.join(BASE_DIR, "references")
DRAFT_MEDIUM_FILE = os.path.join(REFS_DIR, "draft_medium.json")
RESULTS_DIR = os.path.join(BASE_DIR, "results", "matrix")
os.makedirs(RESULTS_DIR, exist_ok=True)

DRAFT_MEDIUM = {}
if os.path.exists(DRAFT_MEDIUM_FILE):
    with open(DRAFT_MEDIUM_FILE, "r", encoding="utf-8") as f:
        DRAFT_MEDIUM = json.load(f)

def get_reference_for_sample(sample_id):
    final_txt = os.path.join(REFS_DIR, f"final_{sample_id}.txt")
    if os.path.exists(final_txt):
        with open(final_txt, "r", encoding="utf-8") as f:
            lines = [line.strip() for line in f if line.strip() and not line.startswith("---") and not line.startswith("[")]
            return " ".join(lines), "human_final"
    # fallback to draft_medium
    return DRAFT_MEDIUM.get(sample_id, ""), "medium_draft"

SAMPLES = [
    ("gaming_slang", "Gaming Slang (Deceit 2)"),
    ("podcast_mixed", "Podcast Mixed (Jonathan Liandi)"),
    ("gaming_live", "Gaming Live (Windah Basudara)"),
]

MODELS = {
    "base": os.path.join(MODELS_DIR, "ggml-base.bin"),
    "small-q5_1": os.path.join(MODELS_DIR, "ggml-small-q5_1.bin"),
    "small": os.path.join(MODELS_DIR, "ggml-small.bin"),
    "medium": os.path.join(MODELS_DIR, "ggml-medium.bin"),
}

VAD_MODEL = os.path.join(MODELS_DIR, "ggml-silero-v6.2.0.bin")

def evaluate_run(res, sample_id):
    ref_text, ref_kind = get_reference_for_sample(sample_id)
    words = []
    hyp_text = ""
    zero_dur_count = 0
    neg_dur_count = 0
    overlap_count = 0
    
    if res.get("result_data"):
        raw_words = convert_whisper_json_to_words(res["result_data"])
        words = raw_words
        hyp_text = " ".join(w["text"] for w in words)
        
        # Word timestamp anomaly checks
        for i in range(len(words)):
            w = words[i]
            dur = w["t1"] - w["t0"]
            if dur == 0:
                zero_dur_count += 1
            elif dur < 0:
                neg_dur_count += 1
            if i > 0:
                prev_w = words[i - 1]
                if w["t0"] < prev_w["t1"]:
                    overlap_count += 1

    wer = compute_wer(ref_text, hyp_text, normalize=True)
    cer = compute_cer(ref_text, hyp_text, normalize=True)
    
    # Check for hallucination on gaming_live (outro hallucination)
    has_outro_hallucination = False
    if sample_id == "gaming_live":
        if "video selanjutnya" in hyp_text.lower() or "sampai jumpa" in hyp_text.lower():
            has_outro_hallucination = True

    return {
        "ref_kind": ref_kind,
        "wer": round(wer * 100, 2),
        "cer": round(cer * 100, 2),
        "words_count": len(words),
        "zero_dur_count": zero_dur_count,
        "neg_dur_count": neg_dur_count,
        "overlap_count": overlap_count,
        "has_outro_hallucination": has_outro_hallucination,
        "hyp_text": hyp_text,
    }

def main():
    all_results = []
    
    print("=================================================================")
    print("STARTING BENCHMARK MATRIX EXECUTION ON R1")
    print("=================================================================")

    # -------------------------------------------------------------
    # 1. CORE MODEL COMPARISON (base, small-q5_1, small, medium)
    #    x 3 samples, 6 threads, forced id, VAD off, DTW on
    # -------------------------------------------------------------
    print("\n--- Part 1: Model Architecture Comparison (6 threads, id, VAD off, DTW on) ---")
    for m_name in ["base", "small-q5_1", "small", "medium"]:
        m_path = MODELS[m_name]
        dtw_preset = "small" if "small" in m_name else m_name
        for sid, sdesc in SAMPLES:
            run_id = f"model_{m_name}_{sid}"
            out_prefix = os.path.join(RESULTS_DIR, run_id)
            audio_path = os.path.join(AUDIO_DIR, f"{sid}.wav")
            print(f"Running {m_name:10} on {sid:15}...", end="", flush=True)
            
            res = run_whisper_benchmark(
                whisper_exe=BIN_AVX2,
                model_path=m_path,
                audio_path=audio_path,
                output_prefix=out_prefix,
                threads=6,
                language="id",
                vad=False,
                dtw=dtw_preset,
                extra_args=["-nfa"],
            )
            eval_metrics = evaluate_run(res, sid)
            record = {
                "category": "model_comparison",
                "run_id": run_id,
                "model": m_name,
                "sample": sid,
                "threads": 6,
                "language": "id",
                "vad": False,
                "dtw": True,
                "backend": "CPU_AVX2",
                "wall_time_s": res["wall_time_s"],
                "rtf": res["rtf"],
                "peak_ram_mb": res["peak_ram_mb"],
                "cpu_util_pct": res["cpu_util_pct"],
                **eval_metrics
            }
            all_results.append(record)
            print(f" RTF: {res['rtf']}x | RAM: {res['peak_ram_mb']:6.1f}MB | WER: {eval_metrics['wer']:5.1f}% | CER: {eval_metrics['cer']:5.1f}%")

    # -------------------------------------------------------------
    # 2. LANGUAGE: id (forced) vs auto (auto-detect)
    #    Tested with small model on all 3 samples
    # -------------------------------------------------------------
    print("\n--- Part 2: Language Comparison: id (forced) vs auto (Small model, 6 threads) ---")
    for sid, sdesc in SAMPLES:
        run_id = f"lang_auto_{sid}"
        out_prefix = os.path.join(RESULTS_DIR, run_id)
        audio_path = os.path.join(AUDIO_DIR, f"{sid}.wav")
        print(f"Running small [auto] on {sid:15}...", end="", flush=True)
        
        res = run_whisper_benchmark(
            whisper_exe=BIN_AVX2,
            model_path=MODELS["small"],
            audio_path=audio_path,
            output_prefix=out_prefix,
            threads=6,
            language="auto",
            vad=False,
            dtw="small",
            extra_args=["-nfa"],
        )
        eval_metrics = evaluate_run(res, sid)
        record = {
            "category": "language_comparison",
            "run_id": run_id,
            "model": "small",
            "sample": sid,
            "threads": 6,
            "language": "auto",
            "vad": False,
            "dtw": True,
            "backend": "CPU_AVX2",
            "wall_time_s": res["wall_time_s"],
            "rtf": res["rtf"],
            "peak_ram_mb": res["peak_ram_mb"],
            "cpu_util_pct": res["cpu_util_pct"],
            **eval_metrics
        }
        all_results.append(record)
        print(f" RTF: {res['rtf']}x | RAM: {res['peak_ram_mb']:6.1f}MB | WER: {eval_metrics['wer']:5.1f}% | CER: {eval_metrics['cer']:5.1f}%")

    # -------------------------------------------------------------
    # 3. VAD (Voice Activity Detection): ON vs OFF
    #    Tested with small model on all 3 samples
    # -------------------------------------------------------------
    print("\n--- Part 3: VAD Comparison: Silero VAD ON vs OFF (Small model, 6 threads, id) ---")
    for sid, sdesc in SAMPLES:
        run_id = f"vad_on_{sid}"
        out_prefix = os.path.join(RESULTS_DIR, run_id)
        audio_path = os.path.join(AUDIO_DIR, f"{sid}.wav")
        print(f"Running small [VAD ON] on {sid:15}...", end="", flush=True)
        
        res = run_whisper_benchmark(
            whisper_exe=BIN_AVX2,
            model_path=MODELS["small"],
            audio_path=audio_path,
            output_prefix=out_prefix,
            threads=6,
            language="id",
            vad=True,
            vad_model=VAD_MODEL,
            dtw="small",
            extra_args=["-nfa"],
        )
        eval_metrics = evaluate_run(res, sid)
        record = {
            "category": "vad_comparison",
            "run_id": run_id,
            "model": "small",
            "sample": sid,
            "threads": 6,
            "language": "id",
            "vad": True,
            "dtw": True,
            "backend": "CPU_AVX2",
            "wall_time_s": res["wall_time_s"],
            "rtf": res["rtf"],
            "peak_ram_mb": res["peak_ram_mb"],
            "cpu_util_pct": res["cpu_util_pct"],
            **eval_metrics
        }
        all_results.append(record)
        halluc_flag = " [HALLUCINATION SUPPRESSED!]" if (sid == "gaming_live" and not eval_metrics["has_outro_hallucination"]) else ""
        print(f" RTF: {res['rtf']}x | RAM: {res['peak_ram_mb']:6.1f}MB | WER: {eval_metrics['wer']:5.1f}%{halluc_flag}")

    # -------------------------------------------------------------
    # 4. THREAD SCALING & FLOOR SIMULATION (4, 6, 12 threads)
    #    Tested on podcast_mixed with small and base
    # -------------------------------------------------------------
    print("\n--- Part 4: Thread Scaling & Floor Simulation (podcast_mixed) ---")
    for m_name in ["small", "base"]:
        dtw_preset = "small" if "small" in m_name else m_name
        for th in [4, 6, 12]:
            run_id = f"threads_{th}_{m_name}"
            out_prefix = os.path.join(RESULTS_DIR, run_id)
            audio_path = os.path.join(AUDIO_DIR, "podcast_mixed.wav")
            print(f"Running {m_name:6} with {th:2} threads...", end="", flush=True)
            
            res = run_whisper_benchmark(
                whisper_exe=BIN_AVX2,
                model_path=MODELS[m_name],
                audio_path=audio_path,
                output_prefix=out_prefix,
                threads=th,
                language="id",
                vad=False,
                dtw=dtw_preset,
                extra_args=["-nfa"],
            )
            eval_metrics = evaluate_run(res, "podcast_mixed")
            record = {
                "category": "thread_scaling",
                "run_id": run_id,
                "model": m_name,
                "sample": "podcast_mixed",
                "threads": th,
                "language": "id",
                "vad": False,
                "dtw": True,
                "backend": "CPU_AVX2",
                "wall_time_s": res["wall_time_s"],
                "rtf": res["rtf"],
                "peak_ram_mb": res["peak_ram_mb"],
                "cpu_util_pct": res["cpu_util_pct"],
                **eval_metrics
            }
            all_results.append(record)
            print(f" RTF: {res['rtf']}x | CPU: {res['cpu_util_pct']:5.1f}% | RAM: {res['peak_ram_mb']:6.1f}MB")

    # -------------------------------------------------------------
    # 5. BACKEND COMPARISON: Standard AVX2 vs OpenBLAS
    # -------------------------------------------------------------
    print("\n--- Part 5: Backend Comparison: AVX2 vs OpenBLAS (Small, 6 threads, podcast_mixed) ---")
    for b_name, b_exe in [("CPU_AVX2", BIN_AVX2), ("CPU_OpenBLAS", BIN_BLAS)]:
        run_id = f"backend_{b_name}"
        out_prefix = os.path.join(RESULTS_DIR, run_id)
        audio_path = os.path.join(AUDIO_DIR, "podcast_mixed.wav")
        print(f"Running {b_name:15}...", end="", flush=True)
        
        res = run_whisper_benchmark(
            whisper_exe=b_exe,
            model_path=MODELS["small"],
            audio_path=audio_path,
            output_prefix=out_prefix,
            threads=6,
            language="id",
            vad=False,
            dtw="small",
            extra_args=["-nfa"],
        )
        eval_metrics = evaluate_run(res, "podcast_mixed")
        record = {
            "category": "backend_comparison",
            "run_id": run_id,
            "model": "small",
            "sample": "podcast_mixed",
            "threads": 6,
            "language": "id",
            "vad": False,
            "dtw": True,
            "backend": b_name,
            "wall_time_s": res["wall_time_s"],
            "rtf": res["rtf"],
            "peak_ram_mb": res["peak_ram_mb"],
            "cpu_util_pct": res["cpu_util_pct"],
            **eval_metrics
        }
        all_results.append(record)
        print(f" RTF: {res['rtf']}x | CPU: {res['cpu_util_pct']:5.1f}% | RAM: {res['peak_ram_mb']:6.1f}MB")

    # -------------------------------------------------------------
    # 6. WORD TIMESTAMPS: DTW ON vs DTW OFF
    # -------------------------------------------------------------
    print("\n--- Part 6: Word Timestamps: DTW ON vs DTW OFF (podcast_mixed, Small, 6 threads) ---")
    for dtw_val, dtw_flag in [(True, "small"), (False, None)]:
        run_id = f"dtw_{dtw_val}"
        out_prefix = os.path.join(RESULTS_DIR, run_id)
        audio_path = os.path.join(AUDIO_DIR, "podcast_mixed.wav")
        extra = ["-nfa"] if dtw_val else ["-fa"]
        print(f"Running DTW={'ON' if dtw_val else 'OFF'}...", end="", flush=True)
        
        res = run_whisper_benchmark(
            whisper_exe=BIN_AVX2,
            model_path=MODELS["small"],
            audio_path=audio_path,
            output_prefix=out_prefix,
            threads=6,
            language="id",
            vad=False,
            dtw=dtw_flag,
            extra_args=extra,
        )
        eval_metrics = evaluate_run(res, "podcast_mixed")
        record = {
            "category": "dtw_comparison",
            "run_id": run_id,
            "model": "small",
            "sample": "podcast_mixed",
            "threads": 6,
            "language": "id",
            "vad": False,
            "dtw": dtw_val,
            "backend": "CPU_AVX2",
            "wall_time_s": res["wall_time_s"],
            "rtf": res["rtf"],
            "peak_ram_mb": res["peak_ram_mb"],
            "cpu_util_pct": res["cpu_util_pct"],
            **eval_metrics
        }
        all_results.append(record)
        print(f" RTF: {res['rtf']}x | zero_dur: {eval_metrics['zero_dur_count']} | overlap: {eval_metrics['overlap_count']}")

    # Save summary matrix JSON
    summary_path = os.path.join(RESULTS_DIR, "matrix_summary.json")
    with open(summary_path, "w", encoding="utf-8") as f:
        json.dump(all_results, f, indent=2, ensure_ascii=False)
    print(f"\n=================================================================")
    print(f"BENCHMARK MATRIX COMPLETED! Saved {len(all_results)} runs to {summary_path}")
    print("=================================================================")

if __name__ == "__main__":
    main()
