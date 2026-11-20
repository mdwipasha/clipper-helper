# Spike S5 — Production FFmpeg Build & Licensing / Compliance

> **Document status:** S5 Completed (2026-11-20).  
> **Milestone:** M0 (Spike & Architecture Validation).  
> **Related documents:** [ARCHITECTURE.md](../ARCHITECTURE.md), [FFMPEG.md](../compliance/FFMPEG.md), [THIRD_PARTY_NOTICES.md](../../THIRD_PARTY_NOTICES.md), [S1-downloader.md](S1-downloader.md).

---

## 1. Executive Summary & Final Recommendation

| Dimension | Decision |
|---|---|
| **Recommended Build** | **BtbN Release Branch Shared LGPL:** `ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0.zip` |
| **Effective License** | **LGPLv3** (compiled with `--enable-version3`, strictly **NO** `--enable-gpl`, **NO** `--enable-nonfree`) |
| **Forbidden Libraries** | `libx264`, `libx265`, `libfdk_aac` verified **100% ABSENT** |
| **Archive Size (Download)** | **74.14 MB** (77,746,112 bytes) — saves **89.75 MB (55%) bandwidth** vs static build |
| **Extracted Size (Disk)** | **183.05 MB** (191,939,584 bytes) — saves **215.88 MB (54%) disk footprint** vs static build |
| **Executable Footprint** | `ffmpeg.exe` (541 KB) + `ffprobe.exe` (265 KB) + 7 shared DLLs (149.93 MB) |
| **Subtitle Burn-in** | `libass` + `libfreetype` + `libfribidi` + `libharfbuzz` (100% functional, fontconfig-free via `fontsdir`) |
| **Hardware Encoders** | `h264_mf` (Media Foundation wrapper, functional), `h264_amf` (functional), `h264_nvenc` & `h264_qsv` (present) |
| **Blur Fit Layout** | `boxblur` is **GPL-only** (absent); `gblur` (Gaussian) and `avgblur` (Average) are **LGPL** and functional |
| **CI Capability Gate** | Automated test harness `spikes/s5-ffmpeg/verify-capabilities.py` passed with **PASS** |

### Why Shared over Static?
In static builds (`ffmpeg-release-static`), both `ffmpeg.exe` (128.23 MB) and `ffprobe.exe` (128.04 MB) statically embed all codec and filter libraries, resulting in **398.93 MB** of extracted files and a **163.89 MB** zip download. In contrast, the shared build links both binaries to 7 shared DLLs (`avcodec-63.dll`, `avfilter-12.dll`, `avformat-63.dll`, `avutil-61.dll`, `swscale-10.dll`, `swresample-7.dll`, `avdevice-63.dll`). This cuts user download bandwidth by **55%** and local disk footprint by **54%**.

### Why Release Branch over Master Snapshot?
The release branch build (`n9.0.2-58-g8c7ef49002`) represents a stable point-release branch (FFmpeg 9.0 branch), guaranteeing ABI stability, reproducible behavior, and isolation from daily upstream regressions found in `master` snapshots (`N-129130`).

---

## 2. Test Environment

- **Machine:** AMD Ryzen 5 5600 6-Core Processor @ 3.50 GHz (12 threads, AVX2 enabled)
- **RAM:** 32.0 GB DDR4 3200 MHz
- **GPU:** AMD Radeon RX 6700 XT (12 GB VRAM, Adrenalin 24.9.1 / Driver 32.0.12011.1036)
- **OS:** Windows 11 Pro 64-bit (Build 26100.2314, Version 24H2)
- **Network / ISP:** Biznet Fiber (FTTH), Jakarta, Indonesia (~150 Mbps symmetrical)
- **Test Date:** 2026-11-20

---

## 3. Candidate Builds Comparison

Three candidate builds from BtbN/FFmpeg-Builds (`autobuild-2026-11-19-12-39`) were downloaded and evaluated in `spikes/s5-ffmpeg/bin/`:

| Attribute | Candidate 1: Release Static | Candidate 2: Release Shared (**Selected**) | Candidate 3: Master Snapshot Static |
|---|---|---|---|
| **Archive File** | `ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-9.0.zip` | `ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0.zip` | `ffmpeg-N-129130-gcd39c36377-win64-lgpl.zip` |
| **Upstream URL** | [BtbN GitHub Release](https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/autobuild-2026-11-19-12-39/ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-9.0.zip) | [BtbN GitHub Release](https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/autobuild-2026-11-19-12-39/ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0.zip) | [BtbN GitHub Release](https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/autobuild-2026-11-19-12-39/ffmpeg-N-129130-gcd39c36377-win64-lgpl.zip) |
| **Version String** | `n9.0.2-58-g8c7ef49002-20261119` | `n9.0.2-58-g8c7ef49002-20261119` | `N-129130-gcd39c36377-20261119` |
| **Git Commit** | `8c7ef49002` | `8c7ef49002` | `cd39c36377` |
| **Archive Size (Zip)** | 163.89 MB (171,853,322 B) | **74.14 MB (77,746,112 B)** | 170.69 MB (178,978,481 B) |
| **Zip SHA-256** | `09f883bb8da62227d58efc2c2f7c859ba81e6e4312036d90a4f6d24e040d2487` | `1fdb837def412ad7cc019874b6310156f40179bd30df58d041a82413f800a331` | `e390e653c931a30a5051c0fe7f6859a2d3162171566c1e422c1f97b157de0da5` |
| **Extracted Size** | 398.93 MB | **183.05 MB** | 411.69 MB |
| **`ffmpeg.exe` Size** | 128.23 MB (134,458,880 B) | **0.53 MB (553,984 B)** | 132.30 MB (138,728,960 B) |
| **`ffmpeg.exe` SHA-256** | `6f9ef91c48f45ebff170feb15bbbf4e596d738ec45581de65b88294ca2d23361` | `1564f9afab0e0a41b1b3632424e9f55e9adc8cf2e521e22a3ebfd127dd6a7041` | `350d23aed259928d7874517547475d59146d6078ff3f85b0e93a9a66ca981891` |
| **`ffprobe.exe` Size** | 128.04 MB (134,258,176 B) | **0.26 MB (271,872 B)** | 132.10 MB (138,518,016 B) |
| **`ffprobe.exe` SHA-256** | `d7db62018c4a26fe22f6c21dfeddd6677d5e4ddec418041cf9b228e636ed31a6` | `abf8af8196ffaa12aa3392f6f809227b22b55a30ff87865a8e032da69016a199` | `163c6a42dfb48bf256222372b05ce0ee969c60399cc4c5777d2e9709fe1ef722` |
| **Shared DLLs Count** | 0 (static) | **7 DLLs** | 0 (static) |
| **Shared DLLs Size** | 0.0 MB | **149.93 MB (157,215,744 B)** | 0.0 MB |
| **Packaging Recommendation** | Discard (wasteful) | **Primary Production Choice** | Discard (unstable master) |

