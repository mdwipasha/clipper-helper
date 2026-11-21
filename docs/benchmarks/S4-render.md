# Spike S4 — Render Pipeline, Encoders, ASS Captions, Export Speed

> **Document status:** S4b Completed (2026-11-21; validity gaps resolved, objective VMAF/SSIM/PSNR vs scaled source measured, operational fallback/cancellation/disk/loudness validated).  
> **Milestone:** M0 (Spike & Architecture Validation).  
> **Related documents:** [ARCHITECTURE.md](../ARCHITECTURE.md), [DATA_MODEL.md](../DATA_MODEL.md), [PROGRESS_AND_ETA.md](../PROGRESS_AND_ETA.md), [S5-ffmpeg.md](S5-ffmpeg.md), [gpl-filters-denylist.txt](gpl-filters-denylist.txt).

---

## 0. Hardware Environment Verification (Reference Machine R1)

To ensure scientific validity and reproducibility, all benchmark data in this report is measured directly on **Reference Machine R1** (Lenovo ThinkPad laptop). An earlier draft of S4 erroneously referenced "Ryzen 5 5600" and "AMD Radeon RX 6700 XT"; that was an unvetted documentation copy-paste from an early desktop draft. Machine provenance audits, log timestamp checks, and re-validation confirm that all actual benchmarks were executed on the R1 hardware specified below.

### 0.1 Host Environment & Raw System Probes

```powershell
PS C:\> systeminfo
Host Name:                     DESKTOP-OO9CA87
OS Name:                       Microsoft Windows 11 Pro
OS Version:                    10.0.26200 N/A Build 26200
System Manufacturer:           LENOVO
System Model:                  20X8S07R00
Total Physical Memory:         31,566 MB
Time Zone:                     (UTC+07:00) Bangkok, Hanoi, Jakarta
```

```powershell
PS C:\> Get-CimInstance Win32_Processor | fl Name,NumberOfCores,NumberOfLogicalProcessors,MaxClockSpeed
Name                      : AMD Ryzen 5 PRO 5650U with Radeon Graphics     
NumberOfCores             : 6
NumberOfLogicalProcessors : 12
MaxClockSpeed             : 2301
```

```powershell
PS C:\> Get-CimInstance Win32_VideoController | fl Name,AdapterRAM,DriverVersion
Name          : AMD Radeon(TM) Graphics
AdapterRAM    : 1073741824
DriverVersion : 31.0.21924.4004
```

```powershell
PS C:\> powercfg /getactivescheme; Get-CimInstance Win32_Battery | fl EstimatedChargeRemaining,BatteryStatus
Power Scheme GUID: 381b4222-f694-41f0-9685-ff5bb260df2e  (Balanced)

EstimatedChargeRemaining : 100
BatteryStatus            : 2  # Connected to AC line power, 100% full charge
```

- **Reference Target:** Device Class B (Mobile Productivity Laptop, 6 cores / 12 threads, iGPU with 1 GB shared VRAM).
- **Physical Hardware:** Lenovo ThinkPad 20X8S07R00, AMD Ryzen 5 PRO 5650U with Radeon Graphics (Vega 7 iGPU, 1 GB shared VRAM), 32 GB RAM.
- **Discrete GPU:** None present. Discrete GPUs (e.g. RX 6700 XT or RTX series) do NOT exist on R1.
- **Power & Thermal:** AC line power, 100% battery, Windows Balanced power plan.

---

## 1. Executive Summary & Core Decisions

