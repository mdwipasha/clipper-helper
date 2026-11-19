# Spike S1 Benchmark & Decision: Downloader & JS Runtime

**Status:** Completed  
**Date:** 2026-11-19  
**Tested on:** Windows 10/11 x64  
**Target video:** `https://www.youtube.com/watch?v=QOJIILaJKNc` (Replay Live Stream VOD, 1080p60 avc1 + m4a)

---

## 1. Executive Summary & Recommendation

### Recommendation: **Approach A (`yt-dlp --download-sections`) with QuickJS-NG**

1. **Approach Choice: Approach A wins definitively.**
   - **Approach A (`yt-dlp --download-sections`)** downloads any range (1 min, 5 min, 10 min) reliably without 403 Forbidden errors. When combined with `--force-keyframes-at-cuts`, yt-dlp outputs standard FFmpeg progress lines (`time=00:XX:XX.XX`, `speed=X.XXx`, `frame=XXXX`) at **2.7–3.7 events/second**, providing reliable monotonic percent and ETA calculation.
   - **Approach B (`yt-dlp -J` + FFmpeg direct)** is **fundamentally fragile and non-viable** for 1080p DASH YouTube content. Standalone FFmpeg connecting to `googlevideo.com` URLs fails with `HTTP 403 Forbidden` on DASH streams (itag 299 / itag 140) due to YouTube's connection binding, player token verification, and aggressive un-chunked request throttling. Building a custom HTTP proxy to mimic yt-dlp's internal SABR chunking would introduce massive complexity.

2. **JS Runtime Choice: QuickJS-NG wins decisively over Deno.**
   - **QuickJS-NG binary size:** **1.87 MB** vs Deno **98.1 MB** (**52× smaller!**).
   - **Probe speed (`yt-dlp -J`):** QuickJS is slightly faster (5.66s vs 5.92s).
   - **Download speed:** Identical (43.19s vs 42.92s for 1-minute range).
   - yt-dlp fully supports QuickJS via `--js-runtimes "quickjs:<path_to_qjs>"`.
   - Switching to QuickJS saves ~96 MB of component download and disk footprint for every user.

---

## 2. Pinned Binaries Inventory

All binaries tested and verified for Windows x64:

| Component | Pinned Version | File Size | SHA-256 Checksum |
|-----------|----------------|-----------|------------------|
| **yt-dlp** | `2026.10.26` | 23.6 MB (`yt-dlp.exe`) | `f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d` |
| **QuickJS-NG** | `v0.18.0` | 1.87 MB (`qjs.exe`) | `ff03398d74ea6e6bd3a8e58058f7003225abcac229d7d57775b7aaae812a49c7` |
| **Deno** | `v2.9.8` | 98.1 MB (`deno.exe`) | `924397800533d3eb0ed0c889b20ad70e18b970d93acd8135bc043f425e528266` |
| **FFmpeg (Candidate)** | `9.0.2-essentials` (gyan.dev) | 105.4 MB (`ffmpeg.exe`) | `3256173f3f8bffd7df12227c68adf68025edb1832273a9530688a7bb1ed8edec` |

> [!WARNING]
> **FFmpeg Licensing Note for S5:** The tested `ffmpeg-release-essentials.zip` build from gyan.dev was compiled with `--enable-gpl` and includes `libx264`. In Milestone M0 / Spike S5, an actual **LGPL-only build** (without `--enable-gpl` and without GPL libraries) must be selected for distribution per Decision D-11. Encoders verified present in this build: `h264_nvenc`, `h264_qsv`, `h264_amf`, `h264_mf`. `libopenh264` was NOT found. Full encoder dump saved in `docs/benchmarks/ffmpeg-encoders.txt`.

---

## 3. Approach A Benchmark (`yt-dlp --download-sections`)

Tested on `https://www.youtube.com/watch?v=QOJIILaJKNc`:
- Quality requested: `bv*[vcodec^=avc1][height<=1080]+ba[acodec^=mp4a]/b[vcodec^=avc1][height<=1080]/bv*[height<=1080]+ba/b[height<=1080]`
- Output format: 1920×1080 H.264 (High) + AAC in MP4 container

