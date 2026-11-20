# Third-Party Software Notices & Licenses

This document lists the third-party software components, open-source libraries, binary sidecars, models, and fonts distributed with or utilized by **Clipper Helper**.

---

## 1. Binary Inventory & Component Manifest Catalog

The following external binaries and models are bundled with the installer or distributed via the signed component manifest into `%LOCALAPPDATA%\ClipperHelper\bin\`:

| Component | Target Binary / Artifact | Version / Commit | Upstream Source URL | License | Package SHA-256 | Extracted Size | Update Channel |
|---|---|---|---|---|---|---|---|
| **FFmpeg (Shared LGPL)** | `ffmpeg.exe`, `ffprobe.exe`, + 7 DLLs | `n9.0.2-58-g8c7ef49002` | [FFmpeg-Builds Release](https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/tag/autobuild-2026-11-19-12-39) | **LGPLv3** | `1fdb837def412ad7cc019874b6310156f40179bd30df58d041a82413f800a331` (zip) | 183.05 MB | Signed Manifest (`get-component-manifest`) |
| **yt-dlp** | `yt-dlp.exe` | `2026.10.26` | [yt-dlp GitHub Release](https://github.com/yt-dlp/yt-dlp/releases/tag/2026.10.26) | **The Unlicense** | `f5389dd336913bb54593e9b110ba8f36c572ab6ee9957d5cb1ea82c237887a7d` | 18.77 MB | Signed Manifest (`get-component-manifest`) |
| **QuickJS-NG** | `qjs.exe` | `v0.18.0` | [QuickJS-NG Release](https://github.com/quickjs-ng/quickjs/releases/tag/v0.18.0) | **MIT** | `c8d2074e64f7281f62b217ffbe8f258a5be8933b93478985c49fe050fb3f9f36` | 1.87 MB | Signed Manifest (`get-component-manifest`) |
| **whisper.cpp** | `whisper.exe` | `v1.7.1` (pinned) | [whisper.cpp GitHub](https://github.com/ggerganov/whisper.cpp) | **MIT** | Pinned per architecture probe | ~8.5 MB | Signed Manifest (`get-component-manifest`) |
| **Whisper Model (Base)** | `ggml-base.bin` | OpenAI Whisper GGML | [ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp) | **MIT** | `d4e5...` (verified on fetch) | ~142 MB | On-demand fetch (verified hash) |
| **Whisper Model (Small)**| `ggml-small.bin` | OpenAI Whisper GGML | [ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp) | **MIT** | `5535...` (verified on fetch) | ~466 MB | On-demand fetch (verified hash) |
| **Montserrat Font** | `Montserrat[wght].ttf` | Google Fonts (Variable) | [google/fonts](https://github.com/google/fonts/tree/main/ofl/montserrat) | **SIL OFL 1.1** | `a9b2...` (bundled) | 744 KB | Installer asset bundle |

---

## 2. Release Distribution Directory Layout (`licenses/`)

Every official release installer and portable archive includes a `licenses/` directory structured as follows:

```
licenses/
├── LICENSE.clipper-helper.txt    # Clipper Helper End-User License Agreement
├── THIRD_PARTY_NOTICES.md        # This comprehensive notice document
├── ffmpeg/
│   ├── COPYING.LGPLv3            # GNU Lesser General Public License version 3 text
│   ├── COPYING.GPLv3             # GNU General Public License version 3 text
│   ├── BUILDCONF.txt             # Exact configure flags used during compilation
│   ├── SOURCE_OFFER.txt          # Written offer and Cloudflare R2 mirror download URL
│   └── external/                 # Licenses for external libraries linked in FFmpeg shared build
│       ├── libass.txt            # ISC License (libass subtitle renderer)
│       ├── freetype.txt          # FreeType Project License (FTL)
│       ├── fribidi.txt           # GNU Lesser General Public License v2.1+
│       ├── harfbuzz.txt          # MIT License (HarfBuzz text shaping engine)
│       ├── dav1d.txt             # BSD 2-Clause License (AV1 decoder)
│       ├── openh264.txt          # BSD 2-Clause License (Cisco OpenH264)
│       ├── opus.txt              # BSD 3-Clause License (Opus audio codec)
│       ├── vorbis.txt            # BSD 3-Clause License (Vorbis audio codec)
│       ├── vpx.txt               # BSD 3-Clause License (VP8/VP9 video codec)
│       ├── webp.txt              # BSD 3-Clause License (WebP image codec)
│       ├── aom.txt               # BSD 2-Clause License (AOMedia AV1 codec)
│       ├── xml2.txt              # MIT License (libxml2)
│       ├── zimg.txt              # WTFPL (z.lib image scaling)
│       ├── vmaf.txt              # BSD 2-Clause + Patent License (Netflix VMAF)
│       ├── amf.txt               # MIT License (AMD Advanced Media Framework headers)
│       └── ffnvcodec.txt         # MIT License (NVIDIA Video Codec SDK headers)
├── yt-dlp/
│   └── LICENSE                   # The Unlicense text
├── quickjs-ng/
│   └── LICENSE                   # MIT License text
├── whisper.cpp/
│   └── LICENSE                   # MIT License text
├── fonts/
│   ├── Montserrat-OFL.txt        # SIL Open Font License 1.1
│   ├── Poppins-OFL.txt           # SIL Open Font License 1.1
│   ├── Inter-OFL.txt             # SIL Open Font License 1.1
│   └── Anton-OFL.txt             # SIL Open Font License 1.1
└── dependencies/
    ├── rust-crates.txt           # Aggregated licenses for all compiled Rust dependencies
    └── npm-packages.txt          # Aggregated licenses for all frontend npm packages
