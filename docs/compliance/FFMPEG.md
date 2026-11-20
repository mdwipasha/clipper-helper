# FFmpeg LGPLv3 Compliance Package & Distribution Guide

> **Document status:** Approved (Spike S5, 2026-11-20).  
> **Target version:** FFmpeg `n9.0.2-58-g8c7ef49002` (Release Branch, Shared LGPLv3).  
> **Related documents:** [S5-ffmpeg.md](../benchmarks/S5-ffmpeg.md), [ARCHITECTURE.md](../ARCHITECTURE.md), [THIRD_PARTY_NOTICES.md](../../THIRD_PARTY_NOTICES.md).

---

## 1. Compliance Statement & Legal Posture

Clipper Helper distributes FFmpeg as an independent, standalone executable package under the **GNU Lesser General Public License version 3 (LGPLv3)**.

To strictly preserve software freedom and satisfy all legal obligations under LGPLv3:
1. **No GPL Code:** FFmpeg is compiled strictly **without** `--enable-gpl` or `--enable-nonfree`. No GPL libraries (such as `libx264`, `libx265`, or `libfdk_aac`) are included.
2. **Effective License is LGPLv3:** The build includes `--enable-version3`, which applies the terms of LGPLv3 (and by reference GPLv3).
3. **Out-of-Process Execution Boundary:** Clipper Helper does **not** link against FFmpeg libraries (`libavcodec`, `libavformat`, etc.) at compile time, nor does it load them dynamically via FFI. FFmpeg is executed strictly as a separate operating system process via argument arrays inside a Windows Job Object.
4. **Source Code Availability:** The complete corresponding source code for the exact binary build distributed to users is mirrored and hosted permanently on Cloudflare R2.
5. **User Replacement Rights (Anti-Tivoization / LGPLv3 §6):** End-users are provided full technical ability and documentation to replace the bundled FFmpeg binaries and DLLs with their own custom versions without breaking the application.

---

## 2. Pinned Build Metadata