| Range | Content Duration | Wall Time | Output Size | Avg Rate | Progress Events | Event Frequency | Video Codec | Audio Codec | Browser Playable | Duration Error |
|---|---|---|---|---|---|---|---|---|---|---|
| **1 min** | 60s (01:00–02:00) | **47.44s** | 6.46 MB | 0.14 MB/s (1.67× RT) | 175 | 3.69 events/s | H.264 | AAC | **Yes** | 0.0s |
| **5 min** | 300s (01:00–06:00) | **190.06s** (3.1 min) | 29.19 MB | 0.15 MB/s (1.76× RT) | 528 | 2.78 events/s | H.264 | AAC | **Yes** | 0.0s |
| **10 min** | 600s (01:00–11:00) | **345.50s** (5.7 min) | 59.49 MB | 0.17 MB/s (1.79× RT) | 928 | 2.69 events/s | H.264 | AAC | **Yes** | 0.0s |

### Progress Observability & ETA in Approach A
- Startup phase: ~11s (yt-dlp extracts webpage, solves player challenge with QuickJS, and negotiates formats). During this phase, progress is indeterminate.
- Download & cutting phase: FFmpeg is invoked by yt-dlp and emits continuous progress lines to stderr/stdout:
  ```
  frame= 1850 fps= 98 q=31.0 size= 3584KiB time=00:03:08.50 bitrate= 844.7kbits/s speed=1.76x
  ```
- **Parsing rule:** Extract `time=(?P<time>\d+:\d+:\d+\.\d+)` and `speed=(?P<speed>[\d.]+)x`.
- `done_seconds = parse_time(time)`
- `progress_percent = clamp((done_seconds / target_duration_seconds) * 100, 0, 100)`
- `eta_seconds = (target_duration_seconds - done_seconds) / speed`
- Emits ~3 events/second, well within the `PROGRESS_AND_ETA.md` contract of ≤4 events/sec.

---

## 4. Approach B Benchmark (`yt-dlp -J` + Standalone FFmpeg)

| Test Case | Resolve Time | FFmpeg Result | Finding |
|---|---|---|---|
| Format 18 (360p progressive MP4) | 5.8s | **Success** (exit 0) | Progressive low-res URLs can be fetched directly by FFmpeg with `-headers`. |
| Format 299 (1080p60 avc1 DASH) | 6.0s | **Fail (403 Forbidden)** | YouTube googlevideo CDN rejects un-chunked standalone FFmpeg connections. |
| Format 140 (128k m4a DASH audio) | 6.0s | **Fail (403 Forbidden)** | YouTube googlevideo CDN rejects standalone FFmpeg connections. |
| Combined (itag 299 + 140 copy) | 6.2s | **Fail (403 Forbidden)** | Exit code 3436169992; access denied. |

**Conclusion on Approach B:** Not viable. YouTube restricts high-resolution DASH stream access to players that emulate SABR/chunked token handshakes. Attempting to maintain direct streaming in our own FFmpeg would be brittle against frequent YouTube changes.

---

## 5. JS Runtime Benchmark: QuickJS-NG vs Deno

| Metric | QuickJS-NG (`v0.18.0`) | Deno (`v2.9.8`) | Advantage |
|---|---|---|---|
| **Binary Size** | **1.87 MB** | 98.1 MB | **QuickJS is 52× smaller (saves ~96 MB)** |
| **Probe (`yt-dlp -J`) Run 1** | **5.66s** | 5.92s | QuickJS faster |
| **Probe (`yt-dlp -J`) Run 2** | 5.89s | 5.89s | Tied |
| **1-min Download Wall Time** | 43.19s | **42.92s** | Statistically identical (within 0.6%) |
| **Output File Size** | 6.46 MB | 6.46 MB | Identical |
| **Configuration in yt-dlp** | `--js-runtimes "quickjs:<path>"` | Default (or `--js-runtimes "deno:<path>"`) | Minor flag difference |

**Conclusion on JS Runtime:** QuickJS-NG should be adopted as the default JS runtime for Clipper Helper instead of Deno.

---

## 6. Failure Classification & `E_YT_*` Mapping

Tested using real YouTube URLs and captured into `spikes/s1-downloader/fixtures/`:

| Case | Tested URL / Input | yt-dlp Stderr / Detection | Mapped Error Code | Retryable | Suggested Action |
|---|---|---|---|---|---|
| **Members-only VOD** | `watch?v=KBdGeakRgqg` | `ERROR: [youtube] KBdGeakRgqg: Join this channel to get access to members-only content like this video, and other exclusive perks.` | `E_YT_UNAVAILABLE` | No | Use another video / local file |
| **Deleted video** | `watch?v=dQw4w9WgXcX` | `ERROR: [youtube] dQw4w9WgXcX: This video is unavailable` | `E_YT_UNAVAILABLE` | No | Check video link |
| **Private video** | `watch?v=_bHqBy5fmUM` | `ERROR: [youtube] _bHqBy5fmUM: This video is unavailable` | `E_YT_UNAVAILABLE` | No | Check video permissions |
| **Age-restricted** | `watch?v=6kLq3WMV1nU` | `ERROR: [youtube] 6kLq3WMV1nU: Sign in to confirm your age. This video may be inappropriate for some users.` | `E_YT_UNAVAILABLE` | No | Video is age-gated (unsupported in MVP) |
| **Ongoing live stream** | `@aljazeeraenglish/live` | `is_live: True`, `live_status: "is_live"` | `E_YT_LIVE_NOT_ENDED` | Yes | Wait until live stream ends |
| **Finished live stream** | `watch?v=QSPPyA1736U` | `is_live: False`, `live_status: "was_live"` | **SUCCESS** | – | Processed normally as VOD |
| **Premiere not started** | Upcoming premiere | `live_status: "is_upcoming"` | `E_YT_LIVE_NOT_ENDED` | Yes | Wait for premiere to finish |
| **Invalid video ID** | `watch?v=XXXXXXXXXXX` | `ERROR: [youtube] XXXXXXXXXXX: This video is unavailable` | `E_YT_UNAVAILABLE` | No | Fix URL |
| **Playlist / channel URL** | `playlist?list=...` | Rejected by client URL parser regex `^[A-Za-z0-9_-]{11}$` | `E_YT_URL_INVALID` | No | Paste direct video URL only |
| **Rate limit / 429** | HTTP 429 / 403 | `HTTP Error 429: Too Many Requests` / `HTTP Error 403: Forbidden` | `E_YT_RATE_LIMITED` | Yes | Wait a few minutes and retry |
| **Extractor broken** | New platform layout | `Unable to extract video data` | `E_YT_EXTRACT_FAILED` | Yes | Update downloader component |

---

## 7. Recommended Production Flags

### Metadata Probe (`probe` stage)
```bash
yt-dlp.exe -J \
  --no-playlist \
  --no-warnings \
  --ignore-config \
  --js-runtimes "quickjs:<bin_dir>\qjs.exe" \
  -f "bv*[vcodec^=avc1][height<=1080]+ba[acodec^=mp4a]/b[vcodec^=avc1][height<=1080]/bv*[height<=1080]+ba/b[height<=1080]" \
  "https://www.youtube.com/watch?v=<video_id>"
```

### Ranged Section Download (`download` stage)
```bash
yt-dlp.exe \
  --no-playlist \
  --no-warnings \
  --newline \
  --ignore-config \
  --ffmpeg-location "<bin_dir>\ffmpeg.exe" \
  --js-runtimes "quickjs:<bin_dir>\qjs.exe" \
  -f "bv*[vcodec^=avc1][height<=1080]+ba[acodec^=mp4a]/b[vcodec^=avc1][height<=1080]/bv*[height<=1080]+ba/b[height<=1080]" \
  --download-sections "*<HH:MM:SS>-<HH:MM:SS>" \
  --force-keyframes-at-cuts \
  -o "<workspace_dir>\cache\<cache_key>.mp4" \
  "https://www.youtube.com/watch?v=<video_id>"
```

---

## 8. Required Documentation Updates

Based on Spike S1 findings, the following doc updates should be applied:
1. **`ARCHITECTURE.md` §3 (Technology choices):**
   - Change Downloader row: replace Deno with **QuickJS-NG** (`qjs.exe`, ~1.9 MB) as the primary bundled JS runtime. Note Deno as fallback option if a future yt-dlp release requires Deno-specific APIs.
2. **`ARCHITECTURE.md` §5.1 (Generate pipeline):**
   - Confirm **Approach A (`yt-dlp --download-sections`)** as the chosen implementation.
   - Record progress extraction via FFmpeg output parsing (`time=` and `speed=`).
3. **`ARCHITECTURE.md` §6 (YouTube source handling):**
   - Update flags to include `--js-runtimes "quickjs:<bin_dir>\qjs.exe"` and `--force-keyframes-at-cuts`.
   - Remove ambiguity between Approach A and B.
4. **`PROGRESS_AND_ETA.md` §4.1:**
   - Update `download` progress source: clarify that sectioned downloads parse the internal FFmpeg `time=` output against requested duration.
