# Progress and ETA

**Product requirement (D-15):** for every operation that can take more than ~2 seconds, the user must always be able to see **(1) what is happening, (2) how far along it is, (3) roughly how long is left, (4) what they can do about it** (cancel, wait, retry). No bare spinners.

## 1. Principles

1. **Honest over precise.** Show ranges or rounded values; say "Menghitung estimasi…" until the estimate is trustworthy.
2. **Stage-based.** Operations are sequences of named stages; the user sees the current stage name and the overall bar.
3. **Monotonic bar.** Overall progress never goes backwards; ETA may change but is damped.
4. **Actionable errors.** Every error has a plain-language message and a next step (catalog §8).
5. **Persistent.** Progress state survives window close/minimize; Windows toast on completion when the app is in the background.
6. **One source of truth.** The Rust core computes progress and ETA; the UI renders what it receives.

## 2. Operations and stages

Default stage weights are *initial* estimates of time share on a mid-spec laptop; they are replaced by calibrated values (§4.3) after the first runs.

### 2.1 `generate`
Free local operation. Download-only: fetches the chosen range and constructs the initial composition without captions.
| Stage id | User-facing text (id) | Work unit | Initial weight |
|----------|----------------------|-----------|----------------|
| `probe` | Memeriksa link video… | indeterminate (timeout 30 s) | 5% |
| `download` | Mengunduh potongan video | media-seconds (startup 6–11 s is indeterminate: "Menyiapkan unduhan…") | 85% |
| `prepare_preview` *(only if needed)* | Menyiapkan pratinjau | media-seconds | 8% |
| `build` | Menyiapkan editor | indeterminate (<2 s) | 2% |

### 2.2 `captions`
Free local operation. Triggered after trimming: transcribes ONLY the trimmed segment(s) (+ ~2 s padding).
| Stage id | User-facing text (id) | Work unit | Initial weight |
|----------|----------------------|-----------|----------------|
| `extract_audio` | Menyiapkan audio clip… | audio-seconds | 10% |
| `transcribe` | Membuat caption ({model})… | audio-seconds | 85% |
| `map` | Menyelaraskan teks… | indeterminate (<1 s) | 5% |

### 2.3 `export`
| Stage id | Text (id) | Unit | Weight |
|----------|-----------|------|--------|
| `ticket` | Memeriksa kredit & izin export… | indeterminate (approval request timeout 15 s; distinct from ticket TTL of 15 min) | 2% |
| `analyze_audio` *(if normalize on)* | Menganalisis volume audio | media-seconds | 8% |
| `render` | Merender video | output frames (or output-seconds) | 82% |
| `verify` | Memeriksa hasil | indeterminate | 5% |
| `finalize` | Menyimpan file | indeterminate | 3% |

### 2.4 Others
`model_download` (bytes), `component_update` (bytes per component), `app_update` (bytes), `first_run_benchmark` (steps: CPU probe, encoder test, speech test — "Menyiapkan aplikasi untuk laptop kamu", ≈10–30 s with step list), `thumbnails` (FFmpeg filmstrip extraction: 160 px wide thumbnails every 2 s over the source file, cached under `cache/thumbs/<source_hash>/` with a manifest; cancel via `ThumbnailCancelRegistry`; progress via `ThumbnailProgressEvent` { percent, thumbnailCount }; cap by total count/file size; runs as a background operation outside the job queue).

## 3. Event contract (Rust → UI)

```ts
type JobEvent = {
  jobId: string; kind: 'generate' | 'captions' | 'export' | 'model_download' | 'component_update' | 'app_update' | 'benchmark';
  state: 'queued' | 'running' | 'paused' | 'succeeded' | 'failed' | 'cancelled' | 'interrupted';
  stage: { id: string; index: number; count: number; labelKey: string; labelArgs?: Record<string, string | number> };
  stageProgress: { done: number; total: number | null; unit: 'bytes' | 'media_seconds' | 'frames' | 'audio_seconds' | 'steps' };
  overall: { percent: number };                           // 0..100, monotonic
  rate?: { value: number; unit: string; smoothed: true }; // e.g. 6.2e6 bytes/s, 1.4 x realtime, 24 fps
  eta?: { seconds: number; lowSeconds: number; highSeconds: number; confidence: 'low' | 'medium' | 'high' } | null;
  detail?: Record<string, string | number>;               // e.g. {encoder:'h264_nvenc'}
  error?: { code: string; messageKey: string; retryable: boolean; actionKeys: string[] };
  ts: number;                                             // monotonic ms
};
```
Emit rules: at most **4 events/second** per job (coalesce); always emit on stage/state change; emit a heartbeat every 2 s while running even if numbers did not change (so the UI can show "still working" and detect stalls).