- **Upstream Release:** BtbN / FFmpeg-Builds (`autobuild-2026-11-19-12-39`)
- **Release Version:** `n9.0.2-58-g8c7ef49002-20261119`
- **Git Commit:** `8c7ef49002d99d3fc04130089e9f60cb6733ec58`
- **Source Git Repository:** [https://github.com/FFmpeg/FFmpeg](https://github.com/FFmpeg/FFmpeg) (branch: `release/9.0`)
- **Build Infrastructure Repo:** [https://github.com/FFmpeg-Builds/FFmpeg-Builds](https://github.com/FFmpeg-Builds/FFmpeg-Builds)
- **Archive Filename:** `ffmpeg-n9.0.2-58-g8c7ef49002-win64-lgpl-shared-9.0.zip`
- **Archive Size:** 77,746,112 bytes (74.14 MB)
- **Archive SHA-256:** `1fdb837def412ad7cc019874b6310156f40179bd30df58d041a82413f800a331`

### Binary Checksums (Extracted):
| Binary / Library | File Size (Bytes) | SHA-256 Checksum |
|---|---|---|
| `bin/ffmpeg.exe` | 553,984 | `1564f9afab0e0a41b1b3632424e9f55e9adc8cf2e521e22a3ebfd127dd6a7041` |
| `bin/ffprobe.exe` | 271,872 | `abf8af8196ffaa12aa3392f6f809227b22b55a30ff87865a8e032da69016a199` |
| `bin/avcodec-63.dll` | 94,045,696 | `3d685122ea03d6d376ca91c49984bfa4d80dffebcf3d4ae11263158ff8c3c137` |
| `bin/avfilter-12.dll` | 32,116,224 | `dfa7d18804683526c80c058fe6480c55f30800b59b58957fba3e77a16f21c172` |
| `bin/avformat-63.dll` | 19,978,240 | `e55b11910a309f4dd45d1f05477c44eaeb1dd6ca337651a70ffaa869ebf9bf60` |
| `bin/avdevice-63.dll` | 4,950,528 | `dcfb7db0a02ea03d42fb53dc564552467d5ce8fa4a39bfe87b404d1efc5bce51` |
| `bin/avutil-61.dll` | 3,028,992 | `d8be30da8a5fbcaee01211b6d1b26859432d56d11fefdb4ca7740f95fcb6eb63` |
| `bin/swscale-10.dll` | 2,346,496 | `c0d7607a7261c6b54784a957d5a57cf02fb62bfaf44c20fbfbcba54117e3bb63` |
| `bin/swresample-7.dll` | 749,568 | `295a004eb76f1837ce5b7bf6db1744ef55e69bf12db73b6aa75168e2195f00e9` |

---

## 3. Exact Upstream Build Configuration

Extracted directly from `ffmpeg.exe -buildconf`:

```
configuration:
  --prefix=/ffbuild/prefix
  --pkg-config-flags=--static
  --pkg-config=pkg-config
  --cross-prefix=x86_64-w64-mingw32-
  --arch=x86_64
  --target-os=mingw32
  --enable-version3
  --disable-debug
  --enable-shared
  --disable-static
  --disable-w32threads
  --enable-pthreads
  --enable-iconv
  --enable-zlib
  --enable-libxml2
  --enable-libvmaf
  --enable-fontconfig
  --enable-libharfbuzz
  --enable-libfreetype
  --enable-libfribidi
  --enable-lcms2
  --enable-vulkan
  --enable-libdav1d
  --enable-libvorbis
  --enable-librav1e
  --enable-librsvg
  --disable-libxcb
  --disable-xlib
  --disable-libpulse
  --enable-gmp
  --enable-lzma
  --enable-liblcevc-dec
  --enable-opencl
  --enable-amf
  --enable-libaom
  --disable-avisynth
  --enable-chromaprint
  --disable-libdavs2
  --disable-libdvdread
  --disable-libdvdnav
  --disable-libfdk-aac
  --enable-ffnvcodec
  --enable-cuda-llvm
  --disable-frei0r
  --enable-libgme
  --enable-libkvazaar
  --enable-libaribb24
  --enable-libaribcaption
  --enable-libass
  --enable-libbluray
  --enable-libjxl
  --enable-libmp3lame
  --enable-libopus
  --enable-libplacebo
  --enable-librist
  --enable-libssh
  --enable-libtheora
  --enable-libvpx
  --enable-libwebp
  --enable-libzmq
  --enable-lv2
  --enable-libvpl
  --enable-openal
  --enable-liboapv
  --enable-libopencore-amrnb
  --enable-libopencore-amrwb
  --enable-libopenh264
  --enable-libopenjpeg
  --enable-libopenmpt
  --disable-librubberband
  --enable-schannel
  --enable-sdl2
  --enable-libsnappy
  --enable-libsoxr
  --enable-libsrt
  --enable-libsvtav1
  --enable-libtwolame
  --enable-libuavs3d
  --disable-libdrm
  --enable-vaapi
  --enable-vapoursynth
  --disable-libvidstab
  --enable-libvvenc
  --disable-whisper
  --disable-libx264
  --disable-libx265
  --disable-libxavs2
  --disable-libxvid
  --enable-libzimg
  --disable-libzvbi
  --extra-cflags=-DLIBTWOLAME_STATIC
  --extra-cxxflags=
  --extra-libs=-lgomp
  --extra-ldflags=-pthread
  --extra-ldexeflags=
  --cc=x86_64-w64-mingw32-gcc
  --cxx=x86_64-w64-mingw32-g++
  --ar=x86_64-w64-mingw32-gcc-ar
  --ranlib=x86_64-w64-mingw32-gcc-ranlib
  --nm=x86_64-w64-mingw32-gcc-nm
  --extra-version=20261119
```

---

## 4. Source Mirroring & Long-Term Availability Plan

To comply with LGPLv3 Section 6(a) and ensure that source code remains available for at least 3 years after the last binary distribution:

1. **Upstream Source Mirrors:**
   - Official upstream tarball: `https://ffmpeg.org/releases/ffmpeg-9.0.tar.xz`
   - BtbN build scripts commit: `https://github.com/FFmpeg-Builds/FFmpeg-Builds/tree/autobuild-2026-11-19-12-39`
2. **Clipper Helper Infrastructure Mirror (Cloudflare R2):**
   - Every binary package distributed via the signed component manifest is accompanied by an exact source tarball stored on Cloudflare R2:
     `https://cdn.clipperhelper.com/sources/ffmpeg/ffmpeg-n9.0.2-58-g8c7ef49002-source.tar.xz`
   - The SHA-256 checksum of the source archive is published in `THIRD_PARTY_NOTICES.md` and in the component manifest.
3. **Written Offer:**
   - Any user can download the corresponding source code directly from the CDN link or request a copy by contacting `support@clipperhelper.com`.

---

## 5. Architectural Process Boundary Rules

To ensure that Clipper Helper and FFmpeg remain legally distinct and independent works:

1. **Process Isolation (`std::process::Command`):**
   - FFmpeg and ffprobe are invoked exclusively via standard operating system process spawning.
   - All invocations pass typed argument arrays (`&[&str]`). Shell strings (`cmd.exe /c` or PowerShell string expansion) are strictly forbidden (`AGENTS.md §1.7`).
2. **No Memory Sharing / No FFI:**
   - No C header files from FFmpeg (`libavformat/*.h`, `libavcodec/*.h`) are included in Clipper Helper's Rust or TypeScript codebases.
   - No Rust FFI bindings (e.g. `ffmpeg-sys` or `ffmpeg-next`) are linked.
3. **Standard Input/Output Communication:**
   - Progress and diagnostics are read solely via redirected standard error (`stderr`) and parsed by `sidecars/ffmpeg/progress.rs`.
   - Video frames are never transferred via shared memory; inputs and outputs are standard file paths managed within the workspace sandbox (`%LOCALAPPDATA%\ClipperHelper\`).
4. **Supervision via Windows Job Object:**
   - Processes are assigned to a Windows Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` to ensure that orphaned processes never linger if Clipper Helper crashes or exits.

---

## 6. End-User Binary Replacement Instructions (LGPLv3 §6)

In accordance with LGPLv3 user modification rights, users have the right to modify or replace the FFmpeg binaries used by Clipper Helper.

### Procedure for Replacing FFmpeg:

1. **Exit Clipper Helper:**
   Ensure Clipper Helper is completely closed and no background jobs are running.

2. **Locate the Sidecar Directory:**
   Open Windows File Explorer and navigate to:
   ```
   %LOCALAPPDATA%\ClipperHelper\bin\ffmpeg\
   ```
   *(Full path typically: `C:\Users\<Username>\AppData\Local\ClipperHelper\bin\ffmpeg\`)*

3. **Enable "Use custom FFmpeg" Setting (Integrity Check Bypass):**
   Clipper Helper by default enforces SHA-256 binary integrity verification on all sidecars at startup (`SECURITY §9`). To reconcile this security control with LGPLv3 §6 user replacement rights:
   - Navigate to **Settings > Advanced > Sidecars**.
   - Enable **"Use custom FFmpeg"** (expert setting, off by default).
   - Alternatively, edit `%LOCALAPPDATA%\ClipperHelper\app_config.json` and set `"use_custom_ffmpeg": true`.
   - **Scope & Safety Invariant:** When enabled, the app skips startup SHA-256 integrity verification **exclusively for the FFmpeg sidecar (`ffmpeg.exe`, `ffprobe.exe`, and shared DLLs)**. It shows a persistent in-app warning banner ("Peringatan: FFmpeg kustom sedang aktif. Validasi integritas dinonaktifkan.") and logs the override into diagnostics. This setting **never** applies to other sidecars (`yt-dlp.exe`, `qjs.exe`, `whisper.exe`, speech models), which remain strictly integrity-verified against signed manifests.

4. **Replace the Executables and DLLs:**
   Replace `ffmpeg.exe`, `ffprobe.exe`, and any associated shared DLLs (`avcodec-*.dll`, `avfilter-*.dll`, etc.) with your own custom-built binaries.

5. **Requirements for Custom Builds:**
   To ensure compatibility with Clipper Helper's timeline engine and subtitle burn-in:
   - Must support `ass` and `subtitles` filters (compiled with `libass` and `libfreetype`).
   - Must support required codecs (`h264`, `aac`, `opus`, `vp9`, `av1`).
   - Must support hardware encoders (`h264_mf`, `h264_nvenc`, `h264_amf`, or `h264_qsv`).
   - Must support `gblur` or `avgblur` for blur-fit layouts.
   - Must NOT rely on GPL-only filters (see `docs/benchmarks/gpl-filters-denylist.txt`).

6. **Locking Component Updates:**
   To prevent Clipper Helper's background component updater from overwriting your custom build:
   - In `%LOCALAPPDATA%\ClipperHelper\app_config.json`, ensure `"lock_ffmpeg_version": true`.

7. **Verification:**
   Open Clipper Helper and navigate to **Settings > Diagnostics > Verify Sidecars**. The app will execute `ffmpeg -version` and display your custom build version and active configuration in the diagnostics panel.

---

## 7. `libopenh264` Patent Policy & Posture

Although `libopenh264` is compiled into the candidate BtbN release build (`--enable-libopenh264`), Clipper Helper treats it as **UNAPPROVED** pending written legal opinion:

1. **Cisco Patent Grant Scope:**
   Cisco's official statement and patent license grant for OpenH264 specifies that Cisco absorbs MPEG LA (Via LA) licensing fees **only for binary modules downloaded directly from Cisco's official servers**.
2. **Third-Party Compilation Risk:**
   Compiling OpenH264 directly from source into a distributed third-party binary (such as BtbN's MinGW cross-build) does not automatically inherit Cisco's patent royalty indemnification. Distributing such a binary may expose distributors to patent royalty claims under the AVC Patent Portfolio.
3. **Operational Enforcement:**
   - `libopenh264` is strictly **excluded** from Clipper Helper's encoder selection chain.
   - The encoder fallback chain is strictly: `h264_nvenc → h264_qsv → h264_amf → h264_mf → E_ENCODER_UNAVAILABLE`.
   - `libopenh264` will not be invoked unless approved by qualified patent counsel.

---

## 8. Open Legal Questions for Counsel

1. **"User Product" Definition under LGPLv3 §6:**
   Does a Windows desktop application distributed as an installer or portable executable qualify as a "User Product" under LGPLv3 (incorporating GPLv3 Section 6)? Specifically, does standard Windows code signing (Authenticode) require providing cryptographic keys or is user replacement in `%LOCALAPPDATA%\` sufficient?
2. **Cisco OpenH264 Source Builds vs Dynamic Cisco Binary Download:**
   Confirm whether shipping third-party compiled OpenH264 binaries creates patent liability under Via LA AVC pool terms.
3. **H.264 Software Decoders (`avcodec`):**
   Confirm applicability of the Via LA royalty-free threshold (<100,000 units/year) to software decoders bundled in free desktop software.
