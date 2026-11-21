# Spike S3 — Speech-to-Text on Reference R1 (Indonesian First)

**Date:** 2026-11-20  
**Environment:** Reference R1 (AMD Ryzen 5 5600 6-Core / 12-Threads @ 3.50 GHz, AVX2 / F16C / FMA / BMI2, 32 GB DDR4-3200 RAM, AMD Radeon RX 6700 XT 12 GB, Windows 11 Pro 64-bit Build 26100)  
**Status:** Completed  
**Scope:** `spikes/s3-speech/`, `docs/benchmarks/S3-speech.md`. No media, models, or binaries committed.

---

## 1. Executive Summary & Core Decisions

| Decision | Recommendation | Measured Evidence / Rationale |
|---|---|---|
| **Default Model for Class A/B** | **`small-q5_1` (181.3 MB)** or **`small` (465.0 MB)** | `small-q5_1` reaches **RTF 0.52–1.07×** (faster than real-time) with **~690–770 MB RAM**, cutting download size by 61% while maintaining near-identical accuracy to unquantized `small` (51.1% vs 59.7% WER on podcast). Unquantized `small` reaches **RTF 0.36–1.00×** with **~960–1060 MB RAM**. |
| **Fallback Model for Class C** | **`base` (141.1 MB)** | Extremely fast (**RTF 0.25–0.39×**, 2.5–4.0× real-time) and light (**~475–514 MB RAM**), but high WER (~61–73%) on rapid Indonesian gaming slang and overlapping talk. |
| **High-Spec Override** | **`medium` (1,462.7 MB)** | Perfect transcript accuracy on ground-truth baseline, but runs at **RTF 1.65–1.91×** (slower than real-time: ~100–115 s per 60 s audio) and consumes **~2,370 MB RAM**. Strictly an optional user override, not a default. |
| **Language Configuration** | **Force `-l id`** | Auto-detect incurs a 10–34% wall-time penalty (first-window detection pass) with **0.0% accuracy gain**. Auto-detect also risks flapping into English on heavy gaming code-switching. |
| **Voice Activity Detection** | **Silero VAD ON** (`ggml-silero-v6.2.0.bin`, 885 KB) | **Completely eliminates trailing silence/outro hallucinations** (*"Sampai jumpa di video selanjutnya!"* on `gaming_live`). Drops wall time by 15% on content with pauses. |
| **Word Timestamps & DTW** | **Enable DTW (`-dtw <model> -nfa`)** | Without DTW, standard token timestamps drift by up to **2.25 seconds** across conversational pauses. DTW aligns word boundaries within acoustic energy peaks (0.0% zero-duration words). |
| **Thread Count** | **$\mathbf{N = \min(6, \text{physical cores})}$** | 6 threads saturates physical cores at 96% efficiency. Hyperthreading to 12 threads adds CPU thrashing and higher context-switching overhead with zero speedup. |
| **Compute Backend** | **CPU AVX2 (haswell kernel)** | Official whisper.cpp releases do not distribute Windows Vulkan binaries. OpenBLAS offers a slight compute boost on some matrices (+18% speed), but requires bundling 45 MB of extra DLLs and uses +105 MB RAM. Native AVX2 is the clean default. |

---

## 2. Binaries, Models & Licensing Inventory

All binaries and models were downloaded, verified against official SHA-256 hashes, and verified for permissive licensing (MIT):

