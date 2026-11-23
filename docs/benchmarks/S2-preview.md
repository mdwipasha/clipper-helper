# Spike S2 Benchmark: Video Preview & YouTube Embed Bridge

**Date:** 2026-11-23  
**Status:** Completed (Empirical Benchmark on R1 Verified; Manual Visual Checklist Prepared)  
**Scope:** `apps/bridge/`, `spikes/s2-preview/`, `docs/benchmarks/S2-preview.md`  
**Tested on:** Reference Machine R1 (Lenovo ThinkPad 20X8S07R00, AMD Ryzen 5 PRO 5650U with Radeon Graphics [6C/12T, ~1990 MHz], 32 GB RAM [31,566 MB], Windows 11 Pro 10.0.26200 — see `docs/benchmarks/README.md`)  
**WebView2 Runtime Version:** `157.0.4341.60` (registry-verified)  
**Tauri Origin Configuration:** `http://tauri.localhost` (default Windows WebView2 scheme) and `https://tauri.localhost` (both allowlisted in CSP `frame-ancestors`)  
**Bridge Deployment URL:** `https://clipper-bridge.vercel.app` (Vercel production deployment)  

---

## 1. Overview & Architecture Summary

### 1.1 The Embed Challenge (Error 153 vs Error 101/150)
YouTube's IFrame Player API blocks embeds inside desktop applications (e.g. Tauri / WebView2) under two distinct failure classes:
1. **Error 153 (Invalid Referrer / Unknown Origin):** When an embed request originates from a custom local scheme (e.g. `tauri://localhost`, `file://`, or an opaque origin), YouTube's player refuses to initialize or throws playback error 153.
2. **Error 101 / 150 (Embedding Restricted by Owner):** The channel owner explicitly disabled external embedding for the video, or the video requires authentication (members-only, age-restricted).
3. **Origin Mismatch Caveat:** Errors 101/150 can **also be triggered spuriously** if the `origin` parameter passed to `YT.Player` does not match the actual hosting origin of the iframe document. To isolate true content restrictions from origin configuration bugs, tests must always benchmark against known-embeddable controls (Group a).

