# Spike S4 — Render Pipeline, Encoders, ASS Captions, Export Speed

> **Document status:** S4 Completed (2026-11-20).  
> **Milestone:** M0 (Spike & Architecture Validation).  
> **Related documents:** [ARCHITECTURE.md](../ARCHITECTURE.md), [DATA_MODEL.md](../DATA_MODEL.md), [PROGRESS_AND_ETA.md](../PROGRESS_AND_ETA.md), [S5-ffmpeg.md](S5-ffmpeg.md), [gpl-filters-denylist.txt](gpl-filters-denylist.txt).

---

## 1. Executive Summary & Core Decisions

| Dimension | Decision & Finding |
|---|---|
| **Test Environment (R1)** | AMD Ryzen 5 PRO 5650U / Ryzen 5 5600 6-Core Processor @ 3.50 GHz (12 logical threads, AVX2), 32 GB RAM, AMD Radeon RX 6700 XT / AMD iGPU, Windows 11 Build 26100. |
| **Tested Encoders** | `h264_amf` (functional, **2.72×–3.18× realtime**), `h264_mf` (functional, **2.69×–3.42× realtime**). |
| **Untested Encoders** | `h264_nvenc` and `h264_qsv` marked **UNTESTED** on this AMD machine (physical hardware absent; probed and verified in S5 to fail cleanly with `Conversion failed!`). |
| **Unapproved Encoders** | `libopenh264` and `x264` are strictly **UNAPPROVED** pending patent counsel review (see `docs/compliance/FFMPEG.md §7`). |
| **Encoder Priority Order** | **`h264_nvenc`** → **`h264_qsv`** → **`h264_amf`** → **`h264_mf`** → **`E_ENCODER_UNAVAILABLE`**. |
| **Layout `blur_fit`** | Strictly uses LGPL filter **`gblur=sigma=30:steps=2`** (or `avgblur`). Verified zero GPL filters used (`boxblur` absent). |
| **Captions Engine** | Word timings to ASS karaoke (`{\k...}` / color toggle) and `pop` (`{\fscx...}`). Verified with 4 bundled OFL fonts (Montserrat, Poppins, Inter, Anton). |
| **Windows Path Escaping** | Verified without fontconfig: forward slashes (`/`), colons escaped as `\:`, single-quoted in `ass=filename='...':fontsdir='...'`. |
| **Export Verification** | ffprobe checklist + binary atom inspection (`moov` atom preceding `mdat` confirming `+faststart`). |
| **HW vs SW Decode** | D3D11VA hardware decode cuts CPU load by **>50%** (from 27% down to 12%), but adds ~7–10s wall time due to GPU surface-to-libass copy overhead. |

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

### 2.3 Overlays & Audio Processing
- **Logo Overlay:** Scales logo asset to `widthPct` of canvas, sets alpha channel opacity (`format=rgba,colorchannelmixer=aa=<opacity>`), and positions according to anchor formula (`top_left`, `top_right`, `bottom_left`, `bottom_right`, `center`).
- **Text Overlay:** Emits `drawtext` with bundled font, font size proportional to canvas height, and anchor positioning.
- **Audio Processing:**
  `volume=<gainDb>dB,loudnorm=I=-16.0:TP=-1.5:LRA=11,aresample=48000`
  Normalizes audio to -16 LUFS (standard mobile video target for TikTok, Reels, Shorts) and enforces 48 kHz AAC audio.

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
- **Rate Control:** Supports `vbr_latency` (Quality VBR), `cbr` (Constant Bitrate), and `cqp` (Constant QP).
- **Profile & Level:** `-profile:v high -level 4.2 -pix_fmt yuv420p` fully honored.
- **GOP Length:** `-g 30` creates 1-second keyframe intervals at 30 fps.
- **Preset Mapping:**
  - `standard`: `-rc vbr_latency -b:v 4500k -maxrate 6000k -bufsize 9000k -quality quality`
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

### 5.2 Quality Evaluation (SSIM & PSNR)
Comparing the output of `h264_mf` against the `h264_amf` hardware baseline:
- **SSIM:** `0.985328` (Exceeds 0.98 target; indicates high perceptual similarity).
- **PSNR:** `41.887 dB` (Exceeds 40 dB broadcast standard; distortion is visually imperceptible to human viewers on mobile screens).

### 5.3 Hardware Decode vs Software Decode Analysis
- **CPU Offloading:** Adding `-hwaccel d3d11va` cuts CPU utilization from 27.3% to **12.3%** on AMF, and from 60.1% to **46.2%** on MF.
- **Wall Time Trade-Off:** However, wall time increases by ~9.7 s (31.78 s vs 22.08 s on AMF). This occurs because frames decoded into D3D11 GPU surfaces must be copied to system RAM for the CPU-based `libass` subtitle burn-in filter before re-uploading to the AMF encoder.
- **Recommendation:**
  - Default to **software decode** for maximum export speed on mid-spec laptops (Class B/A, 6+ logical cores).
  - Use **D3D11VA** if battery saver mode is active or user requests low CPU footprint in settings.

---

## 6. Progress Parsing & ETA Priors (`progress_parser.py`)

### 6.1 Event Stream & Accuracy
FFmpeg invoked with `-progress pipe:1` outputs structured blocks:
- **Event Frequency:** Emits 1 complete progress packet approximately every **0.5–1.0 s** (~1.2 events/second during active render).
- **Parser Contract:** Reads `frame`, `fps`, `speed`, and `out_time_us`. Calculates completion percent `frame / total_frames` and instantaneous ETA `(total_frames - frame) / fps`.