## 4. Measuring progress and estimating time

### 4.1 Progress sources
| Stage | How progress is read |
|-------|----------------------|
| `download` | FFmpeg `time=`/`speed=` output parsed from yt-dlp's internal FFmpeg invocation (≈1.4–1.9 events/s in copy mode). The 6–11 s startup (extraction + JS challenge resolution) is indeterminate — show "Menyiapkan unduhan…". Total duration = requested range duration. Reference implementation: `spikes/s1-downloader/progress_parser.py` (spec for Rust parser; port its unit-test cases). |
| `extract_audio` | FFmpeg `-progress pipe:1`: extracts 16 kHz mono WAV for trimmed segment(s) (+ ~2 s padding). Work unit = audio-seconds. |
| `transcribe` | whisper.cpp `--print-progress` percent × clip audio duration. Whisper emits progress updates sparsely (~0.1 events/s). The Rust sidecar supervisor emits **synthetic heartbeats every 2 s** while running to satisfy the UI activity tray contract (§3) and prevent false stall detections. |
| `map` | Rust maps source-relative words to output timeline and applies user edits (<1 s). |
| `analyze_audio`, `prepare_preview`, `render` | FFmpeg `-progress pipe:1`: `out_time_us` ÷ expected duration (render: `frame` ÷ total frames), plus `speed` and `fps`. |
| `model_download`, `component_update`, `app_update` | Bytes received ÷ `Content-Length` (verify hash after; resumable). |
| indeterminate stages | Elapsed time + stage name; timeouts produce `E_*` errors. |

### 4.2 ETA algorithm
For the running stage *s* with total work `W`, done `D`:
- Live rate `r_live` = EMA (α = 0.2) of instantaneous rate over 2 s windows (download uses median of last 10 s to resist bursts).
- Prior rate `r_prior` = median of the last 10 `stage_stats` for this device profile and variant (model/encoder); if none, benchmark-derived; if none, built-in default table by device class (§4.4).
- Blend: `w = clamp((D/W) / 0.15, 0, 1)`; `r = w·r_live + (1−w)·r_prior` (after ≥3 s of data; before that use `r_prior`).
- `remaining_s = (W − D) / r`.
- Future stages: `remaining_k = W_k / r_prior_k`, where `W_k` is derived from known quantities (range duration, output duration, expected download size).
- **Operation-specific ETA rules:**
  - `generate` ETA is download-only: `range_seconds / 1.66 + ~10 s startup` (+ proxy transcode if needed).
  - `captions` ETA: `clip_seconds / rate + 8 s` (model-init prior = 8 s; transcribe rate priors are **RANGES** 1.1–2.2× real-time depending on speech density, background load, and thermal throttling; marked low confidence until measured on device). Trim bounds are snapshotted when the job starts.
- **ETA = Σ remaining** (running + future). Range: `[0.85·ETA, 1.3·ETA]` when confidence is low/medium, narrower when high.
- **Confidence:** low until ≥10 s of data *and* ≥5% of the stage done *or* a calibrated prior exists; high when `r_live` has been stable (coefficient of variation < 0.25) for ≥20 s.
- **Overall percent:** `Σ(completed stage weights) + weight_current × (D/W)`, with weights = calibrated expected time share. Clamped to be non-decreasing.

### 4.3 Calibration
After every completed stage, store `(device_profile, stage, variant, work_units, wall_ms)` in `stage_stats`. First-run benchmark seeds the priors. Telemetry (opt-in) may report `stage_ms` by device class to improve built-in defaults. Speech priors (model × device class) apply to **CLIP length**, not range length, and are represented as **RANGES** (e.g. 1.1–2.2× real-time for `small-q5_1` with 8 s initialization) to account for benchmark variance (e.g. the same 60 s audio ranged from 37.7 s to 53.2 s on R1). Re-run benchmark after GPU driver/hardware change or from settings.