### Shared DLL Breakdown
The 7 shared DLLs extracted to `%LOCALAPPDATA%\ClipperHelper\bin\`:
1. `avcodec-63.dll` — 94,045,696 bytes (Video/audio encoders & decoders)
2. `avfilter-12.dll` — 32,116,224 bytes (Filtergraphs, ASS caption renderer, scaler)
3. `avformat-63.dll` — 19,978,240 bytes (Demuxers/muxers, MP4/MKV container handlers)
4. `avdevice-63.dll` — 4,950,528 bytes (Device I/O)
5. `avutil-61.dll` — 3,028,992 bytes (Core utilities, mathematical helpers, memory management)
6. `swscale-10.dll` — 2,346,496 bytes (Color space conversion, pixel format scaling)
7. `swresample-7.dll` — 749,568 bytes (Audio resampling, channel mixing)

*(Note: `ffplay.exe` (19.26 MB) is included in the upstream zip archive but must be stripped during component packaging since Clipper Helper does not use SDL2 playback).*

---

## 4. Build Configuration & License Audit

### 4.1 Verification of Build Flags
Executing `ffmpeg -buildconf` confirmed the following configuration parameters:
- **`--enable-gpl`**: **ABSENT** (Strictly excluded)
- **`--enable-nonfree`**: **ABSENT** (Strictly excluded)
- **`--enable-version3`**: **PRESENT** (Effective license: **LGPLv3**)
- **`--disable-static --enable-shared`**: **PRESENT** (Enables dynamic link libraries)
- **`--enable-schannel`**: **PRESENT** (Native Windows Security Support Provider / CryptoAPI TLS)
- **`--disable-libx264 --disable-libx265 --disable-libfdk-aac`**: **CONFIRMED**

### 4.2 External Libraries & Licenses Audit
Every external third-party library compiled into Candidate 2 was audited against LGPL compliance:

| Library | Status in Build | License Type | Compliance Assessment |
|---|---|---|---|
| **`libass`** | Enabled (`--enable-libass`) | ISC | Fully LGPL-compatible permissive license. Primary engine for subtitle rendering. |
| **`libfreetype`** | Enabled (`--enable-libfreetype`) | FTL / GPLv2 dual | Using FreeType Project License (FTL), which is BSD-like and fully LGPL-compatible. |
| **`libfribidi`** | Enabled (`--enable-libfribidi`) | LGPLv2.1+ | Fully compatible with LGPLv3. Handles BiDi text layout. |
| **`libharfbuzz`** | Enabled (`--enable-libharfbuzz`) | MIT | Permissive MIT license. Handles complex font shaping. |
| **`libdav1d`** | Enabled (`--enable-libdav1d`) | BSD-2-Clause | Permissive BSD license. Fast AV1 software video decoder. |
| **`libopenh264`**| Enabled (`--enable-libopenh264`) | BSD-2-Clause | Source license is BSD-2-Clause. (Note: Cisco binary distribution terms do not affect this build as it is statically compiled from source; however, hardware encoders take precedence). |
| **`libopus`** | Enabled (`--enable-libopus`) | BSD-3-Clause | Permissive BSD license. Opus audio codec. |
| **`libvorbis`** | Enabled (`--enable-libvorbis`) | BSD-3-Clause | Permissive BSD license. Vorbis audio codec. |
| **`libvpx`** | Enabled (`--enable-libvpx`) | BSD-3-Clause | Permissive BSD license. VP8/VP9 video decoders. |
| **`libwebp`** | Enabled (`--enable-libwebp`) | BSD-3-Clause | Permissive BSD license. WebP image decoding. |
| **`libaom`** | Enabled (`--enable-libaom`) | BSD-2-Clause | Permissive BSD license. AV1 reference decoder. |
| **`libxml2`** | Enabled (`--enable-libxml2`) | MIT | Permissive MIT license. Dash/XML parsing. |
| **`libzimg`** | Enabled (`--enable-libzimg`) | WTFPL | Extremely permissive. High quality image scaling. |
| **`libvmaf`** | Enabled (`--enable-libvmaf`) | BSD-2-Clause-Patent | Permissive BSD with patent grant. Video quality assessment. |
| **`schannel`** | Enabled (`--enable-schannel`) | Microsoft OS API | Windows native TLS library. Eliminates OpenSSL/GnuTLS GPL/LGPL boundary issues. |
| **`amf`** | Enabled (`--enable-amf`) | MIT | AMD Advanced Media Framework header-only integration. |
| **`ffnvcodec`** | Enabled (`--enable-ffnvcodec`) | MIT | NVIDIA Video Codec SDK header-only integration. |

---

## 5. Capability Audit & CI Verification

The automated capability gate script (`spikes/s5-ffmpeg/verify-capabilities.py`) was executed against Candidate 2. Full dump outputs are committed in:
- `docs/benchmarks/ffmpeg-encoders.txt` (Encoders catalog)
- `docs/benchmarks/ffmpeg-decoders.txt` (Decoders catalog)
- `docs/benchmarks/ffmpeg-filters.txt` (Filters catalog)

### 5.1 Encoders & Hardware Acceleration

| Encoder | Present in Binary | Functional on Test Machine | Error / Diagnostic Note |
|---|---|---|---|
| **`aac`** | Yes | **Yes (PASS)** | Native FFmpeg AAC audio encoder. Working as expected. |
| **`h264_mf`** | Yes | **Yes (PASS)** | Windows Media Foundation H.264 wrapper. Routes to hardware MFT / Microsoft software MFT. |
| **`h264_amf`** | Yes | **Yes (PASS)** | AMD AMF hardware encoder. Functional on test system (Radeon RX 6700 XT). |
| **`h264_nvenc`**| Yes | No | Fails cleanly (`Conversion failed!`) due to absence of NVIDIA GPU. Binary support confirmed. |
| **`h264_qsv`** | Yes | No | Fails cleanly (`Conversion failed!`) due to absence of Intel QSV GPU. Binary support confirmed. |

### 5.2 Decoders
- **Video:** `h264`, `hevc`, `vp9`, `av1` all **PRESENT** and verified.
- **Audio:** `aac`, `opus` both **PRESENT** and verified.

### 5.3 Filtergraph Capabilities & The `boxblur` Discovery
During the filter audit, all standard composition filters passed: `ass`, `subtitles`, `crop`, `scale`, `overlay`, `split`, `vstack`, `hstack`, `loudnorm`, `ssim`, `psnr`, `drawtext`, `libvmaf`.

> [!CAUTION]
> **Discovery: `boxblur` is GPL-only in FFmpeg!**  
> In the official FFmpeg source code, `libavfilter/vf_boxblur.c` is licensed strictly under **GPLv2+**. Consequently, any build compiled without `--enable-gpl` **strictly omits `boxblur`**.
>
> **Direct Architectural Impact:**  
> `ARCHITECTURE.md §7` mentions `blur_fit` using blurred scaled background. Using `boxblur` in an LGPL build will fail immediately with `No such filter: 'boxblur'`.
>
> **LGPL Replacement:**  
> The LGPL-compliant blur filters present in Candidate 2 are:
> 1. **`gblur`** (`libavfilter/vf_gblur.c`, LGPLv2.1+): Gaussian blur filter. Tested and verified functional.
> 2. **`avgblur`** (`libavfilter/vf_avgblur.c`, LGPLv2.1+): Average blur filter. Tested and verified functional.
>
> `ARCHITECTURE.md §7` must be updated to specify `gblur` (or `avgblur`) for `blur_fit` compositions.

### 5.4 Functional Render Test
A synthetic 1080×1920 vertical video crop and ASS subtitle burn-in render test was executed:
- **Input:** VP9 1080p source clip.
- **Command:** Crop center 1080×1920, burn-in ASS caption with local OFL font (`Montserrat`) via `ass=fontsdir`, encode with `h264_mf` at `-b:v 3500k -g 30 -pix_fmt yuv420p`, encode audio with `aac`.
- **Result:** **PASS**. Output file verified with `ffprobe` (1080×1920, H.264, AAC, 323,931 bytes, perfectly readable burned subtitles).
- **Fontconfig Isolation:** The test confirmed that `ass='<caption_path>':fontsdir='<fonts_dir>'` successfully resolves and embeds bundled fonts without requiring a system-wide fontconfig installation or Windows registry fonts.

---

## 6. Windows Media Foundation (`h264_mf`) Technical Analysis

`h264_mf` is the most versatile hardware/software bridge encoder available on Windows for LGPL distributions.

### 6.1 Architecture & Pipeline
`h264_mf` interacts directly with the Windows Media Foundation API (`mfplat.dll`, `mfreadwrite.dll`). It acts as a client to registered Media Foundation Transforms (MFTs):
1. **Hardware MFT Selection:** Windows queries hardware MFTs registered by GPU drivers (Intel Quick Sync MFT, NVIDIA NVENC MFT, AMD AMF MFT). If a supported GPU is present and configured, encoding occurs on dedicated silicon.
2. **Software MFT Fallback:** If hardware MFTs fail or are unavailable, Windows falls back to Microsoft's software H.264 encoder MFT (`mfh264enc.dll`).
3. **Hardware-Only Flag:** Passing `-hw_encoding 1` enforces hardware MFT usage. If hardware encoding is unsupported, the encoder exits with an error rather than falling back to CPU.

### 6.2 The Windows N / KN Edition Gotcha
- **The Issue:** Windows N and Windows KN editions (distributed primarily in Europe and Korea to comply with antitrust rulings) omit Windows Media Player and Media Foundation core binaries out-of-the-box (`mfplat.dll`, `mf.dll`, `mfuuid.dll`).
- **Failure Mode:** On Windows N/KN, `h264_mf` fails during initial API bind, throwing an exit code indicating DLL missing or `E_ENCODER_UNAVAILABLE`.
- **User Remediation:** Microsoft provides a free, official **Media Feature Pack** for Windows 10/11 N/KN (installable via *Windows Settings > System > Optional features*).
- **App Handling:** When `sidecars/ffmpeg` encounters an exit with `E_ENCODER_UNAVAILABLE` and the OS is identified as Windows N, the app must present Indonesian copy directing the user to install the Media Feature Pack.

### 6.3 Options Honored by `h264_mf`
Through parameter testing, the following CLI options were verified:
- `-b:v <bitrate>`: Fully honored (e.g. `3500k`).
- `-g <gop_size>`: Fully honored (e.g. `-g 30` creates keyframes every 1 second at 30 fps for smooth seeking).
- `-pix_fmt yuv420p`: Fully honored.
- `-rate_control`: Supports `cbr`, `pc_vbr`, `u_vbr`, `quality`.

---

## 7. In-House Minimal LGPL Build Evaluation

We evaluated whether Clipper Helper should compile its own minimal LGPL FFmpeg build in GitHub Actions CI rather than consuming BtbN's release builds.

| Evaluation Metric | BtbN Pinned Release Build (Selected) | In-House Minimal Build in CI |
|---|---|---|
| **Archive Size** | 74.14 MB | ~40–50 MB (estimated) |
| **Maintenance Burden** | **Zero.** Track pinned upstream releases via manifest. | **High.** Maintaining MinGW-w64 cross-compilers, toolchains, patching upstream vulnerabilities, MSYS2/Docker environments. |
| **CI Build Duration** | 0 minutes | ~45–60 minutes per build |
| **Reproducibility** | High (BtbN autobuild tags are permanent). | High (if pinned correctly). |
| **Vulnerability Updates**| Handled by upstream community. | Requires manual security triage and rebuilding. |

**Recommendation:**  
Adopt the **BtbN Pinned Release Build** for MVP (Milestones M0–M4). The 74 MB archive is well within Cloudflare R2's zero-egress cost model. A custom in-house build adds non-trivial infrastructure overhead with minimal tangible gain. Defer in-house builds to Post-MVP unless telemetry indicates extreme user bandwidth sensitivity.

---

## 8. Capability Gate Script (`verify-capabilities.py`)

The verification script created in `spikes/s5-ffmpeg/verify-capabilities.py` is established as the project's permanent capability test harness.

### CI Integration Contract:
- **Trigger:** Runs during every FFmpeg component update or component manifest generation.
- **Input:** Path to `ffmpeg.exe` and `ffprobe.exe`.
- **Exit Code:** `0` on `PASS`; `1` on `FAIL`.
- **Output:** Emits structured JSON (`verify_capabilities_results.json`) and prints a tabular summary.
- **Rules Enforced:**
  1. Fail if `--enable-gpl` or `--enable-nonfree` is present.
  2. Fail if `libx264`, `libx265`, or `libfdk_aac` is present.
  3. Fail if required filters (`ass`, `gblur`, `scale`, `crop`, `overlay`) are missing.
  4. Fail if `boxblur` is present (ensures GPL code was not inadvertently compiled).
  5. Fail if synthetic vertical caption burn-in render test fails.

---

## 9. How to Update FFmpeg (Runbook for Future Upgrades)

When updating FFmpeg to a newer point release or security fix, follow this procedure:

1. **Select Candidate Build:**
   - Source from official BtbN/FFmpeg-Builds releases.
   - **Strict Requirement:** Select the **Release-branch Shared LGPL** zip archive (e.g. `ffmpeg-n<version>-win64-lgpl-shared-<branch>.zip`). Do NOT select master snapshots or GPL builds.
2. **Run Capability Gate in CI (`verify-capabilities.py`):**
   - Execute the test harness:
     ```powershell
     python spikes/s5-ffmpeg/verify-capabilities.py --ffmpeg <path_to_ffmpeg.exe> --ffprobe <path_to_ffprobe.exe>
     ```
   - Must exit with `0` (`PASS`). Confirm buildconf excludes GPL/nonfree and contains no forbidden encoders.
   - Check that all filters used in composition are present and none match `docs/benchmarks/gpl-filters-denylist.txt`.
3. **Re-Run Hardware Encoder Smoke Tests:**
   - Execute smoke tests for `h264_mf`, `h264_amf`, `h264_nvenc`, `h264_qsv` on target test hardware.
   - Verify that fallback to `E_ENCODER_UNAVAILABLE` operates cleanly without unhandled crashes.
4. **Update Component Inventories & Documentation:**
   - Calculate SHA-256 checksums of the zip archive, executables, and all shared DLLs.
   - Update the Binary Inventory table in `THIRD_PARTY_NOTICES.md`.
   - Update version, commit, and hashes in `docs/ARCHITECTURE.md §15` and `docs/compliance/FFMPEG.md`.
5. **Publish to CDN & Mirror Source:**
   - Mirror the exact upstream source code tarball to Cloudflare R2 (`https://cdn.clipperhelper.com/sources/ffmpeg/...`).
   - Sign and publish the updated entry in `get-component-manifest`.
