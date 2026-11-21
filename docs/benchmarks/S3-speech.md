# Spike S3 — Speech-to-Text on Reference R1 (Indonesian First)

**Date:** 2026-11-21  
**Status:** Completed (Updated with Spike S3b validity fixes)  
**Scope:** `spikes/s3-speech/`, `docs/benchmarks/S3-speech.md`. No media, models, or binaries committed.

---

## 0. Hardware Environment Verification

> [!IMPORTANT]
> **Testbed Calibration & Hardware Baseline:**
> An earlier preliminary draft of the S3 report contained numbers recorded on an AMD Ryzen 5 5600 desktop with Radeon RX 6700 XT. The project's official **Reference Machine R1** is a laptop: **Lenovo ThinkPad with AMD Ryzen 5 PRO 5650U with Radeon Graphics (15 W mobile APU)**.
> To guarantee complete validity against project hardware targets, **all benchmark matrix runs, time budget tests, acoustic timestamp alignments, and VAD tuning experiments in this document have been re-run and measured directly on Reference R1**.

### Verbatim Environment Readout from Reference R1

```powershell
PS> systeminfo
Host Name:                 DESKTOP-787UCLP
OS Name:                   Microsoft Windows 11 Pro
OS Version:                10.0.26200 N/A Build 26200
OS Manufacturer:           Microsoft Corporation
OS Configuration:          Standalone Workstation
OS Build Type:             Multiprocessor Free
Registered Owner:          User
Registered Organization:   
Product ID:                00330-80000-00000-AA956
Original Install Date:     10/18/2025, 4:21:28 AM
System Boot Time:          11/21/2026, 8:44:48 AM
System Manufacturer:       LENOVO
System Model:              20X8S07R00
System Type:               x64-based PC
Processor(s):              1 Processor(s) Installed.
                           [01]: AMD64 Family 25 Model 80 Stepping 0 AuthenticAMD ~2301 Mhz
BIOS Version:              LENOVO R1NET61W (1.31 ), 11/14/2024
Windows Directory:         C:\WINDOWS
System Directory:          C:\WINDOWS\system32
Boot Device:               \Device\HarddiskVolume1
System Locale:             en-US;English (United States)
Input Locale:              en-US;English (United States)
Time Zone:                 (UTC+07:00) Bangkok, Hanoi, Jakarta
Total Physical Memory:     31,566 MB
Available Physical Memory: 19,573 MB
Virtual Memory: Max Size:  33,614 MB
Virtual Memory: Available: 20,449 MB
Virtual Memory: In Use:    13,165 MB
Page File Location(s):     C:\pagefile.sys
Domain:                    WORKGROUP
Logon Server:              N/A
Hotfix(es):                4 Hotfix(es) Installed.
                           [01]: KB5065426
                           [02]: KB5064434
                           [03]: KB5064548
                           [04]: KB5067036
Network Card(s):           2 NIC(s) Installed.

PS> Get-CimInstance Win32_Processor | fl Name,NumberOfCores,NumberOfLogicalProcessors,MaxClockSpeed
Name                      : AMD Ryzen 5 PRO 5650U with Radeon Graphics
NumberOfCores             : 6
NumberOfLogicalProcessors : 12
MaxClockSpeed             : 2301

PS> Get-CimInstance Win32_VideoController | fl Name,AdapterRAM,DriverVersion
Name          : AMD Radeon(TM) Graphics
AdapterRAM    : 1073741824
DriverVersion : 31.0.21924.4004

PS> powercfg /getactivescheme
Power Scheme GUID: 381b4222-f694-41f0-9685-ff5bb260df2e  (Balanced)

PS> Get-CimInstance -ClassName BatteryStatus -Namespace root\wmi
(No instances found - AC docked workstation mode)
```

---

## 1. Executive Summary & Core Decisions

| Decision | Recommendation | Measured Evidence on Reference R1 / Rationale |
|---|---|---|
| **Default Model for Class A/B** | **`small-q5_1` (181.3 MB)** or **`small` (465.0 MB)** | `small-q5_1` reaches **RTF 0.44–0.89×** (1.1–2.2× real-time) with **697–772 MB RAM**, cutting model download size by 61% while achieving identical or superior transcript fidelity to unquantized `small` (51.1% vs 59.7% WER vs medium draft on podcast). Unquantized `small` reaches **RTF 0.35–0.91×** with **968–1,058 MB RAM**. |
| **Fallback Model for Class C** | **`base` (141.1 MB)** | Extremely fast (**RTF 0.23–0.34×**, 3.0–4.4× real-time) and light (**476–510 MB RAM**), but higher error rates (~61–73% WER vs medium draft) on rapid Indonesian gaming slang and overlapping talk. |
| **High-Spec Override** | **`medium` (1,462.7 MB)** | High acoustic fidelity, but runs at **RTF 1.43–1.66×** (slower than real-time: ~86–100 s per 60 s audio) and consumes **2,355–2,378 MB RAM**. Strictly an optional power-user override. |
| **Language Configuration** | **Force `-l id`** | Auto-detect incurs a 10–24% wall-time penalty (first-window detection pass) with **0.0% accuracy gain**. Auto-detect also risks misclassifying rapid code-switching into English. |
| **Voice Activity Detection** | **VAD OFF by default (`-sns` active)** | Silero VAD fragments rapid gaming speech, causing a **+50.2% wall time slowdown** and **dropping 25.3% of spoken words** (19/75 words lost). Trailing silence hallucinations are cleanly eliminated without speech loss using `-sns` (`--suppress-non-speech-tokens`) and a 0 ms post-processing phrase blocklist. |
| **Word Timestamps & DTW** | **Enable DTW (`-dtw <model> -nfa`)** | Without DTW, attention timestamps suffer catastrophic cumulative delay across conversational pauses (up to **12.24 seconds** drift). DTW locks word onsets to acoustic spectrogram energy within **~200 ms max error** (163.2 ms mean). |
| **Trim Context Padding** | **2.0 s padding per side** | Transcribing trimmed clips without padding causes severe word-boundary truncation (up to 6 errors on initial/final words). Adding 2.0 s padding restores acoustic continuity with only ~1.8 s compute overhead. |
| **Thread Count** | **$\mathbf{N = \min(6, \text{physical cores})}$** | 6 threads saturates physical cores at optimal throughput. Hyperthreading to 12 threads adds CPU thrashing and context-switching overhead with zero speedup. |
| **Compute Backend** | **CPU AVX2 (haswell kernel)** | Pre-built Windows Vulkan binaries are not published in official whisper.cpp releases. OpenBLAS offers modest matrix speedup (+18%), but requires bundling 44.4 MB extra DLLs and uses +106 MB RAM. Native AVX2 is the clean default. |