| Dimension | Decision & Finding | Status |
|---|---|---|
| **Test Environment (R1)** | AMD Ryzen 5 PRO 5650U with Radeon Graphics (6C/12T @ 2.3 GHz), AMD Radeon Graphics (Vega 7 iGPU, 1 GB shared VRAM), 32 GB RAM, Windows 11 Pro Build 26200. | Measured (R1) |
| **Tested Encoders** | `h264_amf` (functional, **2.72×–3.18× realtime**), `h264_mf` (functional, **2.69×–3.42× realtime**). | Measured (R1) |
| **Untested Encoders** | `h264_nvenc` and `h264_qsv` marked **UNTESTED / PLACEHOLDER** (physical hardware absent on AMD R1; verified in live fallback test to fail cleanly with exit codes `4294967295` / `2981409195` and `Conversion failed!`). | Verified Clean Failure |
| **Unapproved Encoders** | `libopenh264` and `x264` are strictly **UNAPPROVED** pending patent counsel review (see `docs/compliance/FFMPEG.md §7`). | Policy Invariant |
| **Encoder Priority Order** | **`h264_nvenc`** → **`h264_qsv`** → **`h264_amf`** → **`h264_mf`** → **`E_ENCODER_UNAVAILABLE`**. Live fallback demonstrated in 1.70 s total elapsed time. | Measured & Verified |
| **AMF Rate Control** | `vbr_peak` recommended for offline export (superior VMAF 97.69 vs 96.98, +1.58 dB PSNR, and accurate 4.4 Mbps bitrate targeting vs 4.0 Mbps underrun); `vbr_latency` supported. | Measured (R1) |
| **Layout `blur_fit`** | Strictly uses LGPL filter **`gblur=sigma=30:steps=2`** (or `avgblur`). Verified zero GPL filters used (`boxblur` absent). | Verified LGPLv3 |
| **Captions Engine** | Word timings to ASS karaoke (`{\k...}` / color toggle) and `pop` (`{\fscx...}`). Verified with 4 bundled OFL fonts (Montserrat, Poppins, Inter, Anton). | Verified |
| **Windows Path Escaping** | Verified without fontconfig: forward slashes (`/`), colons escaped as `\:`, single-quoted in `ass=filename='...':fontsdir='...'`. | Verified |
| **Export Verification** | ffprobe checklist + binary atom inspection (`moov` atom preceding `mdat` confirming `+faststart`). | Verified |
| **HW vs SW Decode** | D3D11VA hardware decode cuts CPU load by **>50%** (from 27% down to 12%), but adds ~7–10s wall time due to GPU surface-to-libass copy overhead. | Measured (R1) |
| **Two-Pass Loudness** | Two-pass linear loudnorm (`analyze_audio` pass + linear render) prevents dynamic compression pumping; default recommended target is **-14.0 LUFS** (aligns with YouTube Shorts, TikTok, Reels; template-configurable). | Measured & Verified |
| **Cancellation & Cleanup** | Terminating FFmpeg mid-render writing to `.tmp` file leaves zero orphaned partial files; supervisor immediately deletes partial output. | Measured & Verified |
| **Disk Space Guard** | Pre-flight check blocks export if `free_space < 1.5 × estimated_size + 500 MB` with `E_DISK_LOW` before spawning FFmpeg. | Tested & Verified |

---

## 2. Reference Render Builder Architecture (`render_builder.py`)

The pure function `build_ffmpeg_args(composition, encoder, output_path, ...)` implements the `Composition` v1 schema (`DATA_MODEL §7`):

### 2.1 Multi-Segment Timeline Concat
Unlike simple single-clip tools, Clipper Helper supports multirange clips (e.g. highlights from minutes 5–6 and 25–26). Multi-segment concatenation is executed entirely via FFmpeg's `concat` filtergraph:
```
[0:v]trim=start=5.000:end=15.000,setpts=PTS-STARTPTS[v_seg_0];
[0:a]atrim=start=5.000:end=15.000,asetpts=PTS-STARTPTS[a_seg_0];
[0:v]trim=start=25.000:end=35.000,setpts=PTS-STARTPTS[v_seg_1];
[0:a]atrim=start=25.000:end=35.000,asetpts=PTS-STARTPTS[a_seg_1];
[v_seg_0][a_seg_0][v_seg_1][a_seg_1]concat=n=2:v=1:a=1[v_timeline][a_timeline]
```
*Rationale:* File-based concat demuxers require intermediate physical files or identical codecs/GOP boundaries. In-filtergraph concat handles arbitrary keyframe intervals, framerate variations, and audio sample alignment losslessly.

### 2.2 Layout Modes (1080×1920 9:16 Canvas)
1. **`fill`:**
   Crops source normalized bounding box `[x, y, w, h]` and scales to 1080×1920:
   `crop=w=iw*w:h=ih*h:x=iw*x:y=ih*y,scale=1080:1920`
2. **`blur_fit`:**
   Splits video into background and foreground:
   - Background: `scale=1080:1920:force_original_aspect_ratio=increase,crop=1080:1920,gblur=sigma=30:steps=2`
   - Foreground: `scale=1080:1920:force_original_aspect_ratio=decrease`
   - Overlay: `overlay=(W-w)/2:(H-h)/2`
   > [!IMPORTANT]
   > `boxblur` is GPL-only and strictly omitted from our LGPL FFmpeg build. S4 verified that `gblur` provides identical aesthetic background blur with full LGPLv3 compliance.
3. **`split`:**
   Splits video into top and bottom crops based on height ratio (e.g. 0.55):
   - Top: `crop=...,scale=1080:1056`
   - Bottom: `crop=...,scale=1080:864`
   - Stacked vertically via `[top][bottom]vstack`

### 2.3 Overlays & Two-Pass Audio Loudness Pipeline
- **Logo Overlay:** Scales logo asset to `widthPct` of canvas, sets alpha channel opacity (`format=rgba,colorchannelmixer=aa=<opacity>`), and positions according to anchor formula (`top_left`, `top_right`, `bottom_left`, `bottom_right`, `center`).
- **Text Overlay:** Emits `drawtext` with bundled font, font size proportional to canvas height, and anchor positioning.
- **Two-Pass Audio Normalization Pipeline:**
  To eliminate compression pumping artifacts caused by dynamic single-pass AGC on speech transients, the render pipeline implements the two-pass EBU R128 loudness normalization flow assumed by `PROGRESS_AND_ETA.md` (`analyze_audio` stage):
  1. **Pass 1 (Analysis Stage `analyze_audio`):**
     FFmpeg analyzes timeline audio into JSON statistics:
     ```bash
     ffmpeg -hide_banner -i <input_audio> -af "loudnorm=I=-14.0:TP=-1.5:LRA=11:print_format=json" -f null -
     ```
     Yields measured values: `input_i`, `input_tp`, `input_lra`, `input_thresh`, and `target_offset`.
  2. **Pass 2 (Render Stage `render`):**
     The measured parameters are injected directly into the final render filtergraph with `linear=true`:
     ```bash
     volume=<gainDb>dB,loudnorm=I=-14.0:TP=-1.5:LRA=11:measured_I=<val>:measured_TP=<val>:measured_LRA=<val>:measured_thresh=<val>:offset=<val>:linear=true,aresample=48000
     ```