### 4.4 Initial built-in rate priors (seed table)
Initial priors before first calibration or benchmark. Hardware encoders reconciled from S4 raw data on Reference Machine R1:

| Operation / Stage | Variant / Hardware | $r_{prior}$ | Basis / Source |
|---|---|---|---|
| `download` | All devices | **1.66× real-time** | S1 baseline copy-mode on YouTube DASH |
| `captions` (transcribe) | `small-q5_1` (CPU AVX2) | **RANGE 1.1–2.2× real-time** (+ 8 s init) | S3b/S3d benchmark range (reflects 37.7 s to 53.2 s variance on R1; low confidence until measured) |
| `captions` (transcribe) | `base` (Class C fallback) | **RANGE 2.5–4.0× real-time** (+ 5 s init) | S3b micro-benchmark range |
| `export` (render) | `h264_amf` (AMD GPU) | **2.70× real-time** (81 fps) | S4b measured median on R1 (22.01 s for 60 s @ 4.4 Mbps) |
| `export` (render) | `h264_mf` (Media Foundation) | **3.50× real-time** (105 fps) | S4b measured median on R1 (16.76 s for 60 s @ 4.0 Mbps) |
| `export` (render) | `h264_nvenc` (NVIDIA GPU) | **1.00× real-time** (conservative default) | **UNTESTED**; numeric placeholder removed. Replaced on first-run benchmark. |
| `export` (render) | `h264_qsv` (Intel QSV) | **1.00× real-time** (conservative default) | **UNTESTED**; numeric placeholder removed. Replaced on first-run benchmark. |

## 5. Display rules

| Rule | Behavior |
|------|----------|
| Refresh | UI updates ETA text at most 1×/s. |
| Damping | Ignore ETA changes < 10% unless > 5 s; do not increase ETA by > 20% within 5 s unless the stage changed or a stall was detected. |
| Rounding | < 10 s: "beberapa detik lagi" · 10–60 s: round to 5 s · 1–10 min: round to 15 s · > 10 min: round to 1 min. |
| Low confidence | Show "Menghitung estimasi…" (first seconds) or a range: "sekitar 2–4 menit lagi". |
| Stalls | No progress for 15 s while running: "Koneksi lambat / proses belum bergerak — masih mencoba…" with Cancel; 60 s: offer Retry. |
| Totals unknown | Show indeterminate bar + elapsed time + stage name. |
| Rate text | Download: "6,2 MB/dtk"; transcribe/render: "1,4× kecepatan normal" or "24 fps". |

## 6. UI components

1. **Operation panel** (in the flow screen): stage list with ✓ done / ● current / ○ upcoming; overall bar with %; line "Tahap 2 dari 3 · Membuat caption (Small) · 42% · sekitar 40 dtk lagi"; details toggle (rate, encoder/model, file size so far); **Batal** button.
2. **Global activity tray** (top bar): compact chip per running job (icon, %, ETA) so users can navigate while it runs.
3. **Toast / Windows notification** on success or failure when the window is not focused.
4. **First-run wizard** with the benchmark and component downloads, each with its own bar.

**Copy examples (id):**
- Download: "Mengunduh potongan video · 42 MB dari ±118 MB · 6,2 MB/dtk · sekitar 15 dtk lagi"
- Captions: "Membuat caption · model Small · sekitar 40 dtk lagi"
- Render: "Merender video · frame 820 dari 1.800 · encoder NVIDIA (h264_nvenc) · sekitar 20 dtk lagi"
- Ticket: "Memeriksa kredit… Export ini memakai 1 kredit (sisa 2)."
- Done: "Selesai! File tersimpan di Videos\ClipperHelper. Buka folder · Salin caption & hashtag"

## 7. Cancel, pause, resume

| Stage | Cancel | Resume |
|-------|--------|--------|
| `probe` | Abort request | n/a |
| `download` | Kill process tree; delete partial file (.part) | Restart stage (download is NOT resumable; restart overwrites the .part file from scratch) |
| `prepare_preview`, `analyze_audio` | Kill; delete temp output | Restart stage |
| `extract_audio` | Kill; delete temp audio | Restart stage |
| `transcribe` | Kill; discard partial results (cancelling never blocks editing) | Restart stage |
| `map` | Not cancellable (<1 s) | – |
| `ticket` | Abort; if the server already processed it, the next attempt with the same idempotency key returns the same result | – |
| `render` | Kill; delete temp output | Restart render (free: entitlement exists) |
| `verify`/`finalize` | Not cancellable (very short) | – |