```

---

## 3. Component Details & Licensing Terms

### 3.1 FFmpeg
- **Copyright:** (c) 2000-2026 the FFmpeg developers.
- **License:** GNU Lesser General Public License version 3 (LGPLv3).
- **Compliance Documentation:** See [docs/compliance/FFMPEG.md](docs/compliance/FFMPEG.md).
- **Source Code Availability:** Full source code is mirrored at `https://cdn.clipperhelper.com/sources/ffmpeg/ffmpeg-n9.0.2-58-g8c7ef49002-source.tar.xz`.
- **User Modification:** Users may replace the FFmpeg executables and shared DLLs in `%LOCALAPPDATA%\ClipperHelper\bin\ffmpeg\` per the instructions in `docs/compliance/FFMPEG.md`.

### 3.2 yt-dlp
- **Copyright:** yt-dlp contributors ([https://github.com/yt-dlp/yt-dlp](https://github.com/yt-dlp/yt-dlp)).
- **License:** The Unlicense.
- **Notice:** This is free and unencumbered software released into the public domain. Anyone is free to copy, modify, publish, use, compile, sell, or distribute this software.

### 3.3 QuickJS-NG
- **Copyright:** (c) 2017-2026 Fabrice Bellard, Charlie Gordon, and QuickJS-NG contributors.
- **License:** MIT License.
- **Notice:** Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files, to deal in the Software without restriction.

### 3.4 whisper.cpp
- **Copyright:** (c) 2023-2026 Georgi Gerganov.
- **License:** MIT License.
- **Model Weights Notice:** The Whisper model weights distributed by OpenAI are licensed under the MIT License ([OpenAI Whisper](https://github.com/openai/whisper)).

### 3.5 Typography & Caption Fonts
- **Fonts:** Montserrat, Poppins, Inter, Anton.
- **License:** SIL Open Font License 1.1 (OFL-1.1).
- **Restrictions:** Font software is not sold by itself; reserved font names are preserved.

### 3.6 Rust & Node.js Ecosystem Libraries
Clipper Helper incorporates open-source libraries from the Rust crate ecosystem (e.g., Tauri, Tokio, Serde, Specta, Tracing, Rusqlite) and npm ecosystem (e.g., React, TailwindCSS, Zustand, TanStack Query). All such dependencies are licensed under permissive open-source licenses (MIT, Apache-2.0, or BSD). Full automated manifests are generated during CI build using `cargo-about` and `license-checker`.

---

## 4. Open Legal Questions for Counsel

> [!IMPORTANT]
> **DISCLAIMER: NOT LEGAL ADVICE**  
> The following items represent technical observations and identified legal risk boundaries compiled by the engineering team. They are flagged for formal review and advice by qualified intellectual property counsel prior to commercial general availability (GA).

### 4.1 H.264 / AVC Patent Pool Royalties (Via LA / MPEG LA)
- **Technical Context:** Clipper Helper decodes H.264 video streams from YouTube and encodes H.264 video files during export.
- **Encoding Delegation:** For export encoding, Clipper Helper defaults to hardware encoders (`h264_mf`, `h264_amf`, `h264_nvenc`, `h264_qsv`). In the case of `h264_mf`, the encoding transform is performed by Microsoft's Windows Media Foundation or the GPU vendor's hardware driver (e.g. NVIDIA, AMD, Intel), where licensing royalties are paid by the OS/hardware OEM.
- **Decoding & Software Codec:** FFmpeg includes software decoders for H.264 (`avcodec`).
- **Counsel Questions:**
  1. Does distributing a pre-compiled LGPL FFmpeg binary containing an H.264 software decoder trigger patent royalty obligations under the Via LA (formerly MPEG LA) AVC Patent Portfolio License for desktop software distributed to end users?
  2. Does the annual volume threshold (e.g. fewer than 100,000 units distributed per year without royalty under standard AVC pool terms) apply to freemium desktop applications distributing free downloads?
  3. Does routing all encoding through Windows Media Foundation (`h264_mf`) and hardware MFTs fully shield Clipper Helper from patent infringement liability for video encoding?

### 4.2 LGPLv3 Section 6 & "Anti-Tivoization" Terms in Commercial Desktop Software
- **Technical Context:** Candidate build 2 is compiled with `--enable-version3`, triggering LGPLv3 terms. Section 6 of GPLv3 (incorporated into LGPLv3) requires distributors to provide "Installation Information" sufficient to allow modified versions of the library to run.
- **Implementation:** Clipper Helper spawns FFmpeg out-of-process via standard OS process calls, loads it from an unencrypted `%LOCALAPPDATA%` directory, and provides documentation allowing users to swap the binary.
- **Counsel Questions:**
  1. Does our out-of-process architecture and documented replacement mechanism fully satisfy LGPLv3 Section 6 requirements without requiring disclosure of any proprietary application source code?
  2. Does Code Signing (Authenticode) on the outer installer or application binary conflict with user modification rights if the external sidecar directory itself is not signature-locked?
  3. Does the Clipper Helper desktop application qualify as a "User Product" under LGPLv3 / GPLv3 Section 6 terms (which determines the legal scope of the Installation Information / anti-tivoization requirement)?

### 4.3 Cisco OpenH264 Patent License Scope
- **Technical Context:** BtbN's build enables `libopenh264`. Cisco provides binary modules with royalty coverage under MPEG LA / Via LA, but BtbN compiles from source.
- **Counsel Questions:**
  1. Does Cisco's patent indemnity extend to binaries compiled from OpenH264 source by third parties, or is it strictly limited to binary downloads hosted on Cisco's infrastructure?
  2. Should Clipper Helper continue to treat `libopenh264` as strictly unapproved and forbidden from the encoder fallback chain?

### 4.4 Native TLS Stack via Microsoft Schannel vs Third-Party Cryptographic Libraries
- **Technical Context:** The candidate FFmpeg build was deliberately compiled with `--enable-schannel`, utilizing Microsoft Windows' native Schannel/CryptoAPI for HTTPS requests rather than OpenSSL or GnuTLS.
- **Counsel Questions:**
  1. Does the use of Windows native Schannel avoid the historical license incompatibility and export control restrictions associated with bundled third-party cryptography libraries?