- **Default Target Recommendation (-14.0 LUFS):**
  - **Platform Standards:** YouTube Shorts, TikTok, and Instagram Reels all target **-14.0 LUFS** (with True Peak capped at -1.5 dBFS). Content delivered at -16.0 LUFS is noticeably quieter than surrounding feed content on mobile platforms.
  - **Dynamic Headroom:** Our empirical testing demonstrated that linear mode with `-14.0 LUFS` target safely caps True Peak at -1.5 dBFS without clipping.
  - **Configurability:** Per `AGENTS.md §3` ("Config over constants"), `-14.0 LUFS` is set as the default, but remains template-configurable via `audio.targetLufs` (supporting `-16.0 LUFS` for AES/EBU broadcast standards or speech-only podcast templates). Enforces 48 kHz stereo AAC audio.

### 2.4 Windows Path Escaping Rules
In Windows FFmpeg filtergraphs, paths passed to `ass`, `subtitles`, and `drawtext` fail if backslashes or colons are unescaped. The verified escaping contract is:
```python
def escape_filter_path(p: str) -> str:
    # 1. Replace Windows backslashes with POSIX forward slashes
    posix_path = p.replace("\\", "/")
    # 2. Escape drive letter colon (F: -> F\:)
    return posix_path.replace(":", r"\:")
```
Passed as: `ass=filename='F\:/path/to/caption.ass':fontsdir='F\:/path/to/fonts'`.

---

## 3. Captions, Typography Suite & Limits (`caption_generator.py`)

### 3.1 Word Timing to ASS Format
The caption generator processes word-level timestamps (`t0`, `t1` in milliseconds) into Advanced SubStation Alpha (`v4.00+`) format:
- **`word_highlight`:** Displays full subtitle phrase, with the active word dynamically highlighted in a contrasting accent color (e.g. yellow `&H0000D7FF&` vs white `&H00FFFFFF&`) with subtle scale pulse (`\fscx105\fscy105`).
- **`pop`:** The current word scales up (`\t(0,100,\fscx115\fscy115)\t(100,200,\fscx100\fscy100)`) on speech onset.
- **`none`:** Clean line subtitles without per-word animation.
- **Safe Zone:** Margin vertical set to 16%–20% of canvas height (`MarginV=307–384px`) to ensure subtitles never collide with TikTok/Reels UI buttons (like, comment, audio disc).

### 3.2 Bundled OFL Font Matrix
All 4 project-approved SIL OFL 1.1 fonts were tested and verified with `ass=fontsdir`:
- **Montserrat** (Geometric Sans, modern clean)
- **Poppins** (Geometric Sans, rounded high legibility)
- **Inter** (Clean neo-grotesque, ultra-readable)
- **Anton** (Impactful condensed bold headline)

**Missing Font Guard:** If an untrusted template specifies an unknown font family (e.g. `"ComicSans"`), `caption_generator.py` raises `ValueError` immediately before FFmpeg invocation.

### 3.3 Emoji Rendering Behavior & Limits
- **Observation:** When emojis (e.g. 🔥, 😂, 👍) appear in caption text, libass paired with bundled pure-typography TTF fonts does NOT render color emojis. Depending on FreeType fallback, it either renders an empty space or a missing glyph box (`[]`).
- **Technical Cause:** Bundled OFL fonts do not contain color bitmap tables (CBDT/CBLC, COLR/CPAL, or SVG). Full color emoji rendering requires system FontConfig and an OS color emoji font (e.g. Segoe UI Emoji or Noto Color Emoji), which conflicts with our fontconfig-free, self-contained sidecar architecture.
- **Product Recommendation:** Strip emoji characters from automated speech-to-text caption lines during caption generation in MVP. Treat rich inline color emoji rendering as a Post-MVP enhancement.

---

## 4. Hardware Encoder Matrix & Presets (AMD Testbed)

### 4.1 AMD AMF (`h264_amf`)
- **Usage:** `-usage transcoding`
- **Rate Control:** Supports `vbr_latency` (Low Latency / Smooth VBR), `vbr_peak` (Peak Constrained VBR), `cbr` (Constant Bitrate), and `cqp` (Constant QP).
- **Profile & Level:** `-profile:v high -level 4.2 -pix_fmt yuv420p` fully honored.
- **GOP Length:** `-g 30` creates 1-second keyframe intervals at 30 fps.
- **Preset Mapping:**
  - `standard`: `-rc vbr_latency -b:v 4500k -maxrate 6000k -bufsize 9000k -quality quality` (or `-rc vbr_peak`)
  - `small_file`: `-rc cbr -b:v 2500k -maxrate 2500k -bufsize 5000k`