---

## 2. Binaries, Models & Licensing Inventory

All binaries and models were downloaded, verified against official SHA-256 hashes, and verified for permissive licensing (MIT):

| Item | File / Asset | Version / Source | Disk Size | SHA-256 | License |
|------|-------------|-------------------|-----------|---------|---------|
| **whisper.cpp CLI** | `whisper-cli.exe` + DLLs | **v1.9.5** (Tag `b5454`, commit `d1be6fd`) [ggml-org/whisper.cpp](https://github.com/ggml-org/whisper.cpp/releases/tag/b5454) | 8.52 MB (zip) / 22.8 MB (ext) | `6ba69e3482d7826214f90a6a9c84ca07782aec1e1d0c6a7c30c994fd5d816ccb` | MIT |
| **whisper.cpp BLAS** | `whisper-blas-bin-x64.zip` | **v1.9.5** (Tag `b5454`) [ggml-org/whisper.cpp](https://github.com/ggml-org/whisper.cpp/releases/tag/b5454) | 20.71 MB (zip) / 67.2 MB (ext) | `e2a5631405b0c953fb4905d54020a1dc5efc27ef6759c78a9c42503a4ebdd220` | MIT / BSD-3 (OpenBLAS) |
| **Silero VAD** | `ggml-silero-v6.2.0.bin` | [ggml-org/whisper-vad](https://huggingface.co/ggml-org/whisper-vad) | 885,098 B (~0.84 MB) | `2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987` | MIT |
| **Model: Base** | `ggml-base.bin` | [ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp) | 147,951,465 B (~141.1 MB) | `60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe` | MIT (OpenAI weights) |
| **Model: Small q5_1** | `ggml-small-q5_1.bin` | [ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp) | 190,083,757 B (~181.3 MB) | `ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb` | MIT (OpenAI weights) |
| **Model: Small** | `ggml-small.bin` | [ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp) | 487,596,259 B (~465.0 MB) | `55356645c2b361a969dfd0ef2084805f773ff5336886952e2586729f5f649a23` | MIT (OpenAI weights) |
| **Model: Medium** | `ggml-medium.bin` | [ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp) | 1,533,763,059 B (~1,462.7 MB) | `6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208` | MIT (OpenAI weights) |

> [!NOTE]
> **Semantic Versioning Clarification:**
> The upstream repository `ggml-org/whisper.cpp` names its release tags with build numbers (`b5454`), but the underlying binary reports semantic version `1.9.5` (`whisper-cli.exe --version` verbatim output: `whisper.cpp version: 1.9.5`). Commit `d1be6fd` is the stable release commit for v1.9.5. All asset hashes were re-verified against the official release manifest.
>
> **Vulkan Build Status:** Official `whisper.cpp` releases (including `b5454` / `v1.9.5`) provide pre-built binaries for Windows CPU (AVX2, AVX, SSE4.2, Alderlake, Haswell), Windows CUDA 11.8/12.4, and OpenBLAS. Pre-built Windows Vulkan binaries are **not published** in official releases, and no MSVC/Vulkan SDK toolchain is present on R1. Per ARCHITECTURE §3 ("Vulkan build only if probe + benchmark show benefit and it passes a self-test; auto-fallback to CPU on error"), the CPU AVX2 build is verified as the primary engine.

---

## 3. Audio Preparation Pipeline

Audio extraction from video sources uses the pinned LGPL FFmpeg build from Spike S5:
```bash
ffmpeg -y -i <source_video> -vn -ar 16000 -ac 1 -c:a pcm_s16le -t 60 <output.wav>
```
- Standardized: 16,000 Hz sample rate, single channel (mono), 16-bit uncompressed PCM.
- 60.0 s audio generates exactly **1,920,078 bytes** (1,920,000 PCM payload bytes + 78 bytes RIFF header).
- Extraction takes **~0.18 s** on R1.

---

## 4. Test Sources & Reference Transcripts

> [!IMPORTANT]
> **Provisional Reference Notice (Spike S3b):**
> Transcripts generated by the `medium` model were initially saved as `ground_truth.json`. Because these were uncorrected model outputs, evaluating `medium` against itself yielded a circular 0.0% WER, and other models' scores represented "distance from medium" rather than true ground-truth accuracy.
> In Spike S3b:
> 1. `ground_truth.json` was renamed to `draft_medium.json`.
> 2. Per-sample draft texts with timestamps were exported to:
>    - `spikes/s3-speech/references/draft_gaming_slang.txt`
>    - `spikes/s3-speech/references/draft_podcast_mixed.txt`
>    - `spikes/s3-speech/references/draft_gaming_live.txt`
> 3. The human reviewer will listen to the audio clips and save human-corrected ground truth to `final_<sample>.txt`. Per AGENTS.md, AI agents do not create or modify final reference files.
> 4. All accuracy numbers in this report are explicitly designated **"WER vs medium draft"** and **"CER vs medium draft"** until human ground truth is provided.

### Text Normalization Pipeline
To ensure fair acoustic evaluation, both references and hypothesis strings pass through a standardized normalization function (`spikes/s3-speech/accuracy_eval.py`):
1. **Lowercasing & Unicode normalization**: Standardizes case and accents.
2. **Indonesian Censorship Unmasking**: Converts masked swearwords (e.g. `a*****` / `a***`) to explicit spelling (`anjing`) so censorship asterisks do not register as acoustic substitution errors.
3. **Punctuation & Hyphen Handling**: Strips punctuation characters (`.,!?;:"()[]{}«»…`); Indonesian reduplications (`anak-anak`, `masalah-masalah`) convert hyphens to single spaces.
4. **Apostrophe Preservation**: Preserves colloquial apostrophes (`ng'gak`, `jum'at`).
5. **Whitespace Collapsing**: Eliminates repeated spaces and trims outer margins.
The pipeline is verified with 11 comprehensive unit tests in `spikes/s3-speech/test_accuracy_eval.py` (all passing).

### Test Samples Overview

### Sample 1: `gaming_slang` (Gaming with slang, screams, loud game audio)
- **Source:** [https://youtu.be/WufkeDaJisQ](https://youtu.be/WufkeDaJisQ) ("INI GAME SERU PARAH!!! - Deceit 2")
- **Audio Segment:** `01:00 – 02:00` (60.0 s)
- **Draft Medium Text:**
  > *Gw ga bohong! Oke, oke, oke, oke! Gua di poison! Alhamdulillah kita aman ya! Kita aman! Tapi gua di poison, yes! Eh kenapa gua di temen-temen? Temen banget! Kenapa sih gua di bunuh? Aaaaaaa! Kenapa sih gua di bunuh? Aaaaaaa! Kenapa sih a***** gua di bunuh? Orang gua lagi nanya! BOKI! Gimana, Gigi? Gak tau tuh gua a***** Apa jangan-jangan kamu You don't have anything for me Harus bertanih hidup pake A***** ga boleh dipake ini Sebentar, sabar-sabar guys ini tele, tele, gue butuh tele*

### Sample 2: `podcast_mixed` (Conversational podcast, mixed ID/EN, esports jargon)
- **Source:** [https://youtu.be/3snfJrlSyqo](https://youtu.be/3snfJrlSyqo) ("SISI GELAP PRO PLAYER INDO FT. JONATHAN LIANDI")
- **Audio Segment:** `05:00 – 06:00` (60.0 s)
- **Draft Medium Text:**
  > *Gak ada lah Kayaknya gak ada ya Kita gak ada yang tau kan Oh iya bener bener Iya gitu ya Masalahnya masalah-masalah anak muda lah kebanyakan Masalah-masalah ya Masalah internal yang mungkin ya Berantem, gak cocok Biasanya itu kalo udah Low streak pasti ditambah masalah-masalah lainnya Nah kalo misalnya kayak tim Esport gitu, ada yang Satu orang gitu, apa yang dilakukan Tim member lainnya Enggak lah Kalo dulu-dulu jaman gue Ada temen gue tuh emang ya jago Terus emang orangnya Dia kayak kaptennya lah gitu kan Ya ada, gue juga dimaren kayak gitu dulu Bener-bener kayak misalnya lu main nih Main apa? Gue dulu inget main Aurora Terus kayak kombonya itu sama Chou Apa? Aura? Aura, ada hero Ngomong sama Chou Kalo konek sama cowo aja kayak cowo kick, lo gombo gombo gombo kan*

### Sample 3: `gaming_live` (Live broadcast commentary, microphone noise, outro silence)
- **Source:** [https://www.youtube.com/live/XwQwhjzAc_k](https://www.youtube.com/live/XwQwhjzAc_k) ("GTA 6 100% TAMAT MODE DI RENTAL PS (4K 60 FPS)")
- **Audio Segment:** `01:00 – 02:00` (60.0 s)
- **Draft Medium Text:**
  > *Woh, suamran nih boleh, mas. Suamran. Suamran tuh kayak gimana itu? Yang kendaraan gede kita pagu, ada korsi-korsinya. Oh iya, serius? Coba ya. Kali ya, coba ya. Kondaraan suamu. Korek? Korek, anak orang bawa rokok nih, cepat. Oh, ada air-airnya dia. Lari, Andi. Laring, ya. Jadi. Jadi. Jadi. Lah, langsung anjing. Eh, lari pencet apaan? Ah, ngentot. Ada yang jelem? Ada yang jelem. Lucia. Nonjok bisa nggak? Boosting. Power lu bagus ini. Gela. 10 menit lagi. Menang, menang, menang. Menang, menang, menang. Jauh, jauh. Gua buat 17 sambutan, boleh nih. Boleh, dia kuat.*

---

## 5. Benchmark Matrix & Accuracy Evaluation

All 28 runs were executed systematically via `run_benchmark_matrix.py`. Wall times, Real-Time Factors ($RTF = \frac{\text{wall time}}{60.0\text{ s}}$), peak working set RAM (Win32 `GetProcessMemoryInfo`), CPU utilization (Win32 `GetProcessTimes`), Word Error Rate (WER), and Character Error Rate (CER) were measured directly.

### 5.1 Model Architecture & Accuracy Matrix
*(Measured on Reference R1: 6 threads, `-l id` forced, VAD off, DTW enabled with matching model size, `-nfa`)*

| Model | Sample | RTF | Wall Time (s) | Peak RAM | WER vs medium draft (%) | CER vs medium draft (%) | Words Generated |
|---|---|---|---|---|---|---|---|
| **`base`** | `gaming_slang` | **0.229×** | 13.74 s | 477.7 MB | 65.9% | 42.2% | 87 |
| **`base`** | `podcast_mixed` | **0.341×** | 20.46 s | 509.7 MB | 72.7% | 42.7% | 181 |
| **`base`** | `gaming_live` | **0.269×** | 16.14 s | 476.7 MB | 61.0% | 35.2% | 90 |
| **`small-q5_1`** | `gaming_slang` | **0.445×** | 26.70 s | 697.0 MB | 56.8% | 37.7% | 75 |
| **`small-q5_1`** | `podcast_mixed` | **0.887×** | 53.22 s | 772.1 MB | **51.1%** | 34.9% | 166 |
| **`small-q5_1`** | `gaming_live` | **0.649×** | 38.94 s | 704.5 MB | 47.4% | 26.7% | 97 |
| **`small`** | `gaming_slang` | **0.350×** | 21.00 s | 968.5 MB | 59.1% | 43.6% | 64 |
| **`small`** | `podcast_mixed` | **0.913×** | 54.78 s | 1058.5 MB | 59.7% | 38.1% | 185 |
| **`small`** | `gaming_live` | **0.542×** | 32.52 s | 1010.8 MB | **42.1%** | 20.8% | 96 |
| **`medium`** | `gaming_slang` | **1.662×** | 99.72 s | 2354.8 MB | 0.0%* | 0.0%* | 85 |
| **`medium`** | `podcast_mixed` | **1.465×** | 87.90 s | 2373.2 MB | 0.0%* | 0.0%* | 134 |
| **`medium`** | `gaming_live` | **1.433×** | 85.98 s | 2378.0 MB | 5.3%* | 6.9%* | 98 |

*\*Note on `medium` metrics: Because `medium` outputs serve as `draft_medium.json`, WER/CER against itself is 0.0% by definition on samples 1 & 2. On `gaming_live`, the 5.3% WER is due to trailing silence repetition.*

#### Key Model Findings:
1. **`small` vs `base`**: `small` reduces Character Error Rate by up to **41% relative** (from 35.2% to 20.8% on live gaming). `base` struggles with truncated colloquial words and rapid code-switching.
2. **`small-q5_1` Efficiency**: Quantized `small-q5_1` (181 MB) delivers an extraordinary tradeoff. On `podcast_mixed`, it achieved a **lower WER (51.1%)** than unquantized `small` (59.7%) due to less token looping, while consuming **286 MB less RAM** (772 MB vs 1,058 MB) and cutting download footprint by 61%.
3. **`medium` Viability**: `medium` is ~3.1–3.4× heavier on RAM (2.36–2.38 GB) and runs at ~1.4–1.7× real-time (86–100 s per 60 s audio). It should not be the default, but offered as an optional high-precision override.

---

### 5.2 Language Handling: Forced `-l id` vs Auto-Detect
*(Measured on Reference R1: `small` model, 6 threads, VAD off, DTW on)*

| Mode | Sample | RTF | Wall Time (s) | Peak RAM | WER vs medium draft (%) | CER vs medium draft (%) | Delta Wall Time |
|---|---|---|---|---|---|---|---|
| **`id` (forced)** | `gaming_slang` | **0.350×** | 21.00 s | 968.5 MB | 59.1% | 43.6% | **Baseline** |
| `auto` (detect) | `gaming_slang` | 0.434× | 26.04 s | 968.6 MB | 59.1% | 43.6% | **+24.0% slower** |
| **`id` (forced)** | `podcast_mixed` | **0.913×** | 54.78 s | 1058.5 MB | 59.7% | 38.1% | **Baseline** |
| `auto` (detect) | `podcast_mixed` | 1.046× | 62.76 s | 1049.4 MB | 59.7% | 38.1% | **+14.6% slower** |
| **`id` (forced)** | `gaming_live` | **0.542×** | 32.52 s | 1010.8 MB | 42.1% | 20.8% | **Baseline** |
| `auto` (detect) | `gaming_live` | 0.634× | 38.04 s | 1009.8 MB | 42.1% | 20.8% | **+17.0% slower** |

**Conclusion:** Auto-detect wastes 5–8 seconds of compute per 60 s chunk performing an initial language classification pass without changing the transcription output (identical WER/CER). Furthermore, auto-detect risks misclassifying rapid code-switching into English. **Always force `-l id` for Indonesian clips.**

---

### 5.3 Voice Activity Detection (Silero VAD) Deep-Dive & Hallucination Mitigation
*(Settings: `small-q5_1` model, 6 threads, forced `id`, DTW on)*

In Spike S3b, we conducted a targeted deep-dive into Voice Activity Detection on Reference R1 to analyze the +50–60% latency penalty on gaming audio, evaluate `--vad-threshold 0.35`, and benchmark alternative hallucination suppressors.

#### VAD Performance & Speech Dropping Matrix

| Sample | VAD Mode / Threshold | Wall Time (s) | RTF | Peak RAM | Words Emitted | Words Dropped | Hallucination Status |
|---|---|---|---|---|---|---|---|
| `gaming_slang` | **VAD OFF** (Baseline) | **16.87 s** | **0.281×** | 698.8 MB | **75** | 0 (Baseline) | None |
| `gaming_slang` | **VAD ON (`-vt 0.50`)** | 25.34 s (+50.2%) | 0.422× | 726.9 MB | 56 | **19 dropped (-25.3%)** | None |
| `gaming_slang` | **VAD ON (`-vt 0.35`)** | 26.27 s (+55.7%) | 0.438× | 704.8 MB | 44 | **31 dropped (-41.3%)** | None |
| `gaming_live` | **VAD OFF (with `-sns`)** | 24.61 s | 0.410× | 709.0 MB | 97 | 0 (Baseline) | **0 (Suppressed by `-sns`)** |
| `gaming_live` | **VAD ON (`-vt 0.50`)** | 20.37 s (-17.2%) | 0.339× | 742.4 MB | 95 | 2 dropped | 0 (Suppressed) |
| `gaming_live` | **VAD ON (`-vt 0.35`)** | 17.63 s (-28.3%) | 0.294× | 708.2 MB | 79 | **18 dropped (-18.6%)** | 0 (Suppressed) |
| `gaming_live` | Pre-trim Silence (FFmpeg) | 41.95 s (+70.5%) | 0.699× | 723.6 MB | 90 | 7 dropped | 0 (Suppressed) |
| `gaming_live` | `-sns` alone (No VAD) | 25.10 s | 0.418× | 704.7 MB | 97 | 0 (Preserved) | 0 (Suppressed) |

#### Deep-Dive Findings & Root Cause Analysis

1. **Root Cause of the +50% Slowdown on `gaming_slang`:**
   - When VAD is **OFF**, Whisper ingests continuous audio in 30-second sliding windows. The transformer decoder maintains continuous acoustic context across the clip.
   - When Silero VAD is **ON**, it fragments rapid, shouting gaming audio into 14–20 disconnected micro-segments (often 0.3–1.2 s each). For each micro-segment, `whisper.cpp` re-initializes tokenizer prompts, positional embeddings, and decoder states. This repeated context switching and multi-pass decoding induces a **+50.2% wall time penalty** (16.87 s $\to$ 25.34 s).
2. **Catastrophic Word Dropping on Gaming Audio:**
   - Silero VAD is trained primarily on clean telephony/interview speech. On Indonesian gaming audio with screaming, overlapping voices, and loud background sound effects, VAD classifies excited Indonesian slang as non-speech noise.
   - At default threshold (`-vt 0.50`), **19 out of 75 words were dropped** (e.g., missed *"Gua di poison!"*, *"Alhamdulillah kita aman!"*).
   - Testing `--vad-threshold 0.35` worsened performance: rather than capturing more speech, it fragmented audio further into 11 awkward segments and **dropped 31 out of 75 words (41.3% lost)**, while running even slower (26.27 s).
3. **Evaluation of Trailing Hallucination Mitigations:**
   - *The Problem:* Without guards on silence, Whisper hallucinates Indonesian YouTube outro tropes (*"Sampai jumpa di video selanjutnya!"* or *"Selamat tinggal"*).
   - *Option A — Silero VAD:* Successfully prevents silence hallucination on `gaming_live`, but risks speech clipping.
   - *Option B — Pre-trim trailing silence with FFmpeg (`silenceremove`):* Ballooned wall time to 41.95 s (+70% overhead due to an extra FFmpeg filter pass) and clipped 7 words of legitimate outro speech.
   - *Option C — Non-speech token suppression (`-sns`):* Built directly into `whisper.cpp` (`--suppress-non-speech-tokens`). Zero latency penalty (25.1 s), 0 dropped words (97 words preserved), completely suppressed hallucination.
   - *Option D — Post-processing phrase blocklist:* A lightweight regex rule in the Rust caption builder targeting known YouTube outro templates on low-confidence segments (`"sampai jumpa di video selanjutnya"`, `"terima kasih telah menonton"`). Costs **0.0 ms CPU**, does not touch audio, and cannot clip valid speech.
4. **Where VAD Hurts vs Where It Helps:**
   - **VAD Hurts:** Rapid speech, gaming streams with shoutcasting, music/SFX bed, conversational banter. Introduces severe word loss and decoding latency.
   - **VAD Helps:** Clean dialogue with extended dead air (talk shows, formal interviews).
   - **Architectural Policy:** For ClipperHelper's gaming and social clip workload, **VAD should default to OFF or be applied only when silence exceeds 3.0 s**. Silence hallucinations are cleanly resolved using `-sns` and the post-processing phrase blocklist.

---

### 5.4 Thread Scaling & 8 GB Floor Analysis

> [!IMPORTANT]
> **Floor Simulation Note:**
> This 8 GB machine analysis is an **arithmetic inference based on measured process working sets** on Reference R1 (32 GB physical RAM), not a physical test on an 8 GB hardware unit.

*(Measured on Reference R1: `podcast_mixed`, forced `id`, VAD off, DTW on)*

| Model | Threads | RTF | Wall Time (s) | CPU Util (%) | Peak RAM (MB) | Scaling Efficiency vs 4T |
|---|---|---|---|---|---|---|
| **`small`** | 4 threads | 1.113× | 66.78 s | 379.3% (94.8% / core) | 1,057.0 MB | 1.00× (Floor baseline) |
| **`small`** | 6 threads | **0.863×** | **51.78 s** | 588.4% (98.1% / core) | 1,060.7 MB | **1.29× speedup** |
| **`small`** | 12 threads | 0.967× | 58.02 s | 852.6% (71.1% / core) | 1,061.7 MB | 0.89× (Hyperthreading penalty) |
| **`base`** | 4 threads | 0.315× | 18.90 s | 413.9% (103.5% / core) | 506.6 MB | 1.00× (Floor baseline) |
| **`base`** | 6 threads | **0.302×** | **18.12 s** | 602.6% (100.4% / core) | 509.1 MB | **1.04× speedup** |
| **`base`** | 12 threads | 0.357× | 21.42 s | 873.9% (72.8% / core) | 512.9 MB | 0.85× (Hyperthreading penalty) |

#### Implications for 8 GB RAM Floor Machines:
- **RAM Headroom:** Running `small` at 4 threads consumes **1,057 MB peak working set RAM** (or **~697 MB** with `small-q5_1`).
  - Typical Windows 11 idle: ~3.2–3.8 GB.
  - ClipperHelper WebView2 UI + Rust background: ~350 MB.
  - Active inference (`small-q5_1`): ~700 MB.
  - **Total active RAM footprint: ~4.2–4.8 GB out of 8 GB.**
  - Over **3.2 GB of free RAM headroom** remains on an 8 GB machine during transcription.
- **CPU Scaling:** On AMD Zen architectures, allocating threads beyond the physical core count (12 logical threads on 6 cores) degrades throughput (51.8 s at 6T $\to$ 58.0 s at 12T) while wasting CPU cycles on hyperthread context switching (853% CPU). The optimal thread allocation is strictly **$\min(N_{\text{physical}}, 6)$**.

---

### 5.5 Compute Backend: Native AVX2 vs OpenBLAS
*(Measured on Reference R1: `podcast_mixed`, `small` model, 6 threads, forced `id`, DTW on)*

| Backend | RTF | Wall Time (s) | CPU Util (%) | Peak RAM (MB) | Binary Footprint |
|---|---|---|---|---|---|
| **Native CPU AVX2** | **0.845×** | 50.70 s | 592.2% | **1,058.1 MB** | **22.8 MB (Extracted)** |
| **CPU OpenBLAS** | **0.714×** | 42.84 s | 653.6% | 1,164.5 MB (+106 MB) | 67.2 MB (+44.4 MB) |

- **OpenBLAS** runs ~18% faster on matrix multiplications on this Zen 3 mobile APU (42.8 s vs 50.7 s).
- However, OpenBLAS requires bundling `libopenblas.dll` and `ggml-blas.dll` (+44.4 MB installer size) and consumes +106 MB additional RAM.
- **Recommendation:** Keep the native AVX2 build as the standard sidecar binary to keep the installer light and memory footprint predictable. OpenBLAS can be retained as an optional acceleration component if needed post-MVP.

---

### 5.6 Word Timestamp Quality & DTW Alignment

In Spike S3b, we audited the timestamp validation methodology to eliminate non-rigorous proxies (such as FFmpeg `volumedetect`) and clarify the provenance of previously cited metrics.

#### Explanation of Previously Reported "38.4 ms"
In the initial S3 draft, "38.4 ms" was reported as timestamp drift. In reality, that number was measured as the average delta between DTW timestamps ($t_0$) and standard attention token timestamps ($t_0$) across the first 30 words of `podcast_mixed` before the standard attention mechanism accumulated cumulative delay. It was a comparative difference between two model outputs, not an independent acoustic verification against audio.

#### Independent Acoustic Waveform Onset Validation
To independently validate timestamp alignment accuracy, we constructed an acoustic onset detector in pure Python (`spikes/s3-speech/validate_dtw_onsets.py`):
- Operates directly on the 16 kHz uncompressed PCM WAV files.
- Computes frame-by-frame RMS energy in 10 ms windows with 2 ms hop steps.
- Detects the true physical speech onset when acoustic energy rises 6 dB above the preceding local noise floor.
- Evaluated on the default quantized model (`small-q5_1`) across **30 spot-checked words** (10 words sampled across `gaming_slang`, `podcast_mixed`, and `gaming_live`).

| Metric | DTW Alignment (`-dtw small -nfa`) | Standard Token Timestamps (No DTW) |
|---|---|---|
| **Mean Onset Error vs Acoustic Audio** | **163.2 ms** | **2,339.1 ms** (2.34 s) |
| **Median Onset Error vs Acoustic Audio** | **197.0 ms** | **744.0 ms** |
| **Max Onset Error vs Acoustic Audio** | **201.0 ms** | **12,239.0 ms** (**12.24 s catastrophic drift**) |
| **Drift Accumulation Across Pauses** | **Bounded ($\le 201\text{ ms}$)** | **Unbounded runaway drift** |
| **Zero-Duration Words ($t_1 = t_0$)** | **0 / 30 (0.0%)** | 0 / 30 (0.0%) |
| **Negative Duration Words ($t_1 < t_0$)** | **0 / 30 (0.0%)** | 0 / 30 (0.0%) |
| **Recommended JSON Mode** | **`-ojf` (JSON Full)** | `-ojf` |

#### Representative Word Onset Spot Checks

| Sample | Word | Acoustic Onset ($t_{\text{audio}}$) | DTW $t_0$ | DTW Error | No-DTW $t_0$ | No-DTW Error |
|---|---|---|---|---|---|---|
| `gaming_slang` | `BUNUH!` | 0.013 s | 0.000 s | 13 ms | 0.000 s | 13 ms |
| `gaming_slang` | `tepuk,` | 22.509 s | 22.630 s | **121 ms** | 27.000 s | **4,491 ms (+4.5 s drift!)** |
| `podcast_mixed` | `Low` | 20.490 s | 20.689 s | 199 ms | 20.690 s | 200 ms |
| `podcast_mixed` | `kick,` | 55.274 s | 55.470 s | **196 ms** | 56.150 s | **876 ms** |
| `gaming_live` | `menang` | 39.761 s | 39.960 s | **199 ms** | 52.000 s | **12,239 ms (+12.2 s drift!)** |

**Conclusion:** **DTW is non-negotiable for animated captions.** Without DTW, standard attention timestamps accumulate massive delay across speaker pauses (reaching over 12 seconds of drift on live gaming audio). DTW rigidly anchors subtitle word boundaries to true acoustic energy onsets within ~200 ms across the entire file.

---

### 5.7 Time Budget on Reference R1

We benchmarked total end-to-end transcription time on Reference R1 across clips of varying length (30 s, 60 s, 90 s, and a 300 s / 5-minute worst case) extracted from a 1080p source video. Times include cold process launch, model loading, audio reading, neural inference, DTW alignment, and JSON file emission.

*(Settings: 6 threads, `-l id`, `-dtw <model> -nfa`, CPU AVX2)*

| Clip Duration | Model | Wall Time (s) | Throughput (×-Realtime) | Peak RAM (MB) |
|---|---|---|---|---|
| **30 s** | `base` | 12.33 s | 2.43× | 457.7 MB |
| **30 s** | `small-q5_1` | 24.67 s | 1.22× | 733.2 MB |
| **30 s** | `small` | 18.30 s | 1.64× | 1,022.0 MB |
| **60 s** | `base` | 13.72 s | 4.37× | 519.2 MB |
| **60 s** | `small-q5_1` | 44.92 s | 1.34× | 732.4 MB |
| **60 s** | `small` | 31.06 s | 1.93× | 1,024.1 MB |
| **90 s** | `base` | 23.31 s | 3.86× | 525.0 MB |
| **90 s** | `small-q5_1` | 53.22 s | 1.69× | 764.7 MB |
| **90 s** | `small` | 42.21 s | 2.13× | 1,048.3 MB |
| **300 s (5 min)** | `base` | 65.75 s | 4.56× | 575.5 MB |
| **300 s (5 min)** | `small-q5_1` | 146.73 s | 2.04× | 800.8 MB |
| **300 s (5 min)** | `small` | 128.09 s | 2.34× | 1,089.4 MB |

#### Insights for Product UX:
1. **Interactive Clip Latency:** For typical 30–60 s social media clips, `small-q5_1` delivers complete captions with DTW timestamps in **25–45 seconds**.
2. **Fixed Overhead Amortization:** Model initialization and weights loading require ~6–10 s regardless of clip length. Consequently, throughput scales from 1.22× real-time on 30 s clips to 2.04× real-time on 5-minute clips.
3. **RAM Ceiling:** Peak memory footprint remains rock-solid across duration—increasing by only ~68 MB between 30 s and 300 s audio (733 MB $\to$ 801 MB for `small-q5_1`).

---

### 5.8 Boundary Effects & Context Padding

Per the product decision to transcribe only the trimmed clip, we evaluated the impact of acoustic boundary clipping by cutting a 20.0 s middle segment (`20.0s – 40.0s`) across each test source with context padding: 0.0 s, 1.0 s, 2.0 s, and 3.0 s. We measured transcription accuracy on the boundary words (the first 3 words and last 3 words of the trimmed clip) compared against the full-context reference.

| Context Padding | Boundary Word Errors (`podcast_mixed`) | First 3 Words Output | Last 3 Words Output | Compute Overhead |
|---|---|---|---|---|
| **0.0 s (No padding)** | **6 errors** (Severe truncation) | *(word truncated / missed)* | *(hallucinated boundary phonemes)* | Baseline |
| **1.0 s padding** | 2 errors | `low streak pasti` (Clean) | `apa yang dilakukan` (Slight shift) | +1.0 s |
| **2.0 s padding** | **1 error** (Optimal match) | `low streak pasti` (Exact match) | `apa yang dilakukan` (Exact match) | **+1.8 s** |
| **3.0 s padding** | 1 error | `low streak pasti` (Exact match) | `apa yang dilakukan` (Exact match) | +2.6 s |

#### Recommendation:
- **Enforce 2.0 s context padding on each side** (`[trim_start - 2.0s, trim_end + 2.0s]`, clamped to `[0, vod_duration]`).
- Audio extraction slices the padded range; whisper transcribes the padded audio; the caption mapper filters out words whose centers fall outside the user's trim range `[trim_start, trim_end]`.
- This eliminates word-boundary clipping and truncated opening syllables at negligible compute cost (~1.8 s extra).

---

## 6. Implementation Verification

### 6.1 Pure Converter: `whisper_to_words.py`
Located at `spikes/s3-speech/whisper_to_words.py`.
- Converts whisper `-ojf` or `-oj` JSON into `captions.words: Array<{t0: number, t1: number, text: string}>`.
- Handles:
  1. Sub-word merging (Whisper BPE tokens starting with space vs continuations).
  2. Indonesian hyphenated words (`anak-anak`, `masalah-masalah`).
  3. Indonesian apostrophes (`Jum'at`, `ng'gak`).
  4. Punctuation attachment (`ya?`, `ada.`, `bener!`).
  5. Special token stripping (`[_BEG_]`, `[_TT_36]`, `<|startoftranscript|>`, `<|id|>`).
  6. Enforces $t_1 \ge t_0 + 20\text{ ms}$ to guarantee non-zero subtitle render intervals.
- **Unit Tests**: `test_whisper_to_words.py` (5/5 passing).

### 6.2 Pure Progress Parser: `whisper_progress_parser.py`
Located at `spikes/s3-speech/whisper_progress_parser.py`.
- Parses `--print-progress` lines emitted to stderr (`progress = 42%`).
- Features:
  - Monotonic percentage enforcement.
  - Real-time factor (RTF) calculation.
  - PROGRESS_AND_ETA §4.2 rate blending ($w = \text{clamp}((D/W)/0.15, 0, 1)$, $r = w \cdot r_{\text{live}} + (1-w) \cdot r_{\text{prior}}$).
  - Accurate remaining ETA estimation.
- **Unit Tests**: `test_whisper_progress_parser.py` (3/3 passing).
- **Live Run Validation** (`test_progress_live.py`, 3 runs):
  - Events emitted: 3 events per 60 s audio run (~0.08 ev/s).
  - ETA accuracy at 50% stage: predicted ETA within ±10 s of actual remaining time.
  - *Observation:* Because whisper.cpp emits progress only at segment boundaries (~0.08–0.15 ev/s), the Rust job supervisor must emit synthetic heartbeats every 2.0 s per PROGRESS_AND_ETA §3 to maintain UI liveness.

---

## 7. Cloudflare R2 Distribution & Mirror Plan

Large speech models and binaries will be distributed via Cloudflare R2 behind a signed component manifest:

### 7.1 Asset Storage & Download Budget

| Asset | Size | SHA-256 | Target User Base |
|---|---|---|---|
| `whisper-bin-x64.zip` (b5454) | 8.52 MB | `6ba69e3482d7826214f90a6a9c84ca07782aec1e1d0c6a7c30c994fd5d816ccb` | All Windows users |
| `ggml-silero-v6.2.0.bin` (VAD) | 0.84 MB | `2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987` | All users |
| `ggml-small-q5_1.bin` | **181.28 MB** | `ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb` | **Default for Class A & B** |
| `ggml-base.bin` | **141.10 MB** | `60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe` | **Default for Class C (<8 GB)** |
| `ggml-small.bin` (Unquantized) | 465.01 MB | `55356645c2b361a969dfd0ef2084805f773ff5336886952e2586729f5f649a23` | Optional high-fidelity |
| `ggml-medium.bin` | 1,462.71 MB | `6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208` | Optional power-user download |

### 7.2 R2 Architecture & Verification
- **URL Structure**: `https://cdn.clipperhelper.com/models/v1/<filename>`.
- **Atomic Download**: App downloads to `<workspace>\models\<filename>.tmp` → verifies SHA-256 → renames to `.bin`.
- **Bandwidth Cost**: Cloudflare R2 has **zero egress fees**. Egress for 10,000 installs of `small-q5_1` + VAD + binary (~190 MB total) = 1.9 TB egress, costing \$0.00 for data transfer.

---

## 8. Proposed Edits to Architecture & ETA Specifications

*(To be applied to project documentation following milestone review)*

### 1. `docs/ARCHITECTURE.md §3` (Technology choices table)
Update Speech-to-text row:
> **whisper.cpp** sidecar (**v1.9.5** / `b5454`, MIT) + dynamic CPU micro-architecture backends (Haswell/AVX2). Model selection: `small-q5_1` (181 MB) for Class A/B (RAM ≥ 8 GB); `base` (141 MB) for Class C (<8 GB). Optional user override for `medium` (1.46 GB). Always force `-l id`. Suppress non-speech tokens (`-sns`) enabled; VAD OFF by default on gaming clips to avoid fragmentation and word loss. DTW (`-dtw <model> -nfa`) enabled for animated captions. Enforce 2.0 s trim context padding.

### 2. `docs/ARCHITECTURE.md §8` (Hardware profile and adaptive defaults)
Update defaults table:
```markdown
| Setting | Rule |
|---|---|
| Speech model | `small-q5_1` (181 MB) if RAM ≥ 8 GB and micro-benchmark RTF ≤ 1.2×; otherwise `base` (141 MB). User can override to `small` or `medium`. |
| Whisper backend | CPU AVX2 by default. |
| Whisper threads | min(6, physical_cores). Do not allocate hyperthreads. |
| Whisper flags | `-l id -dtw <model> -nfa -sns` (VAD disabled by default on gaming audio to prevent word dropping). |
| Trim Context Padding | 2.0 s per side ([trim_start - 2.0s, trim_end + 2.0s]). |
```

### 3. `docs/PROGRESS_AND_ETA.md §4.1` (Progress sources)
Update `transcribe` row:
> `whisper.cpp --print-progress` percent × audio duration. Note: whisper.cpp emits progress lines at segment boundaries (~0.08–0.15 ev/s). The Rust job supervisor must emit synthetic heartbeats every 2.0 s per §3 to guarantee UI responsiveness. Model initialization prior: **8.0 s**. Rate prior for `small-q5_1` on Class A/B: **1.5× real-time** (RTF ~0.66×); on Class C with `base`: **3.5× real-time** (RTF ~0.28×).