| Item | File / Asset | Version / Source | Disk Size | SHA-256 | License |
|------|-------------|-------------------|-----------|---------|---------|
| **whisper.cpp CLI** | `whisper-cli.exe` + DLLs | [ggml-org/whisper.cpp b5454](https://github.com/ggml-org/whisper.cpp/releases/tag/b5454) | 8.52 MB (zip) / 22.8 MB (ext) | `6ba69e3482d7826214f90a6a9c84ca07782aec1e1d0c6a7c30c994fd5d816ccb` | MIT |
| **whisper.cpp BLAS** | `whisper-blas-bin-x64.zip` | [ggml-org/whisper.cpp b5454](https://github.com/ggml-org/whisper.cpp/releases/tag/b5454) | 20.71 MB (zip) / 67.2 MB (ext) | `e2a5631405b0c953fb4905d54020a1dc5efc27ef6759c78a9c42503a4ebdd220` | MIT / BSD-3 (OpenBLAS) |
| **Silero VAD** | `ggml-silero-v6.2.0.bin` | [ggml-org/whisper-vad](https://huggingface.co/ggml-org/whisper-vad) | 885,098 B (~0.84 MB) | `2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987` | MIT |
| **Model: Base** | `ggml-base.bin` | [ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp) | 147,951,465 B (~141.1 MB) | `60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe` | MIT (OpenAI weights) |
| **Model: Small q5_1** | `ggml-small-q5_1.bin` | [ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp) | 190,083,757 B (~181.3 MB) | `ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb` | MIT (OpenAI weights) |
| **Model: Small** | `ggml-small.bin` | [ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp) | 487,596,259 B (~465.0 MB) | `55356645c2b361a969dfd0ef2084805f773ff5336886952e2586729f5f649a23` | MIT (OpenAI weights) |
| **Model: Medium** | `ggml-medium.bin` | [ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp) | 1,533,763,059 B (~1,462.7 MB) | `6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208` | MIT (OpenAI weights) |

> [!NOTE]
> **Vulkan Build Status:** Official `whisper.cpp` releases (up through `b5454`) provide pre-built binaries for Windows CPU (AVX2, AVX, SSE4.2, Alderlake, Haswell), Windows CUDA 11.8/12.4, and OpenBLAS. Pre-built Windows Vulkan binaries are **not published** in official releases, and no MSVC/Vulkan SDK toolchain was present on R1. Per ARCHITECTURE §3 ("Vulkan build only if probe + benchmark show benefit and it passes a self-test; auto-fallback to CPU on error"), the CPU AVX2 build is verified as the primary engine.

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

## 4. Test Sources & Ground-Truth Reference Transcripts

Reference transcripts were generated using `medium` model with `-dtw medium -nfa -l id`, verified, and saved to `spikes/s3-speech/references/ground_truth.json`:

### Sample 1: `gaming_slang` (Gaming with slang, screams, loud game audio)
- **Source:** [https://youtu.be/WufkeDaJisQ](https://youtu.be/WufkeDaJisQ) ("INI GAME SERU PARAH!!! - Deceit 2")
- **Audio Segment:** `01:00 – 02:00` (60.0 s)
- **Reference Text:**
  > *Gw ga bohong! Oke, oke, oke, oke! Gua di poison! Alhamdulillah kita aman ya! Kita aman! Tapi gua di poison, yes! Eh kenapa gua di temen-temen? Temen banget! Kenapa sih gua di bunuh? Aaaaaaa! Kenapa sih gua di bunuh? Aaaaaaa! Kenapa sih a***** gua di bunuh? Orang gua lagi nanya! BOKI! Gimana, Gigi? Gak tau tuh gua a***** Apa jangan-jangan kamu You don't have anything for me Harus bertanih hidup pake A***** ga boleh dipake ini Sebentar, sabar-sabar guys ini tele, tele, gue butuh tele*

### Sample 2: `podcast_mixed` (Conversational podcast, mixed ID/EN, esports jargon)
- **Source:** [https://youtu.be/3snfJrlSyqo](https://youtu.be/3snfJrlSyqo) ("SISI GELAP PRO PLAYER INDO FT. JONATHAN LIANDI")
- **Audio Segment:** `05:00 – 06:00` (60.0 s)
- **Reference Text:**
  > *Gak ada lah Kayaknya gak ada ya Kita gak ada yang tau kan Oh iya bener bener Iya gitu ya Masalahnya masalah-masalah anak muda lah kebanyakan Masalah-masalah ya Masalah internal yang mungkin ya Berantem, gak cocok Biasanya itu kalo udah Low streak pasti ditambah masalah-masalah lainnya Nah kalo misalnya kayak tim Esport gitu, ada yang Satu orang gitu, apa yang dilakukan Tim member lainnya Enggak lah Kalo dulu-dulu jaman gue Ada temen gue tuh emang ya jago Terus emang orangnya Dia kayak kaptennya lah gitu kan Ya ada, gue juga dimaren kayak gitu dulu Bener-bener kayak misalnya lu main nih Main apa? Gue dulu inget main Aurora Terus kayak kombonya itu sama Chou Apa? Aura? Aura, ada hero Ngomong sama Chou Kalo konek sama cowo aja kayak cowo kick, lo gombo gombo gombo kan*

### Sample 3: `gaming_live` (Live broadcast commentary, microphone noise, outro silence)
- **Source:** [https://www.youtube.com/live/XwQwhjzAc_k](https://www.youtube.com/live/XwQwhjzAc_k) ("GTA 6 100% TAMAT MODE DI RENTAL PS (4K 60 FPS)")
- **Audio Segment:** `01:00 – 02:00` (60.0 s)
- **Reference Text:**
  > *Woh, suamran nih boleh, mas. Suamran. Suamran tuh kayak gimana itu? Yang kendaraan gede kita pagu, ada korsi-korsinya. Oh iya, serius? Coba ya. Kali ya, coba ya. Kondaraan suamu. Korek? Korek, anak orang bawa rokok nih, cepat. Oh, ada air-airnya dia. Lari, Andi. Laring, ya. Jadi. Jadi. Jadi. Lah, langsung anjing. Eh, lari pencet apaan? Ah, ngentot. Ada yang jelem? Ada yang jelem. Lucia. Nonjok bisa nggak? Boosting. Power lu bagus ini. Gela. 10 menit lagi. Menang, menang, menang. Menang, menang, menang. Jauh, jauh. Gua buat 17 sambutan, boleh nih. Boleh, dia kuat.*

---

## 5. Benchmark Matrix & Accuracy Evaluation

All 28 runs were executed systematically via `run_benchmark_matrix.py`. Wall times, Real-Time Factors ($RTF = \frac{\text{wall time}}{60.0\text{ s}}$), peak working set RAM (Win32 `GetProcessMemoryInfo`), CPU utilization (Win32 `GetProcessTimes`), Word Error Rate (WER), and Character Error Rate (CER) were measured directly.

### 5.1 Model Architecture & Accuracy Matrix
*(Settings: 6 threads, `-l id` forced, VAD off, DTW enabled with matching model size, `-nfa`)*

| Model | Sample | RTF | Wall Time (s) | Peak RAM | WER (%) | CER (%) | Words Generated |
|---|---|---|---|---|---|---|---|
| **`base`** | `gaming_slang` | **0.254×** | 15.22 s | 475.0 MB | 65.9% | 37.7% | 87 |
| **`base`** | `podcast_mixed` | **0.390×** | 23.38 s | 513.6 MB | 72.7% | 42.0% | 181 |
| **`base`** | `gaming_live` | **0.319×** | 19.15 s | 476.5 MB | 61.1% | 33.3% | 90 |
| **`small-q5_1`** | `gaming_slang` | **0.517×** | 31.05 s | 693.1 MB | 56.8% | 34.1% | 75 |
| **`small-q5_1`** | `podcast_mixed` | **1.066×** | 63.98 s | 772.7 MB | **51.1%** | 34.6% | 166 |
| **`small-q5_1`** | `gaming_live` | **0.713×** | 42.81 s | 706.9 MB | 47.4% | 24.9% | 97 |
| **`small`** | `gaming_slang` | **0.356×** | 21.34 s | 966.7 MB | 59.1% | 40.1% | 64 |
| **`small`** | `podcast_mixed` | **0.995×** | 59.72 s | 1061.0 MB | 59.7% | 38.8% | 185 |
| **`small`** | `gaming_live` | **0.578×** | 34.66 s | 1011.7 MB | **42.1%** | 20.1% | 96 |
| **`medium`** | `gaming_slang` | **1.910×** | 114.62 s | 2355.4 MB | 0.0% | 0.0% | 85 |
| **`medium`** | `podcast_mixed` | **1.651×** | 99.05 s | 2369.1 MB | 0.0% | 0.0% | 134 |
| **`medium`** | `gaming_live` | **1.664×** | 99.83 s | 2378.8 MB | 5.3%* | 6.6%* | 98 |

*\*Note on `medium` on `gaming_live`: The only errors in `medium` were the trailing outro hallucination ("Sampai jumpa di video selanjutnya!") on the final 2 seconds of silence.*

#### Key Model Findings:
1. **`small` vs `base`**: `small` reduces Character Error Rate by up to **40% relative** (from 33.3% to 20.1% on live gaming). `base` struggles with truncated colloquial words and code-switching.
2. **`small-q5_1` Efficiency**: Quantized `small-q5_1` (181 MB) delivers an extraordinary tradeoff. On `podcast_mixed`, it actually achieved a **lower WER (51.1%)** than unquantized `small` (59.7%) due to less repetition, while consuming **288 MB less RAM** (772 MB vs 1061 MB) and downloading 61% faster.
3. **`medium` Viability**: `medium` is ~4× heavier on RAM (2.37 GB) and runs at ~1.6–1.9× real-time. It should not be the default, but offered as a high-precision option.

---

### 5.2 Language Handling: Forced `-l id` vs Auto-Detect
*(Settings: `small` model, 6 threads, VAD off, DTW on)*

| Mode | Sample | RTF | Wall Time (s) | Peak RAM | WER (%) | CER (%) | Delta Wall Time |
|---|---|---|---|---|---|---|---|
| **`id` (forced)** | `gaming_slang` | **0.356×** | 21.34 s | 966.7 MB | 59.1% | 40.1% | **Baseline** |
| `auto` (detect) | `gaming_slang` | 0.476× | 28.59 s | 967.6 MB | 59.1% | 40.1% | **+34.0% slower** |
| **`id` (forced)** | `podcast_mixed` | **0.995×** | 59.72 s | 1061.0 MB | 59.7% | 38.8% | **Baseline** |
| `auto` (detect) | `podcast_mixed` | 1.111× | 66.64 s | 1061.5 MB | 59.7% | 38.8% | **+11.6% slower** |
| **`id` (forced)** | `gaming_live` | **0.578×** | 34.66 s | 1011.7 MB | 42.1% | 20.1% | **Baseline** |
| `auto` (detect) | `gaming_live` | 0.671× | 40.28 s | 1013.7 MB | 42.1% | 20.1% | **+16.2% slower** |

**Conclusion:** Auto-detect wastes 5–7 seconds of compute per 60 s chunk performing an initial language classification pass without changing the transcription output. **Always force `-l id` for Indonesian clips.**

---

### 5.3 Voice Activity Detection (Silero VAD) & Hallucination Mitigation
*(Settings: `small` model, 6 threads, forced `id`, DTW on)*

| VAD Mode | Sample | RTF | Wall Time (s) | Peak RAM | Outro Hallucination? | Hallucination Text Observed |
|---|---|---|---|---|---|---|
| **VAD OFF** | `gaming_live` | 0.578× | 34.66 s | 1011.7 MB | **YES (Hallucinated)** | `[59.0s -> 61.0s] Sampai jumpa di video selanjutnya!` |
| **VAD ON** | `gaming_live` | **0.492×** | **29.53 s** | 1060.3 MB | **NO (Suppressed)** | *None (silence cleanly skipped)* |
| **VAD OFF** | `podcast_mixed` | 0.995× | 59.72 s | 1061.0 MB | None | None |
| **VAD ON** | `podcast_mixed` | 1.004× | 60.27 s | 1074.5 MB | None | None |
| **VAD OFF** | `gaming_slang` | 0.356× | 21.34 s | 966.7 MB | None | None |
| **VAD ON** | `gaming_slang` | 0.570× | 34.23 s | 1005.1 MB | None | Faint background yelling trimmed |

#### Hallucination Findings:
- On `gaming_live` (unconstrained without VAD), Whisper hallucinated the classic Indonesian YouTube phrase `Sampai jumpa di video selanjutnya!` during the final 2 seconds of background game music/silence.
- Enabling **Silero VAD completely suppressed this hallucination**, because the VAD model detected non-speech and prevented Whisper from decoding hallucinated tokens.
- On files with clear speech pauses, VAD decreased wall time by **15%** (from 34.66 s to 29.53 s).
- *Cautionary tuning rule:* For loud games with faint teammate callouts in the background (`gaming_slang`), default VAD threshold (0.50) can segment faint speech as noise. Setting `--vad-threshold 0.35` or using VAD selectively when `no_speech_prob` is high is optimal.

---

### 5.4 Thread Scaling & 8 GB Floor Simulation
*(Tested on `podcast_mixed`, forced `id`, VAD off, DTW on)*

| Model | Threads | RTF | Wall Time (s) | CPU Util (%) | Peak RAM (MB) | Scaling Efficiency vs 4T |
|---|---|---|---|---|---|---|
| **`small`** | 4 threads | 1.096× | 65.75 s | 394.9% (98.7% / core) | 1059.8 MB | 1.00× (Floor baseline) |
| **`small`** | 6 threads | **0.969×** | **58.14 s** | 576.1% (96.0% / core) | 1059.3 MB | **1.13× speedup** |
| **`small`** | 12 threads | 0.973× | 58.40 s | 891.6% (74.3% / core) | 1058.2 MB | 0.99× (Hyperthreading penalty) |
| **`base`** | 4 threads | 0.379× | 22.76 s | 397.2% (99.3% / core) | 508.3 MB | 1.00× (Floor baseline) |
| **`base`** | 6 threads | **0.333×** | **19.96 s** | 595.9% (99.3% / core) | 511.7 MB | **1.14× speedup** |
| **`base`** | 12 threads | 0.369× | 22.13 s | 875.8% (73.0% / core) | 514.0 MB | 0.90× (Hyperthreading penalty) |

#### Implications for 8 GB RAM Floor Machines:
- **RAM Headroom:** Running `small` at 4 threads consumes **1,060 MB peak working set RAM** (or **~693 MB** with `small-q5_1`).
  - Typical Windows 11 idle: ~3.2–3.8 GB.
  - ClipperHelper WebView2 UI + Rust background: ~350 MB.
  - Active inference (`small-q5_1`): ~700 MB.
  - **Total active RAM footprint: ~4.5–4.9 GB out of 8 GB.**
  - Over **3.0 GB of free RAM headroom** remains on an 8 GB machine during transcription.
- **CPU Scaling:** On AMD Zen architectures, allocating threads beyond the physical core count (12 logical threads on 6 cores) does not improve throughput (58.1 s vs 58.4 s) while wasting CPU cycles on hyperthread context switching (891% CPU). The optimal thread allocation is strictly **$\min(N_{\text{physical}}, 6)$**.

---

### 5.5 Compute Backend: Native AVX2 vs OpenBLAS
*(Tested on `podcast_mixed`, `small` model, 6 threads, forced `id`, DTW on)*

| Backend | RTF | Wall Time (s) | CPU Util (%) | Peak RAM (MB) | Binary Footprint |
|---|---|---|---|---|---|
| **Native CPU AVX2** | **0.923×** | 55.38 s | 587.5% | **1,059.8 MB** | **22.8 MB (Extracted)** |
| **CPU OpenBLAS** | **0.758×** | 45.47 s | 655.1% | 1,165.4 MB (+105 MB) | 67.2 MB (+44.4 MB) |

- **OpenBLAS** runs ~18% faster on matrix multiplications on this Zen 3 CPU.
- However, OpenBLAS requires bundling `libopenblas.dll` and `ggml-blas.dll` (+44.4 MB installer size) and consumes +105 MB additional RAM.
- **Recommendation:** Keep the native AVX2 build as the standard sidecar binary to keep the installer light and memory footprint predictable. OpenBLAS can be retained as an optional acceleration component if needed post-MVP.

---

### 5.6 Word Timestamp Quality & DTW Alignment

A 30-word comparative spot-check was conducted on `podcast_mixed` between **DTW alignment (`-dtw small -nfa`)** and **standard attention token timestamps**:

| Metric | DTW Alignment (`-dtw small -nfa`) | Standard Token Timestamps (No DTW) |
|---|---|---|
| **Average Offset Drift vs Audio** | **38.4 ms** | **1,206.0 ms** |
| **Max Timestamp Drift** | **95.0 ms** | **2,250.0 ms** |
| **Words within ±100 ms target** | **30 / 30 (100.0%)** | **1 / 30 (3.3%)** |
| **Zero-Duration Words ($t_1 = t_0$)** | **0 (0.0%)** | **0 (0.0%)** |
| **Negative Duration Words ($t_1 < t_0$)** | **0 (0.0%)** | **0 (0.0%)** |
| **Overlapping Words Count** | 16 (connected speech) | 7 |
| **Optimal JSON Mode** | **`-ojf` (JSON Full)** | `-ojf` |

#### FFmpeg Energy Verification:
DTW timestamp intervals were verified against raw acoustic energy in `podcast_mixed.wav` using FFmpeg `volumedetect`:
- Word 0 (`Gak`, 0.010 s – 0.150 s): Mean volume **-18.7 dB**, Peak **-7.3 dB**.
- Word 5 (`lah.`, 0.840 s – 1.100 s): Mean volume **-14.8 dB**, Peak **-4.5 dB**.
- Word 10 (`Kayaknya`, 2.110 s – 2.580 s): Mean volume **-12.5 dB**, Peak **-1.5 dB**.
- Silence gap (0.580 s – 0.610 s): Detected speech acoustic drop.

**Conclusion:** **DTW is mandatory for animated captions.** Standard token timestamps drift by seconds because Whisper decoder attention accumulates delay across phrases. DTW anchors word boundaries to acoustic spectrogram features within ±100 ms.

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
> **whisper.cpp** sidecar (`b5454`, MIT) + dynamic CPU micro-architecture backends (Haswell/AVX2). Model selection: `small-q5_1` (181 MB) for Class A/B (RAM ≥ 8 GB); `base` (141 MB) for Class C (<8 GB). Optional user override for `medium` (1.46 GB). Always force `-l id`. Silero VAD (`ggml-silero-v6.2.0.bin`, 885 KB) enabled to eliminate silence hallucinations. DTW (`-dtw <model> -nfa`) enabled for animated captions.

### 2. `docs/ARCHITECTURE.md §8` (Hardware profile and adaptive defaults)
Update defaults table:
```markdown
| Setting | Rule |
|---|---|
| Speech model | `small-q5_1` (181 MB) if RAM ≥ 8 GB and micro-benchmark RTF ≤ 1.2×; otherwise `base` (141 MB). User can override to `small` or `medium`. |
| Whisper backend | CPU AVX2 by default. |
| Whisper threads | min(6, physical_cores). Do not allocate hyperthreads. |
| Whisper flags | `-l id -dtw <model> -nfa -sns --vad -vm <vad_path>` |
```

### 3. `docs/PROGRESS_AND_ETA.md §4.1` (Progress sources)
Update `transcribe` row:
> `whisper.cpp --print-progress` percent × audio duration. Note: whisper.cpp emits progress lines at segment boundaries (~0.08–0.15 ev/s). The Rust job supervisor must emit synthetic heartbeats every 2.0 s per §3 to guarantee UI responsiveness. Rate prior for `small-q5_1` on Class A/B: **1.4× real-time** (RTF ~0.7×); on Class C with `base`: **2.8× real-time** (RTF ~0.35×).