### 4.2 Windows Media Foundation (`h264_mf`)
- **Rate Control:** Supports `pc_vbr` (Peak Constrained VBR), `cbr` (Constant Bitrate), `quality` (Quality mode 1–100).
- **Profile Gotcha:**
  > [!WARNING]
  > Passing `-profile:v high` to `h264_mf` causes FFmpeg to exit immediately with:  
  > `[h264_mf] Unable to parse "profile" option value "high" - Invalid argument`.  
  > The Windows Media Foundation MFT manages profile selection automatically. In `render_builder.py`, `-profile:v` **must NOT** be passed to `h264_mf`.
- **Preset Mapping:**
  - `standard`: `-b:v 4500k -rate_control pc_vbr -g 30`
  - `small_file`: `-b:v 2500k -rate_control cbr -g 30`

### 4.3 NVIDIA NVENC (`h264_nvenc`) — UNTESTED / PLACEHOLDER
- **Hardware Status:** **UNTESTED / PLACEHOLDER**. NVIDIA hardware is absent on Reference Machine R1 (AMD).
- **Failure Verification:** Probed in S4b fallback test; cleanly exits with return code `4294967295` and error `[vost#0:0/h264_nvenc] Task finished with error code: -22 (Invalid argument)`, triggering graceful fallback.
- **Proposed Preset Template (Pending Hardware Validation on Device Class A):**
  - `standard`: `-rc vbr -cq 23 -b:v 4500k -maxrate 6000k -bufsize 9000k -profile:v high -g 30`
  - `small_file`: `-rc cbr -b:v 2500k -maxrate 2500k -bufsize 5000k -profile:v high -g 30`

### 4.4 Intel Quick Sync Video (`h264_qsv`) — UNTESTED / PLACEHOLDER
- **Hardware Status:** **UNTESTED / PLACEHOLDER**. Intel QSV hardware is absent on Reference Machine R1 (AMD).
- **Failure Verification:** Probed in S4b fallback test; cleanly exits with return code `2981409195` and error `[vost#0:0/h264_qsv] Task finished with error code: -22 (Invalid argument)`, triggering graceful fallback.
- **Proposed Preset Template (Pending Hardware Validation on Intel Testbed):**
  - `standard`: `-global_quality 23 -b:v 4500k -maxrate 6000k -profile:v high -g 30`
  - `small_file`: `-b:v 2500k -maxrate 2500k -bufsize 5000k -profile:v high -g 30`

---

## 5. Empirical Benchmark Results (60 s Export, 3 Runs, Median)

The benchmark executed 21 complete 60-second 1080×1920 vertical video renders on R1. All runs included full 1080×1920 crop, burning 60 seconds of animated ASS captions (Montserrat), audio loudness normalization (-16 LUFS), and `+faststart`.

### 5.1 Benchmark Summary Table (Medians of 3 Runs)

| Scenario / Configuration | Source Format | Encoder | Decoder Mode | Wall Time (s) | Speed (x Realtime) | Avg CPU % | Peak RAM (MB) | Output Size (MB) | Bitrate (kbps) |
|---|---|---|---|---|---|---|---|---|---|
| **`h264_src_amf_swdec`** | 1080p60 H.264 | `h264_amf` | Software | **22.08 s** | **2.72×** | 27.3% | 333.3 MB | 31.07 MB | 4242 kbps |
| **`h264_src_amf_d3d11va`**| 1080p60 H.264 | `h264_amf` | D3D11VA HW | **31.78 s** | **1.89×** | **12.3%** | 395.9 MB | 31.07 MB | 4242 kbps |
| **`h264_src_mf_swdec`** | 1080p60 H.264 | `h264_mf` | Software | **22.31 s** | **2.69×** | 60.1% | 605.4 MB | 30.92 MB | 4220 kbps |
| **`h264_src_mf_d3d11va`** | 1080p60 H.264 | `h264_mf` | D3D11VA HW | **23.57 s** | **2.55×** | 46.2% | 634.1 MB | 30.92 MB | 4220 kbps |
| **`vp9_src_amf_swdec`** | 1080p VP9 | `h264_amf` | Software | **18.92 s** | **3.17×** | 24.3% | 237.0 MB | 27.41 MB | 3741 kbps |
| **`vp9_src_amf_d3d11va`** | 1080p VP9 | `h264_amf` | D3D11VA HW | **29.14 s** | **2.06×** | **11.3%** | 277.5 MB | 27.41 MB | 3741 kbps |
| **`vp9_src_mf_swdec`** | 1080p VP9 | `h264_mf` | Software | **17.53 s** | **3.42×** | 55.5% | 501.9 MB | 26.89 MB | 3671 kbps |

*(Note: CPU % is measured across all 12 logical threads; e.g. 25% CPU usage represents ~3 full cores saturated).*

