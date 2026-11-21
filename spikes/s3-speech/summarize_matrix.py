import json
import os

with open("spikes/s3-speech/results/matrix/matrix_summary.json", "r", encoding="utf-8") as f:
    data = json.load(f)

print("### 1. Model Architecture & Accuracy Matrix (6 threads, id forced, VAD off, DTW on)")
print("| Model | Sample | RTF | Wall Time (s) | Peak RAM (MB) | WER (%) | CER (%) | Words |")
print("|-------|--------|-----|---------------|---------------|---------|---------|-------|")
for r in data:
    if r.get("category") == "model_comparison":
        print(f"| `{r['model']}` | {r['sample']} | {r['rtf']}x | {r['wall_time_s']}s | {r['peak_ram_mb']:.1f} MB | {r['wer']}% | {r['cer']}% | {r['words_count']} |")

print("\n### 2. Language Comparison: id (forced) vs auto (Small model, 6 threads)")
print("| Mode | Sample | RTF | Wall Time (s) | Peak RAM (MB) | WER (%) | CER (%) |")
print("|------|--------|-----|---------------|---------------|---------|---------|")
for r in data:
    if r.get("category") == "language_comparison":
        print(f"| `auto` | {r['sample']} | {r['rtf']}x | {r['wall_time_s']}s | {r['peak_ram_mb']:.1f} MB | {r['wer']}% | {r['cer']}% |")
    elif r.get("category") == "model_comparison" and r.get("model") == "small":
        print(f"| `id` (forced) | {r['sample']} | {r['rtf']}x | {r['wall_time_s']}s | {r['peak_ram_mb']:.1f} MB | {r['wer']}% | {r['cer']}% |")

print("\n### 3. VAD Evaluation: Silero VAD ON vs OFF (Small model, 6 threads, id)")
print("| VAD Mode | Sample | RTF | Wall Time (s) | Peak RAM (MB) | WER (%) | Outro Hallucination? |")
print("|----------|--------|-----|---------------|---------------|---------|-----------------------|")
for r in data:
    if r.get("category") == "vad_comparison":
        h_str = "No (Suppressed)" if not r['has_outro_hallucination'] and r['sample'] == 'gaming_live' else ("Yes" if r['has_outro_hallucination'] else "No")
        print(f"| **VAD ON** | {r['sample']} | {r['rtf']}x | {r['wall_time_s']}s | {r['peak_ram_mb']:.1f} MB | {r['wer']}% | {h_str} |")
    elif r.get("category") == "model_comparison" and r.get("model") == "small":
        h_str = "Yes (Hallucinated)" if r['has_outro_hallucination'] else "No"
        print(f"| VAD OFF | {r['sample']} | {r['rtf']}x | {r['wall_time_s']}s | {r['peak_ram_mb']:.1f} MB | {r['wer']}% | {h_str} |")

print("\n### 4. Thread Scaling & Floor Simulation (podcast_mixed)")
print("| Model | Threads | RTF | Wall Time (s) | CPU Util (%) | Peak RAM (MB) |")
print("|-------|---------|-----|---------------|--------------|---------------|")
for r in data:
    if r.get("category") == "thread_scaling":
        print(f"| `{r['model']}` | {r['threads']} threads | {r['rtf']}x | {r['wall_time_s']}s | {r['cpu_util_pct']}% | {r['peak_ram_mb']:.1f} MB |")

print("\n### 5. Backend Comparison: AVX2 vs OpenBLAS (Small, 6 threads, podcast_mixed)")
print("| Backend | RTF | Wall Time (s) | CPU Util (%) | Peak RAM (MB) |")
print("|---------|-----|---------------|--------------|---------------|")
for r in data:
    if r.get("category") == "backend_comparison":
        print(f"| `{r['backend']}` | {r['rtf']}x | {r['wall_time_s']}s | {r['cpu_util_pct']}% | {r['peak_ram_mb']:.1f} MB |")

print("\n### 6. DTW Word Timestamps Comparison (Small, 6 threads, podcast_mixed)")
print("| DTW Mode | RTF | Wall Time (s) | Zero-dur words | Overlapping words | Words count |")
print("|----------|-----|---------------|----------------|-------------------|-------------|")
for r in data:
    if r.get("category") == "dtw_comparison":
        dtw_str = "DTW ON (-dtw small -nfa)" if r['dtw'] else "DTW OFF (standard attention)"
        print(f"| {dtw_str} | {r['rtf']}x | {r['wall_time_s']}s | {r['zero_dur_count']} | {r['overlap_count']} | {r['words_count']} |")