Cancelling `captions` kills the sidecar immediately, discards partial results, and reverts `captions.status` to `'none'`. Cancelling captions **never blocks editing** (user can continue trimming, adjusting layout, or changing text overlays uninterrupted). Cancelling during `download` warns: "Proses unduh akan dihentikan dan harus diulang dari awal jika dilanjutkan." Cancelling **after** a ticket was issued shows: "Kredit sudah dipakai untuk proyek ini. Kamu bisa export ulang gratis." A job interrupted by crash/reboot is offered for resume at next launch using the persisted `jobs` row (non-resumable stages like download restart from 0%).

## 8. Error catalog

User text is Indonesian (i18n keys `err.<code>.title/body`); each entry lists default actions.

| Code | Meaning | Retryable | Actions |
|------|---------|-----------|---------|
| `E_NET_OFFLINE` | No internet | yes | Retry · Check connection |
| `E_NET_TIMEOUT` | Request/download timed out | yes | Retry |
| `E_MANIFEST_INVALID` | Manifest verification failed (bad signature, unknown kid, malformed JSON, schema mismatch) | no | Contact support · Retry · Update app |
| `E_MANIFEST_STALE` | Manifest sequence is older than last accepted or build floor (rollback attack prevention) | no | Check update channel · Retry |
| `E_YT_URL_INVALID` | Not a supported single YouTube video URL (`err.E_YT_URL_INVALID.{body,collection,too_long,host}`) | no | Fix link |
| `E_YT_UNAVAILABLE` | Video unavailable on YouTube. Carries `reason` mapped to distinct Indonesian keys `err.E_YT_UNAVAILABLE.<reason>` (`members_only` \| `age_restricted` \| `region_blocked` \| `private` \| `deleted` \| `unknown`) | no | Try another video / local file |
| `E_YT_LIVE_NOT_ENDED` | Stream live/upcoming/processing (`err.E_YT_LIVE_NOT_ENDED.{ongoing_live,premiere_not_started,post_live}`) | yes | Try again after the stream ends |
| `E_YT_BOT_CHECK` | "Sign in to confirm you're not a bot" anti-bot challenge | no (not immediately) | Wait a few minutes · Change network/VPN · (Not fixed by updating downloader) |
| `E_YT_RATE_LIMITED` | Platform rate limit / HTTP 403 or 429 (after silent re-probe retry fails) | yes | Wait a few minutes · Retry |
| `E_YT_EXTRACT_FAILED` | Downloader can't read the video (unknown classification falls here; raw text in redacted logs only) | yes | **Update downloader** · Retry · Use local file |
| `E_RANGE_INVALID` / `E_RANGE_TOO_LONG` | Start/end invalid or over the cap | no | Adjust range |
| `E_JOB_BUSY` | Another exclusive job (e.g. `generate`) is already active | yes | Wait or cancel active job |
| `E_JOB_CANCELLED` | Active job was cancelled by the user | yes | Retry |
| `E_DISK_LOW` / `E_DISK_FULL` | Not enough free space | yes | Free space · Clean cache |
| `E_COMPONENT_MISSING` | Required sidecar/model missing or hash mismatch | yes | Repair (re-download) |
| `E_MODEL_DOWNLOAD_FAILED` | Speech model download failed | yes | Retry (resumes) |
| `E_TRANSCRIBE_FAILED` / `E_TRANSCRIBE_OOM` | Speech step failed / out of memory | yes | Switch to smaller model · Retry |
| `E_ENCODER_UNAVAILABLE` | No working video encoder found after trying the lazy chain (`h264_nvenc` → `h264_qsv` → `h264_amf` → `h264_mf`) or runtime fallback exhausted | yes (manual) | Update GPU driver · Install Media Feature Pack (Windows N/KN) · Export diagnostics |
| `E_RENDER_FAILED` | FFmpeg error | yes | Retry (free) · Export diagnostics |
| `E_VERIFY_FAILED` | Output failed post-render checks | yes | Review checklist · Retry |
| `E_AUTH_EXPIRED` | Session expired | yes | Sign in again |
| `E_NO_CREDITS` | Daily credits used up | no (until reset) | Buy pass · Show reset time |
| `E_DEVICE_LIMIT` / `E_DEVICE_INVALID` | Too many devices / device revoked | no | Manage devices |
| `E_DEVICE_LIMIT_FREE` | Free-tier cap for this PC reached | no | Sign in with your own account / buy pass |
| `E_REEXPORT_CAP` | Too many re-exports of this clip today | no | Try tomorrow / contact support |
| `E_ACCOUNT_SUSPENDED` | Account suspended | no | Contact support |
| `E_APP_OUTDATED` | App below minimum version | yes | Update now |
| `E_SERVER_UNAVAILABLE` | Backend/CDN returned HTTP ≥ 400 (`err.E_SERVER_UNAVAILABLE.{not_found,auth,rate_limited,server_error,body}`) | yes | Retry later |
| `E_TICKET_INVALID` | Ticket failed verification (clock/tamper) | yes | Retry · Update app |
| `E_PAYMENT_PENDING` / `E_PAYMENT_FAILED` / `E_PAYMENT_EXPIRED` | Checkout states | varies | Continue payment / Try again |
| `E_TEMPLATE_INVALID` | Template/schema validation failed | no | Fix template |
| `E_PREVIEW_CONFIG` | Preview bridge configuration or origin error (YouTube error 153 / bridge misconfiguration) | yes (auto-switch) | Auto-switch to timecode-only mode · Check app network/bridge config |
| `E_UNKNOWN` | Unexpected | yes | Retry · Export diagnostics |