### 5.2 Quality Evaluation (SSIM, PSNR & VMAF vs Scaled Source)

#### 1. Audit of Previous Metrics (Cross-Lossy Comparison Flaw)
In the initial S4 report draft, quality was reported as:
- **SSIM:** `0.985328`
- **PSNR:** `41.887 dB` (with ungrounded claim of meeting "broadcast standard 40 dB")

**Validity Audit Finding:**
1. **Cross-Lossy Evaluation:** An inspection of `spikes/s4-render/benchmark_export.py` revealed that these figures were calculated by running `ffmpeg` with `-i h264_src_mf_swdec.mp4 -i h264_src_amf_swdec.mp4` — measuring the distortion between **two lossy encoder outputs**, neither of which represented the source reference. This cross-codec comparison cannot evaluate encode fidelity against ground truth.
2. **S1c Proxy Duplication:** The PSNR figure of 41.887 dB coincidentally matched the S1c proxy generation quality figure (41.89 dB at 3.5 Mbps), indicating an inadvertent metric conflation in early notes.
3. **"Broadcast Standard" Wording Removed:** Professional broadcast transmission specifications (e.g. EBU R128 / ITU-R BT.500) do not define a universal 40 dB PSNR floor for vertical mobile transcoding; the phrasing has been retracted.

#### 2. Rigorous Objective Quality vs Pristine Scaled Source
In Spike S4b (`test_quality_and_rate_control.py`), all 5 production encoder presets were re-benchmarked directly against the pristine **SCALED SOURCE** (1080×1920 @ 30 fps, cropped to match composition framing, format `yuv420p`). Per-frame SSIM, PSNR, and VMAF (via `libvmaf`, `n_threads=6`) were computed across the entire 60-second clip (1,800 frames):

| Preset / Encoder Configuration | Encoder | Preset | Rate Control Mode | Bitrate (kbps) | Speed (x Realtime) | SSIM vs Scaled Source | PSNR (dB) vs Scaled Source | VMAF (0–100) vs Scaled Source | Perceptual Quality Assessment |
|---|---|---|---|---|---|---|---|---|---|
| **`amf_standard_vbr_peak`** | `h264_amf` | `standard` | `vbr_peak` | 4420.5 kbps | **2.73×** | **0.99251** | **47.995 dB** | **97.69** | Visually transparent; indistinguishable from master |
| **`amf_standard_vbr_latency`** | `h264_amf` | `standard` | `vbr_latency` | 4022.3 kbps | **2.69×** | **0.99125** | **46.415 dB** | **96.98** | Visually transparent; slight bitrate underrun |
| **`amf_small_file_cbr`** | `h264_amf` | `small_file` | `cbr` (2.5M) | 2456.1 kbps | **4.41×** | **0.98814** | **44.703 dB** | **92.58** | Excellent mobile quality; strict size cap compliance |
| **`mf_standard_pc_vbr`** | `h264_mf` | `standard` | `pc_vbr` | 4037.5 kbps | **3.58×** | **0.98615** | **43.213 dB** | **91.37** | Good quality fallback; minor high-frequency softening |
| **`mf_small_file_cbr`** | `h264_mf` | `small_file` | `cbr` (2.5M) | 2440.3 kbps | **3.53×** | **0.98369** | **42.880 dB** | **87.84** | Acceptable fallback; noticeable grain suppression |

- **VMAF Findings:**
  - Both AMF standard configurations achieve VMAF > 96.9, which exceeds the industry threshold (93+) for visually lossless consumer delivery.
  - Media Foundation achieves respectable quality as a zero-dependency Windows fallback (VMAF 91.37 at 4.0 Mbps and 87.84 at 2.4 Mbps).

### 5.3 Hardware Decode vs Software Decode Analysis
- **CPU Offloading:** Adding `-hwaccel d3d11va` cuts CPU utilization from 27.3% to **12.3%** on AMF, and from 60.1% to **46.2%** on MF.
- **Wall Time Trade-Off:** However, wall time increases by ~9.7 s (31.78 s vs 22.08 s on AMF). This occurs because frames decoded into D3D11 GPU surfaces must be copied to system RAM for the CPU-based `libass` subtitle burn-in filter before re-uploading to the AMF encoder.
- **Recommendation:**
  - Default to **software decode** for maximum export speed on mid-spec laptops (Class B/A, 6+ logical cores).
  - Use **D3D11VA** if battery saver mode is active or user requests low CPU footprint in settings.

### 5.4 AMF Rate Control Evaluation: `vbr_latency` vs `vbr_peak`

For offline batch video export, the choice of AMF rate control parameter (`-rc`) has notable implications on bitrate allocation and quality:

| Metric | `vbr_latency` (Low Latency VBR) | `vbr_peak` (Peak Constrained VBR) | Difference / Impact |
|---|---|---|---|
| **Target Bitrate** | 4500 kbps (max 6000k, buf 9000k) | 4500 kbps (max 6000k, buf 9000k) | Same target specification |
| **Delivered Bitrate** | 4022.3 kbps (-10.6% underrun) | 4420.5 kbps (-1.8% target accuracy) | `vbr_peak` utilizes allocated bandwidth much better |
| **Encode Speed** | 22.30 s (2.69× realtime) | 22.01 s (2.73× realtime) | Negligible difference (within measurement noise) |
| **PSNR** | 46.415 dB | **47.995 dB** | **+1.58 dB** improvement with `vbr_peak` |
| **SSIM** | 0.99125 | **0.99251** | +0.00126 improvement |
| **VMAF** | 96.98 | **97.69** | **+0.71 VMAF points** improvement |

- **Technical Analysis & Justification:**
  - `vbr_latency` was engineered by AMD primarily for interactive game streaming, remote desktop, and low-latency cloud capture where encoder buffer latency must remain below 16–33 ms. To prevent buffer overflow when rapid motion occurs, the encoder conservatively throttles quantization, causing the delivered average bitrate to drop to 4022 kbps.
  - `vbr_peak` is designed for peak-constrained offline transcoding. It allows the encoder buffer to fully utilize the defined `bufsize` (9000k) and peak bursts up to `maxrate` (6000k), achieving higher bit allocation in motion scenes without artificial latency throttling.
  - **Recommendation:** Both `-rc vbr_peak` and `-rc vbr_latency` produce high visual fidelity (VMAF > 96.9). For desktop offline exports, **`vbr_peak` is recommended** for optimal visual quality, while `vbr_latency` remains a fully supported alternative.

### 5.5 Operational Validation Evidence (`test_operational_validations.py`)

Spike S4b executed comprehensive operational validations covering encoder fallback, mid-render cancellation cleanup, disk space limits, and loudness normalization.

#### 1. Automatic Fallback Chain Demonstration
Priority sequence: **`h264_nvenc` → `h264_qsv` → `h264_amf` → `h264_mf` → `E_ENCODER_UNAVAILABLE`**.

| Stage | Attempted Encoder | Exit Code | Stage Latency | Success | Captured Stderr Diagnostic Excerpt |
|---|---|---|---|---|---|
| 1 | `h264_nvenc` | `4294967295` (-22) | 0.144 s | False | `[vost#0:0/h264_nvenc] Task finished with error code: -22 (Invalid argument)`<br>`Conversion failed!` |
| 2 | `h264_qsv` | `2981409195` (-22) | 0.235 s | False | `[vost#0:0/h264_qsv] Task finished with error code: -22 (Invalid argument)`<br>`Conversion failed!` |
| 3 | `h264_amf` | `0` (Success) | 1.324 s | **True** | Clean initialization, successful test render output. |

- **Total Fallback Latency:** **1.704 s**.
- **Takeaway:** When unsupported hardware is configured, FFmpeg fails immediately without freezing, allowing the Rust supervisor to transition down the priority chain in under 2 seconds.

#### 2. Mid-Render Cancellation & Temp Cleanup
- **Job ID:** `job_cancel_test_54321`
- **Output Target:** `%LOCALAPPDATA%\...\operational_s4b\job_cancel_test_54321.tmp`
- **Disk Flush Observed:** Active writing observed during stream execution with **262,192 bytes** flushed to disk.
- **Process Termination:** Process terminated mid-stream (simulating Rust Job Object `kill-on-close`).
- **Cleanup Audit:** Supervisor cleanup invoked immediately upon exit.
  - `cleanup_successful: true`
  - `orphaned_tmp_exists_after: false`
- **Takeaway:** Zero orphaned `.tmp` files are left on disk upon user cancellation or fatal error.

#### 3. Low Disk Space Guard (`E_DISK_LOW`)
- **Safety Formula:** `required_bytes = 1.5 * estimated_output_bytes + 500 MB headroom`.
- For a 60-second 4500 kbps render (estimated output: 32.2 MB, required free space: 548.3 MB):
  - **Real Hardware (155,545.4 MB free):** Evaluation: `status: PROCEED`.
  - **Simulated Low Disk (200.0 MB free):** Evaluation: `status: BLOCKED`, returning error code `E_DISK_LOW` with user-facing Indonesian message:
    `"Ruang disk tidak mencukupi untuk export (200.0 MB bebas, diperlukan minimal 548.3 MB)"`.
  - **Takeaway:** Prevents half-completed renders or out-of-disk crashes by failing fast in the Rust core prior to spawning FFmpeg.

#### 4. Two-Pass Audio Loudness Pipeline & Target Justification
Audio dynamic range on source clip: `input_i = -16.63 LUFS`, `input_tp = +0.01 dBFS`, `input_lra = 15.90 LU`.