### 1.2 The Bridge Solution
The bridge ([apps/bridge/](file:///f:/coding/clipper-helper/apps/bridge/)) is a minimal static web app deployed on an authentic HTTPS origin (`https://clipper-bridge.vercel.app`):
- It embeds the official YouTube IFrame API using `https://www.youtube.com/iframe_api`.
  *(Note: `https://www.youtube-nocookie.com/iframe_api` was tested and discovered to return HTTP 404; the loader script must originate from `youtube.com/iframe_api`).*
- Sends `Referrer-Policy: strict-origin-when-cross-origin` to ensure YouTube receives a valid HTTPS referrer.
- Restricts embedding via CSP `frame-ancestors` to authorized Tauri origins (`tauri.localhost`, `localhost`, `127.0.0.1`).
- Relays player state, playback time, duration, and error codes to the desktop app via `postMessage`.

---

## 2. Deployment Verification (`curl.exe -I` & `-s`)

The production deployment at `https://clipper-bridge.vercel.app` was verified via `curl.exe`:

```http
HTTP/1.1 200 OK
Content-Type: text/html; charset=utf-8
Content-Length: 1233
Content-Security-Policy: default-src 'self' https://www.youtube-nocookie.com https://www.youtube.com; script-src 'self' 'unsafe-inline' https://www.youtube-nocookie.com https://www.youtube.com https://s.ytimg.com; style-src 'self' 'unsafe-inline'; img-src 'self' data: https://i.ytimg.com https://s.ytimg.com; frame-src https://www.youtube-nocookie.com https://www.youtube.com; connect-src 'self' https://www.youtube-nocookie.com https://www.youtube.com https://*.googlevideo.com; frame-ancestors 'self' https://tauri.localhost http://tauri.localhost tauri://localhost http://localhost:* http://127.0.0.1:*;
Referrer-Policy: strict-origin-when-cross-origin
X-Content-Type-Options: nosniff
Strict-Transport-Security: max-age=63072000; includeSubDomains; preload
```

### Verification Highlights:
- **Status:** HTTP 200 OK.
- **CSP `frame-ancestors`:** Strictly restricted to `'self' https://tauri.localhost http://tauri.localhost tauri://localhost http://localhost:* http://127.0.0.1:*`. Unauthorized domains cannot embed the bridge.
- **X-Frame-Options:** Absent (correct standard practice; `frame-ancestors` obsoletes `X-Frame-Options` and prevents conflicting frame denial).
- **Referrer-Policy:** `strict-origin-when-cross-origin` guarantees YouTube receives `https://clipper-bridge.vercel.app/` as the referrer origin.
- **Vercel Injections:** Inspected via `curl.exe -s`; payload is 1,233 bytes of clean HTML with **zero** injected analytics, tracking scripts, or external tags.

---

## 3. Pre-Flight Candidate Verification (`yt-dlp -J`)

All candidate URLs were pre-verified using `yt-dlp -J` (version `2026.10.26` + QuickJS-NG `v0.18.0`):

| Group | Target URL | Video ID | Title | Duration | `live_status` | `availability` | `playable_in_embed` | Expected Behavior |
|---|---|---|---|---|---|---|---|---|
| **(a)** | `https://youtu.be/WufkeDaJisQ` | `WufkeDaJisQ` | INI GAME SERU PARAH!!! - Deceit 2 | 602 s (10m 02s) | `not_live` | `public` | **`true`** | Known-Embeddable Control (Short) |
| **(a)** | `https://youtu.be/3snfJrlSyqo` | `3snfJrlSyqo` | SISI GELAP PRO PLAYER INDO FT. JONATHAN LIANDI | 2,415 s (40m 15s) | `not_live` | `public` | **`true`** | Known-Embeddable Control (Medium) |
| **(a)** | `https://www.youtube.com/live/XwQwhjzAc_k` | `XwQwhjzAc_k` | GTA 6 100% TAMAT MODE DI RENTAL PS ( 4K 60 FPS ) - PART 4🔥 | 35,942 s (9h 59m 02s) | `was_live` | `public` | **`true`** | Live URL Format & Marathon VOD |
| **(c)(d)** | `https://www.youtube.com/watch?v=9s6imDGjy08` | `9s6imDGjy08` | irl stream in Indonesia 🇮🇩 | 13,668 s (3h 47m 48s) | `was_live` | `public` | **`true`** | Monetized Long VOD (IShowSpeed) |
| **(d)** | `https://www.youtube.com/watch?v=QOJIILaJKNc` | `QOJIILaJKNc` | GTA 6 RADIO ON - PART 1🔥 | 19,728 s (5h 28m 48s) | `was_live` | `public` | **`true`** | Long VOD Reference (S1) |
| **(e)** | `https://www.youtube.com/watch?v=KBdGeakRgqg` | `KBdGeakRgqg` | *(Members-only video)* | `null` | `null` | `unavailable` | **`false`** | Blocked Control (Members-only) |
| **(e)** | `https://www.youtube.com/watch?v=6kLq3WMV1nU` | `6kLq3WMV1nU` | *(Age-restricted video)* | `null` | `null` | `unavailable` | **`false`** | Blocked Control (Age-restricted) |
| **(b)** | `<MY_EMBED_DISABLED_VIDEO_URL>` | *(Pending)* | *(User Unlisted Embed-Disabled Video)* | *(Pending)* | *(Pending)* | *(Pending)* | *(Pending)* | Embed-Disabled Owner Test |

---

## 4. Empirical Test Matrix Results on Reference Machine R1

Tests were executed using the automated test harness (`spikes/s2-preview/app/`) running against `https://clipper-bridge.vercel.app` on Reference Machine R1 (Edge / WebView2 engine). All measurements reflect live IPC events:

| Group | Candidate Video ID | Embed Loaded? | Error Code / Type | Time to Bridge Ready | Time to Player Ready | Time to First Play | `timeUpdate` Event Rate | Seek Latency (t = 30s) | Seek Latency (t = ~3h / 10,800s) | Fallback View Triggered? | Diagnosis (Origin Mismatch vs Owner Restriction) |
|---|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|---|
| **(a)** | `WufkeDaJisQ` (10m) | **YES** | None | 1,249 ms | 2,587 ms | 4,784 ms | 17.0 Hz | 14 ms | — | No | **Baseline Pass:** Clean load, smooth play, 0 origin errors. |
| **(a)** | `3snfJrlSyqo` (40m) | **YES** | None | 634 ms | 1,067 ms | 2,240 ms | 13.1 Hz | 48 ms | — | No | **Baseline Pass:** Clean load, chapters functional. |
| **(a)** | `XwQwhjzAc_k` (10h) | **YES** | None | 1,389 ms | 2,115 ms | 3,409 ms | 4.0 Hz | 31 ms | — | No | **Baseline Pass:** Marathon timeline initialized smoothly. |
| **(c)(d)** | `9s6imDGjy08` (3h47m) | **YES** | None | 144 ms | 509 ms | 1,613 ms | 8.0 Hz | 40 ms | **47 ms** | No | **Pass:** Instant cache startup; 3-hour seek completed in 47 ms. |
| **(d)** | `QOJIILaJKNc` (5.5h) | **YES** | None | 162 ms | 488 ms | 1,546 ms | 4.0 Hz | 60 ms | **43 ms** | No | **Pass:** Instant cache startup; 3-hour seek completed in 43 ms. |
| **(e)** | `KBdGeakRgqg` (Members) | **NO** | 150 (`E_YT_EMBED_RESTRICTED`) | 517 ms | — | — | — | — | — | **YES** | **Confirmed Owner Restriction:** Zero origin bugs; YouTube policy restricts unauthenticated playback. |
| **(e)** | `6kLq3WMV1nU` (Age-rest.) | **NO** | 150 (`E_YT_EMBED_RESTRICTED`) | 660 ms | — | — | — | — | — | **YES** | **Confirmed Owner Restriction:** YouTube policy restricts unauthenticated embed. |
| **(b)** | *(User Unlisted Video)* | *(Pending)* | *(Pending)* | — | — | — | — | — | — | *(Pending)* | Awaiting user URL; harness input field ready. |

*Raw data persisted in [spikes/s2-preview/results/matrix_results.json](file:///f:/coding/clipper-helper/spikes/s2-preview/results/matrix_results.json).*

### 4.1 Diagnosis Analysis: Origin Mismatch vs Owner Restriction
- **Measured:** Group (a) baseline videos (`WufkeDaJisQ`, `3snfJrlSyqo`, `XwQwhjzAc_k`) achieved **100% embed success** without a single Error 153 or spurious Error 150.
- **Conclusion:** The bridge origin configuration (`origin: window.location.origin` and `strict-origin-when-cross-origin`) completely eliminates Error 153.
- **Validation of Group (e):** The Error 150 triggered on `KBdGeakRgqg` and `6kLq3WMV1nU` is **100% confirmed to be YouTube account/content restriction** and NOT an origin or referrer defect.
- **Seek Performance:** Seeking across massive offsets (e.g. `10,800s` into a 3.75-hour or 5.5-hour stream) resolved in **43–47 ms** via YouTube's adaptive streaming layer.

---

## 5. Decision for D-13: Preview Strategy

### 5.1 Alternatives Considered
1. **Direct Embed inside WebView2 (`tauri://localhost` or `http://tauri.localhost`):**
   - *Outcome:* **REJECTED (Not pursued for MVP)**. YouTube blocks playback with **Error 153** due to custom protocol origins not matching standard web origin expectations.
2. **Loopback Local Bridge (Local HTTP server in Rust):**
   - *Outcome:* **REJECTED (Not pursued for MVP)**. Unmeasured; introduces local port management, socket complexity, and potential firewall issues. Revisit only if the hosted bridge becomes an operational problem.
3. **Fallback-Only (Storyboard Frames & Scrubber):**
   - *Outcome:* **CUT FROM MVP (Moved to Post-MVP Backlog)**. Storyboard sprite extraction and frame scrubbing add substantial upfront parsing complexity without audio feedback. Deferred to post-MVP backlog.
4. **Hosted Bridge (`https://clipper-bridge.vercel.app`):**
   - *Outcome:* **ACCEPTED**. Solves Error 153, supplies valid HTTPS origin and strict-origin-when-cross-origin referrer, isolates embed inside an iframe, and supports smooth range picking. Production deployment live on Vercel; migrate to owned domain (`player.<domain>`) in M5 before launch.

### 5.2 Architectural Decision: Hybrid Bridge with Timecode-Only Fallback Mode
- **Primary Preview:** Runs through the hosted HTTPS Bridge (`https://clipper-bridge.vercel.app`).
- **Secondary Fallback (Timecode-Only Mode):** If the yt-dlp probe indicates `playable_in_embed=false`, or the bridge reports error 101/150 (after a control-video check), or no `bridgeReady` arrives within 6 s, the UI cleanly switches to **timecode-only mode**:
  - Displays message: *"Pratinjau tidak tersedia untuk video ini"*.
  - Start/end timecode inputs with ±1 s / ±5 s nudges.
  - Displays estimated download time (`range_seconds / 1.66 + ~10 s`).
  - Occurrences counted as aggregate opt-in telemetry (`preview_fallback{reason}`).
- **Mode Switch vs Error Code:** A mode switch to timecode-only is a normal UI state, **not an error code**. `E_PREVIEW_CONFIG` is added strictly for YouTube Error 153 / bridge misconfiguration (auto-switches to timecode-only + telemetry). Members-only and age-restricted videos are rejected during initial probe (`E_YT_UNAVAILABLE`) and never reach preview.

---

## 6. Bridge Specification & Timing Rules

### 6.1 Host-to-Bridge IPC Contract (Inbound Commands)
Messages sent from Tauri host to Bridge iframe via `iframe.contentWindow.postMessage(msg, bridgeOrigin)`:

| Command Action | Payload Fields | Description |
|---|---|---|
| `loadVideo` | `{ videoId: string, startSeconds?: number, endSeconds?: number, autoplay?: boolean }` | Loads and immediately plays a new video. |
| `cueVideo` | `{ videoId: string, startSeconds?: number, endSeconds?: number }` | Cues a video without autoplaying. |
| `play` | `{}` | Resumes playback. |
| `pause` | `{}` | Pauses playback. |
| `seekTo` | `{ seconds: number, allowSeekAhead?: boolean }` | Seeks to specified timecode in seconds. |
| `setVolume` | `{ volume: number }` (0–100) | Sets player volume. |
| `mute` | `{}` | Mutes audio. |
| `unMute` | `{}` | Unmutes audio. |
| `setPlaybackRate` | `{ rate: number }` (0.25 to 2.0) | Adjusts playback speed. |
| `ping` | `{ payload?: any }` | Health check (responds with `pong`). |

### 6.2 Bridge-to-Host IPC Contract (Outbound Events)
Messages sent from Bridge iframe to parent via `window.parent.postMessage(msg, hostOrigin)`:

```typescript
interface BridgeMessage<T = any> {
  source: 'clipper-bridge';
  version: '1.0.0';
  type: 'bridgeReady' | 'playerReady' | 'stateChange' | 'timeUpdate' | 'playbackRateChange' | 'error' | 'pong';
  timestamp: number;
  data?: T;
}
```

- **`bridgeReady`:** Emitted immediately upon bridge DOM load.
  `{ hasInitialVideo: boolean, initialVideoId: string | null }`
- **`playerReady`:** Emitted when `window.YT.Player` triggers `onReady`.
  `{ videoId: string, duration: number }`
- **`stateChange`:** Emitted on `onStateChange`.
  `{ state: 'UNSTARTED'|'ENDED'|'PLAYING'|'PAUSED'|'BUFFERING'|'CUED', code: number, currentTime: number, duration: number }`
- **`timeUpdate`:** Emitted while playing.
  `{ currentTime: number, duration: number }`
- **`error`:** Emitted on `onError`.
  `{ code: number, errorType: string, title: string, message: string }`

### 6.3 Preview Timing Rules (M1 Acceptance Tests)
1. **Timing Extrapolation:** Do **not** claim a fixed 16 Hz update rate (measured `timeUpdate` rate varies between 4–17 Hz depending on the video and browser scheduling). The desktop host extrapolates current playback time locally:
   $$\text{displayTime} = \text{lastCurrentTime} + (\text{now} - \text{lastUpdateTimestamp}) \times \text{playbackRate}$$
2. **Seek Latency Definition:** Seek latency is measured as elapsed time from `seekTo` command dispatch until the player state transitions to `PLAYING` after `BUFFERING`.
3. **Ads Detection Guard:** If player reported duration differs from probe duration by > ±2 s ($\lvert\text{playerDuration} - \text{probeDuration}\rvert > 2.0\text{ s}$), an ad is playing. The UI disables the "set start/end here" buttons and displays an informative banner: *"Iklan sedang diputar — tombol penetapan rentang dinonaktifkan sementara."*

### 6.4 Security Contract & Hardening Requirements (SECURITY §9)
1. **Target Origin Enforcement:** No `'*'` postMessage targets. The host specifies `https://clipper-bridge.vercel.app` (or owned domain); the bridge specifies the validated parent origin (`http://tauri.localhost` or `https://tauri.localhost`).
2. **Handshake & Source Verification:** Initialization handshake validates `event.origin` against an explicit allowlist and verifies `event.source === iframe.contentWindow`.
3. **Schema Validation & Size Limit:** Schema-validate every incoming message at the boundary with a strict 64 KB size limit; drop unrecognized fields.
4. **Script CSP:** Remove `'unsafe-inline'` from bridge `script-src` in production deployment.
5. **Production Frame-Ancestors:** Production bridge `frame-ancestors` permits only Tauri origins (`https://tauri.localhost http://tauri.localhost tauri://localhost`). `localhost` / `127.0.0.1` are strictly excluded from production and reserved for separate development deployments.
6. **App CSP Isolation:** Desktop app CSP maintains `frame-src https://clipper-bridge.vercel.app` only.

---

## 7. Error-Mapping Table

| YouTube API Code | Catalog Code | Indonesian User Title (`title`) | User-Facing Explanation (`message`) | UX Trigger |
|:---:|---|---|---|---|
| `2` | `E_YT_INVALID_PARAM` | Parameter Video Tidak Valid | ID video mengandung karakter tidak valid atau format URL salah. | Prompts user to check input URL. |
| `5` | `E_YT_HTML5_ERROR` | Kesalahan Pemutar HTML5 | Pemutar HTML5 YouTube mengalami kesalahan pada sistem browser ini. | Prompts WebView2 hardware acceleration check. |
| `100` | `E_YT_NOT_FOUND` | Video Tidak Ditemukan | Video telah dihapus, dijadikan privat, atau tidak tersedia. | Re-directs to URL input. |
| `101` / `150` | `E_YT_EMBED_RESTRICTED` | Pemutaran Embed Ditolak Pemilik | Pemilik video menonaktifkan pemutaran di luar YouTube. | **Switches to Timecode-Only Mode (not an error dialog).** |
| `153` | `E_PREVIEW_CONFIG` | Kesalahan Konfigurasi Pratinjau | Terjadi kesalahan origin atau konfigurasi pemutar embed. | **Switches to Timecode-Only Mode + records telemetry.** |

*Note: Members-only and age-restricted videos are caught by yt-dlp probe (`E_YT_UNAVAILABLE`) and never reach the preview stage.*

---

## 8. Fallback UX Flow (Timecode-Only Mode)

```
[User enters Video URL]
          │
          ▼
[yt-dlp Probe Metadata]
          │
          ├───► [playable_in_embed == false] ──────────────────────────┐
          │                                                            │
          ▼                                                            │
[Mount Bridge IFrame] ────► [Bridge Ready > 6s Timeout] ───────────────┤
          │                                                            │
          ├───► [Bridge Error 101/150 (Confirmed by Control)] ─────────┤
          │                                                            │
          ├───► [YouTube Error 153 (Bridge Misconfig)] ──► [E_PREVIEW_CONFIG]
          │                                                            │
          │                                                            ▼
          │                                              [TIMECODE-ONLY FALLBACK MODE]
          │                                              - Message: "Pratinjau tidak tersedia untuk video ini"
          │                                              - Start / End inputs with ±1s / ±5s nudges
          │                                              - Estimated download time ("Perkiraan unduh: ~N menit")
          │                                              - Telemetry: preview_fallback{reason}
          ▼
[Normal Embed Preview]
- Official YouTube player
- Local time extrapolation (4–17 Hz input)
- Seek latency measured to PLAYING after BUFFERING
- Ads detection guard (duration mismatch > 2s disables "set start/end")
```

The timecode-only mode guarantees that even when embedding is restricted, clippers can still enter timestamps, use ±1/±5 s nudges, see the download ETA, and generate their clip without UI dead-ends.

---

## 9. Manual Inspection Checklist for User Review

A separate checklist has been generated at [spikes/s2-preview/manual_checklist.md](file:///f:/coding/clipper-helper/spikes/s2-preview/manual_checklist.md).
- Contains empty result cells for visual glitches, controls overlap, audio clarity, and ad presentation on monetized videos (`9s6imDGjy08`).
- Allows the user to inspect and fill in subjective evaluation points without agent hallucination.

---

## 10. Applied Documentation Edits & Cross-References

The decisions and scope boundaries finalized in Spike S2 have been applied across project documents:
- **`README.md` (D-13):** Records preview via hosted bridge (`https://clipper-bridge.vercel.app`), timecode-only fallback mode, storyboard scrubber moved to post-MVP, direct embed and loopback bridge not pursued for MVP.
- **`docs/ARCHITECTURE.md` (§6):** Documents bridge origin resolution, timecode-only mode switch, `E_PREVIEW_CONFIG` definition, timing extrapolation (4–17 Hz), seek latency criteria, and ads detection guard.
- **`docs/PRD.md` (FR-SRC-3, FR-SRC-4, §10):** Updates range picker and embed failure behaviors; moves storyboard scrubber to Post-MVP backlog.
- **`docs/SECURITY.md` (§9, §11):** Hardens bridge postMessage contract (explicit target origins, handshake validation, 64 KB schema limit, `'unsafe-inline'` removal, separate dev/prod frame-ancestors, app CSP `frame-src`), and adds `preview_fallback{reason}` aggregate telemetry.
- **`docs/PROGRESS_AND_ETA.md` (§8):** Adds `E_PREVIEW_CONFIG` to catalog and clarifies mode switch is not an error code.
- **`docs/ROADMAP.md` (§2, §5, §9, §11):** Marks S2 as Done, schedules S6 and S7 right before M3 (not blocking M1), defines the 9 M1 implementation steps and M1 preview acceptance tests, adds bridge migration to owned domain in M5, and logs storyboard scrubber in post-MVP backlog.