> [!NOTE]
> A mode switch to timecode-only mode (e.g. when yt-dlp probe reports `playable_in_embed=false`, the bridge reports 101/150 after a control-video check, or the bridge handshake times out after 6 s) is a normal UI state, **not an error code**. `E_PREVIEW_CONFIG` is recorded strictly when YouTube emits Error 153 or the bridge configuration fails (triggering an automatic switch to timecode-only mode + telemetry). Members-only and age-restricted videos are rejected during initial probe (`E_YT_UNAVAILABLE`) and never reach preview.

### 8.1 YouTube error classification
1. **Probe JSON first:** inspect `availability` and `live_status`:
   - `is_upcoming` or ongoing live → `E_YT_LIVE_NOT_ENDED`
   - Explicit unavailable flags → `E_YT_UNAVAILABLE` with corresponding `reason` (`members_only`, `age_restricted`, `region_blocked`, `private`, `deleted`)
2. **Stderr patterns:**
   - Anti-bot verification ("Sign in to confirm you're not a bot") → `E_YT_BOT_CHECK`
   - HTTP 403 / 429 during download → silently re-probe once; if retry fails → `E_YT_RATE_LIMITED`
3. **Fallback:**
   - Any unrecognized extractor error → `E_YT_EXTRACT_FAILED`, saving raw output strictly to redacted local logs (never in user-facing UI or network telemetry).

### 8.2 Core Error Store, Preview Counters & Diagnostics Capture
- Every typed core error (`E_*`) and rate-limited WebView JS error is recorded locally in SQLite table `error_events` (30-day retention, max 500 newest rows), scrubbed by the shared redaction layer before storage.
- Preview instrumentation records last preview `mode`, `bridge_ready_ms`, `fallback_reason`, `yt_error_code`, and aggregate allowlisted counters (`embed_disabled_count`, `fallback_count`, `config_error_count`) in SQLite `settings` plus a dedicated `### Preview` section in local diagnostics reports.
- Users inspect recent errors, filter by `E_*` code, copy single error details from any shared error banner (`Salin detail error`), or generate a copy-pasteable local diagnostics report in **Diagnostik** (zero automatic network uploads).

## 9. Testing the progress system

- **ETA simulator:** feed recorded rate traces (steady, bursty, stalling, slow-start) into the estimator; assert monotonic percent, bounded ETA flapping, and error ≤ 25% at 30% progress on steady traces.
- **Contract tests:** each stage emits `stage change → progress… → success/failure` with heartbeats; events ≤ 4/s.
- **Fake sidecars:** scripts that print FFmpeg/whisper/yt-dlp style progress (and bad output) to test parsers, stalls and cancel.
- **Manual checklist:** disconnect network mid-download; kill FFmpeg mid-render; fill the disk; cancel after ticket; reboot during generate; verify every state shows a clear message and a next step.