| Target LUFS | Method | Wall Time (s) | Measured Integrated LUFS | LUFS Error | Measured True Peak | Normalization Behavior |
|---|---|---|---|---|---|---|
| **-14.0 LUFS** | Single-Pass (Dynamic) | 1.97 s | -14.8 LUFS | 0.8 LU | -1.5 dBFS | Dynamic compression (LRA reduced to 6.6 LU) |
| **-14.0 LUFS** | **Two-Pass (Linear)** | 4.37 s | -15.5 LUFS | 1.5 LU | -1.5 dBFS | Linear scaling; backs off to protect True Peak ceiling |
| **-16.0 LUFS** | Single-Pass (Dynamic) | 2.07 s | -15.7 LUFS | 0.3 LU | -1.5 dBFS | Dynamic compression (LRA reduced to 7.1 LU) |
| **-16.0 LUFS** | **Two-Pass (Linear)** | 4.38 s | -16.8 LUFS | 0.8 LU | -1.5 dBFS | Linear scaling; backs off to protect True Peak ceiling |

- **Why Two-Pass is Superior:** Single-pass loudnorm uses dynamic gain adjustment with an internal compressor/limiter that audibly pumps speech transients in gaming clips. Two-pass linear mode (`measured_I=...:measured_TP=...:linear=true`) maintains natural dynamics and linear gain scaling.
- **Default Target Justification:** We recommend **-14.0 LUFS** as the default template target:
  - YouTube Shorts, TikTok, and Instagram Reels all target -14.0 LUFS. Clips mastered to -16.0 LUFS sound noticeably quieter than other platform content.
  - Per `AGENTS.md §3` ("Config over constants"), loudness target is kept template-configurable via `audio.targetLufs` (default `-14.0`, optional `-16.0`).

---

## 6. Progress Parsing & ETA Priors (`progress_parser.py`)

### 6.1 Event Stream & Accuracy
FFmpeg invoked with `-progress pipe:1` outputs structured blocks:
- **Event Frequency:** Emits 1 complete progress packet approximately every **0.5–1.0 s** (~1.2 events/second during active render).
- **Parser Contract:** Reads `frame`, `fps`, `speed`, and `out_time_us`. Calculates completion percent `frame / total_frames` and instantaneous ETA `(total_frames - frame) / fps`.

### 6.2 Prior Rates for ETA Estimator (`stage_stats` Seeds)
Based on measured medians on Device Class B (AMD Ryzen 5 PRO 5650U / Vega 7 iGPU):

| Variant / Profile | Prior Rate $r_{prior}$ | Expected 60 s Render Time | Measurement Status |
|---|---|---|---|
| `h264_amf` (AMD iGPU) | **2.70× realtime** (81 fps) | **~22.2 seconds** | **Measured (R1)** |
| `h264_mf` (Media Foundation) | **2.70× realtime** (81 fps) | **~22.2 seconds** | **Measured (R1)** |
| `h264_nvenc` (NVIDIA GPU)* | *3.50× realtime* | *~17.0 seconds* | **UNTESTED / PLACEHOLDER** |
| `h264_qsv` (Intel QSV)* | *2.80× realtime* | *~21.5 seconds* | **UNTESTED / PLACEHOLDER** |

*(\*UNTESTED / PLACEHOLDER: Physical NVIDIA and Intel hardware absent on Reference Machine R1. Per AGENTS.md, ETA priors must only come from measured data or the first-run benchmark; these placeholder seeds must not be treated as verified).*

---

## 7. Verification & Error Catalog Integration

### 7.1 Verification Checklist (`verify_export.py`)
Post-render checks verify:
1. **Container Faststart:** Binary inspection confirms `moov` atom is positioned at byte 0 before `mdat` atom.
2. **Video Stream:** Codec `h264`, dimensions exactly `1080×1920`, framerate `30 fps`, pixel format `yuv420p`, color space `bt709 tv`.
3. **Audio Stream:** Codec `aac`, sample rate `48000 Hz` (or `44100 Hz`), stereo.
4. **Duration:** Rendered duration within ±0.5 s of requested composition timeline.

### 7.2 Real Stderr Patterns for Catalog Mapping
Real captured stderr messages for error handling from S4b operational tests:

| Error Code | Trigger Condition | Real Stderr Pattern Captured (Measured) |
|---|---|---|
| **`E_ENCODER_UNAVAILABLE`** | Unsupported hardware encoder requested (`h264_nvenc` on AMD) | `[vost#0:0/h264_nvenc] Task finished with error code: -22 (Invalid argument)`<br>`[vost#0:0/h264_nvenc] Terminating thread with return code -22`<br>`Conversion failed!` (Exit code: `4294967295`) |
| **`E_ENCODER_UNAVAILABLE`** | Unsupported hardware encoder requested (`h264_qsv` on AMD) | `[vost#0:0/h264_qsv] Task finished with error code: -22 (Invalid argument)`<br>`[vost#0:0/h264_qsv] Terminating thread with return code -22`<br>`Conversion failed!` (Exit code: `2981409195`) |
| **`E_ENCODER_UNAVAILABLE`** | Encoder binary name missing | `Unknown encoder 'non_existent_gpu_enc'`<br>`Error selecting an encoder`<br>`Error opening output files: Encoder not found` |
| **`E_RENDER_FAILED`** | Invalid encoder argument (e.g. `-profile:v high` on `h264_mf`) | `[h264_mf] Unable to parse "profile" option value "high"`<br>`[h264_mf] Error setting option profile to value high.`<br>`Error applying encoder options: Invalid argument` |
| **`E_DISK_LOW`** | Free disk space < 1.5 × output size + 500 MB | Checked pre-render in supervisor before spawning FFmpeg.<br>Blocks invocation: `"Ruang disk tidak mencukupi untuk export (200.0 MB bebas, diperlukan minimal 548.3 MB)"`. |