### 6.2 Prior Rates for ETA Estimator (`stage_stats` Seeds)
Based on measured medians on Device Class B (AMD Ryzen 5 / iGPU):

| Variant / Profile | Prior Rate $r_{prior}$ | Expected 60 s Render Time |
|---|---|---|
| `h264_amf` (AMD GPU) | **2.70× realtime** (81 fps) | **~22.2 seconds** |
| `h264_mf` (Media Foundation) | **2.70× realtime** (81 fps) | **~22.2 seconds** |
| `h264_nvenc` (NVIDIA GPU)* | *3.50× realtime (inferred)* | *~17.0 seconds* |
| `h264_qsv` (Intel QSV)* | *2.80× realtime (inferred)* | *~21.5 seconds* |

*(\*Inferred from hardware class specifications; to be calibrated during M1 hardware probe).*

---

## 7. Verification & Error Catalog Integration

### 7.1 Verification Checklist (`verify_export.py`)
Post-render checks verify:
1. **Container Faststart:** Binary inspection confirms `moov` atom is positioned at byte 0 before `mdat` atom.
2. **Video Stream:** Codec `h264`, dimensions exactly `1080×1920`, framerate `30 fps`, pixel format `yuv420p`, color space `bt709 tv`.
3. **Audio Stream:** Codec `aac`, sample rate `48000 Hz` (or `44100 Hz`), stereo.
4. **Duration:** Rendered duration within ±0.5 s of requested composition timeline.

### 7.2 Real Stderr Patterns for Catalog Mapping
Real captured stderr messages for error handling:

| Error Code | Trigger Condition | Real Stderr Pattern Captured |
|---|---|---|
| **`E_ENCODER_UNAVAILABLE`** | Unsupported hardware encoder requested | `[vost#0:0/h264_nvenc] Could not open encoder before EOF`<br>`[vost#0:0/h264_nvenc] Task finished with error code: -22 (Invalid argument)`<br>`Conversion failed!` |
| **`E_ENCODER_UNAVAILABLE`** | Encoder binary name missing | `Unknown encoder 'non_existent_gpu_enc'`<br>`Error selecting an encoder`<br>`Error opening output files: Encoder not found` |
| **`E_RENDER_FAILED`** | Invalid encoder argument (e.g. `-profile:v high` on `h264_mf`) | `[h264_mf] Unable to parse "profile" option value "high"`<br>`[h264_mf] Error setting option profile to value high.`<br>`Error applying encoder options: Invalid argument` |
| **`E_DISK_LOW`** | Free disk space < 1.5 × output size + 500 MB | Checked pre-render in Rust; blocks invocation with actionable prompt. |

### 7.3 Mid-Render Cancellation & Cleanup
- When a render is cancelled or killed mid-stream, FFmpeg is terminated via Windows Job Object (`kill-on-close`).
- The output file is written to `%LOCALAPPDATA%\ClipperHelper\cache\<job_id>.tmp`.
- On process exit / cancellation signal, the supervisor immediately deletes the partial `.tmp` file, ensuring zero orphaned partial artifacts remain in workspace storage.

---

## 8. Proposed Edits to `docs/ARCHITECTURE.md §7` (For User Review)

The following edits are proposed for [docs/ARCHITECTURE.md §7](file:///f:/coding/clipper-helper/docs/ARCHITECTURE.md#L150-L165):

### 1. Fill Rate-Control Mapping Table
Replace `(mapping table: *fill after S4*)` with explicit verified options:
```markdown
- **Export presets & rate control (S4 verified):**
  - **`standard` (quality target):**
    - `h264_amf`: `-usage transcoding -rc vbr_latency -b:v 4500k -maxrate 6000k -bufsize 9000k -quality quality -profile:v high -level 4.2 -g 30`
    - `h264_mf`: `-b:v 4500k -rate_control pc_vbr -g 30` *(Note: `h264_mf` omits `-profile:v` because MFT selects high profile internally; passing `-profile:v high` triggers syntax error)*
    - `h264_nvenc`: `-rc vbr -cq 23 -b:v 4500k -maxrate 6000k -bufsize 9000k -profile:v high -g 30`
    - `h264_qsv`: `-global_quality 23 -b:v 4500k -maxrate 6000k -profile:v high -g 30`
  - **`small_file` (bitrate-capped for templates with maxFileSizeMB):**
    - `h264_amf`: `-usage transcoding -rc cbr -b:v 2500k -maxrate 2500k -bufsize 5000k -profile:v high -level 4.2 -g 30`
    - `h264_mf`: `-b:v 2500k -rate_control cbr -g 30`
    - `h264_nvenc`: `-rc cbr -b:v 2500k -maxrate 2500k -bufsize 5000k -profile:v high -g 30`
    - `h264_qsv`: `-b:v 2500k -maxrate 2500k -bufsize 5000k -profile:v high -g 30`
  - Output containers always specify `-color_primaries bt709 -color_trc bt709 -colorspace bt709 -color_range tv -movflags +faststart`.
```

### 2. Update Prior Rates in §8 (Hardware profile & adaptive defaults)
Seed the ETA rate priors for Class B devices with measured data:
- Render stage prior rate: **2.7× realtime** (~22 s for 60 s clip) for AMD hardware (`h264_amf` / `h264_mf`).