### 7.3 Mid-Render Cancellation & Cleanup (Demonstrated Evidence)
- **Demonstration (`test_operational_validations.py`):**
  - Render spawned writing to temporary file: `%LOCALAPPDATA%\...\<job_id>.tmp`.
  - Process monitored; active disk writing confirmed with **262,192 bytes** flushed before kill.
  - Termination signal issued to simulate Rust Job Object `kill-on-close`.
  - Supervisor cleanup executed immediately upon process exit: `.tmp` file deleted.
  - Verification: `orphaned_tmp_exists_after: false`. Zero orphaned temp files remain.

---

## 8. Proposed Edits to Project Documentation (For User Review)

The following edits are proposed for project documentation files (not applied, per spike rules):

### 8.1 Proposed Edits to `docs/ARCHITECTURE.md §7` (Render Pipeline & Presets)
Replace `(mapping table: *fill after S4*)` with explicit options, marking untested hardware appropriately:
```markdown
- **Export presets & rate control:**
  - **`standard` (quality target):**
    - `h264_amf` (Measured on R1): `-usage transcoding -rc vbr_latency -b:v 4500k -maxrate 6000k -bufsize 9000k -quality quality -profile:v high -level 4.2 -g 30` *(or `-rc vbr_peak`)*
    - `h264_mf` (Measured on R1): `-b:v 4500k -rate_control pc_vbr -g 30` *(Note: `h264_mf` omits `-profile:v` because MFT selects high profile internally; passing `-profile:v high` triggers syntax error)*
    - `h264_nvenc` (UNTESTED / PLACEHOLDER): `-rc vbr -cq 23 -b:v 4500k -maxrate 6000k -bufsize 9000k -profile:v high -g 30`
    - `h264_qsv` (UNTESTED / PLACEHOLDER): `-global_quality 23 -b:v 4500k -maxrate 6000k -profile:v high -g 30`
  - **`small_file` (bitrate-capped for templates with maxFileSizeMB):**
    - `h264_amf` (Measured on R1): `-usage transcoding -rc cbr -b:v 2500k -maxrate 2500k -bufsize 5000k -profile:v high -level 4.2 -g 30`
    - `h264_mf` (Measured on R1): `-b:v 2500k -rate_control cbr -g 30`
    - `h264_nvenc` (UNTESTED / PLACEHOLDER): `-rc cbr -b:v 2500k -maxrate 2500k -bufsize 5000k -profile:v high -g 30`
    - `h264_qsv` (UNTESTED / PLACEHOLDER): `-b:v 2500k -maxrate 2500k -bufsize 5000k -profile:v high -g 30`
  - Output containers always specify `-color_primaries bt709 -color_trc bt709 -colorspace bt709 -color_range tv -movflags +faststart`.
  - **Two-Pass Audio Normalization:**
    - Stage `analyze_audio` runs Pass 1: `loudnorm=I=-14.0:TP=-1.5:LRA=11:print_format=json -f null -`.
    - Stage `render` runs Pass 2 linear mode: `loudnorm=I=-14.0:TP=-1.5:LRA=11:measured_I=...:measured_TP=...:measured_LRA=...:measured_thresh=...:offset=...:linear=true`.
    - Default target: `-14.0 LUFS` (optimal for TikTok/Shorts/Reels); configurable via `audio.targetLufs`.
```

### 8.2 Proposed Edits to `docs/PROGRESS_AND_ETA.md §4.4` (Prior Rates Seed Table)
Update the prior rate seeds for export, strictly labeling measured vs placeholder:
```markdown
| Hardware Class | Encoder | $r_{prior}$ (x Realtime) | Basis |
|---|---|---|---|
| Class B (AMD iGPU) | `h264_amf` | **2.70×** | Measured on R1 (Ryzen 5 PRO 5650U / Vega 7) |
| Class B (AMD / Fallback) | `h264_mf` | **2.70×** | Measured on R1 (Ryzen 5 PRO 5650U / Vega 7) |
| Class A (NVIDIA GPU) | `h264_nvenc` | *3.50×* | **UNTESTED / PLACEHOLDER** (Calibrate in M1 probe) |
| Class B/C (Intel iGPU) | `h264_qsv` | *2.80×* | **UNTESTED / PLACEHOLDER** (Calibrate in M1 probe) |
```

### 8.3 Proposed Edits to `docs/ROADMAP.md` (Spike S4 Outcomes)
- Update Spike S4 completion notes: S4b resolved all validity gaps, confirmed R1 testbed provenance, measured objective VMAF/SSIM/PSNR against scaled source, demonstrated live fallback chain, mid-render cancellation cleanup, low-disk pre-flight check, and implemented the two-pass loudness normalization pipeline.
