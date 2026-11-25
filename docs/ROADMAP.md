# Roadmap

Milestones are ordered by **dependency and risk**, not by dates. Do not start a milestone before the previous one's exit criteria are met (except where noted).

## 1. Overview

| M | Name | Outcome |
|---|------|---------|
| M0 | Spikes & foundations | Risky assumptions proven or replaced; repo, CI and environments exist |
| M1 | Core local pipeline | Link → range → generate → minimal editor → export, with full progress/ETA (no accounts) |
| M2 | Editor, templates, checklist | Campaign-aware editing and rule checking |
| M3 | Accounts, devices, credits | Server-enforced export tickets and entitlements |
| M4 | Payments | Midtrans pass purchase end-to-end + reconciliation |
| M5 | Hardening & release engineering | Signed installer, updater, component manifest, observability, security review |
| M6 | Closed beta → launch | 10–20 real clippers, fixes, go/no-go |
| M7+ | Post-MVP | Admin dashboard, packs, AI features, macOS, live capture |

## 2. M0 — Spikes and foundations

| Spike | Question | Output / decision |
|-------|----------|-------------------|
| **S1 Downloader (incl. S1b, S1c)** *(done)* | Does `yt-dlp --download-sections` give reliable progress and fast ranged downloads on the target network? Or is "resolve with yt-dlp, download with our FFmpeg" better? Is Deno needed/what size? QuickJS viable? Can yt-dlp failures be classified into private / members-only / age-restricted / region-blocked / deleted / premiere-not-started / ongoing-live? Can throughput be optimized beyond ~1.66×? What is the exact time-mapping accuracy of file t=0 vs `absoluteStartMs`? What explicit bitrate preset should preview proxy use? | **Done** (see `docs/benchmarks/S1-downloader.md`). Approach A copy mode + QuickJS-NG; probe/download split via `--load-info-json`; progress from FFmpeg `time=`; `absoluteStartMs = requestedStartMs` verified (0.00 ms relative acoustic error); proxy `h264_mf` at 3500k (`-g 30`); parallel split rejected (sequential baseline retained behind `RangeDownloader` trait); 5 VERIFIED / 5 ASSUMED fixtures (ASSUMED to be replaced in beta). |
| **S1d Chunking Proxy** *(optional, post-M1)* | Can a local range-chunking proxy allow FFmpeg to download sections through yt-dlp-style chunked upstream requests to achieve ~8× real-time instead of ~1.66×? | Prototype proxy, speedup measurements, complexity vs UX trade-off assessment |
| **S2 Preview** *(done)* | Does the official YouTube embed work inside WebView2? Does the HTTPS **bridge page** avoid Error 153-class failures? Is storyboard fallback good enough? | **Done** (see `docs/benchmarks/S2-preview.md`). Official YouTube embed through the hosted bridge (`https://clipper-bridge.vercel.app`, production Vercel deployment; move to owned domain before launch in M5) with timecode-only fallback mode (message, start/end timecode inputs with ±1/±5 s nudges, estimated download time). Direct embed inside WebView2 and loopback bridge NOT pursued for MVP. Storyboard scrubber moved to post-MVP backlog. Official loader is `youtube.com/iframe_api` (`youtube-nocookie.com/iframe_api` returns 404). Timing rules & ads detection established for M1 acceptance tests. Mode switch is not an error; `E_PREVIEW_CONFIG` added for YouTube error 153 / bridge misconfiguration. |
| **S3 Speech** *(done with open items)* | Which whisper.cpp build/model gives acceptable Indonesian quality and speed on a **representative mid-spec laptop**? Word-timestamp quality (DTW) and VAD? Is Vulkan worth it? | **Done with open items** (see `docs/benchmarks/S3-speech.md`). whisper.cpp v1.9.5 tag `b5454` (CPU AVX2); provisional default `small-q5_1` (base as Class C fallback); flags `-l id -dtw <canonical base> -nfa -sow -sns -ojf` (Rust core maps quantized names; flash attention disables DTW so `-nfa` mandatory; runtime self-check requires ≥95% DTW tokens; converter reads `t_dtw * 10` ms); threads = `min(6, physical cores)`; VAD OFF by default (measured word loss on gaming audio); 2 s context padding (filtered by center); phrase blocklist in `app_config`; ETA prior range 1.1–2.2× real-time (+ 8 s init); synthetic heartbeat every 2 s. Word timing is SUPPORTED by an automated unanchored check (n=338 words, 3 samples, median abs error 29 ms, mean bias +7 ms [95% CI +1.4, +13.0]). Open items: human timing audit (30 words/sample), negative control (shifted timestamps vs onset density), and either human-corrected references or the beta metric (caption edit rate, aggregate counts only). |
| **S4 Render** *(done)* | Which encoders work (NVENC/QSV/AMF/MF)? ASS burn-in correctness with bundled fonts? Export time for 60 s 1080×1920? | **Done** (see `docs/benchmarks/S4-render.md`). Encoder fallback chain `nvenc → qsv → amf → mf → E_ENCODER_UNAVAILABLE` (`libopenh264`/`x264` unapproved); AMF `vbr_peak` (2.73×, 97.69 VMAF, 4.4 Mbps) selected over `vbr_latency`; MF `pc_vbr` (3.58×, 91.37 VMAF) without `-profile:v`; `small_file` CBR 2.5 Mbps; software decode default (D3D11VA optional); `gblur` for `blur_fit`; Windows filter-path escaping; two-pass linear loudness (−14 LUFS default); mid-render cancellation cleanup and low-disk pre-flight guard verified. |
| **S5 Licensing** *(done)* | Which FFmpeg build (LGPL, libass present, no GPL libs) and how to ship notices/source offer? Does the build expose `h264_mf` and the hardware encoders? Is `libopenh264` included under acceptable license terms? | **Done** (see `docs/benchmarks/S5-ffmpeg.md`, `docs/compliance/FFMPEG.md`, `THIRD_PARTY_NOTICES.md`). Adopted BtbN release shared LGPLv3 (`n9.0.2`, SHA-256 `1fdb837…`, saves 55% download / 54% disk); `h264_mf` honors `-b:v`, `-g`, rate control; `boxblur` absent (GPL-only), replaced by LGPL `gblur`/`avgblur`; automated capability gate in CI (`verify-capabilities.py`). |
| **S5b Minimal LGPL Build** *(optional, post-M1)* | Can an in-house minimal LGPL build reduce package size (~30–40 MB vs 74 MB) and target pure LGPLv2.1+ (avoiding version3-only libraries while keeping libass and schannel TLS)? Is the CI maintenance cost justified? | S5 recommended adopting BtbN release shared LGPLv3 for MVP (zero CI maintenance; 74 MB is well within R2 budget); evaluate S5b post-M1 if binary footprint reduction or LGPLv2.1 purity is required. |
| **S6 Platform** | Supabase project (Singapore), Midtrans sandbox round-trip, CI Windows build, signing pipeline (dev cert) | **Scheduled right before M3** (not blocking M1). Environments + secrets layout + CI green. |
| **S7 Login** | Google PKCE deep-link flow on Windows; email OTP; CAPTCHA feasibility (Turnstile in webview vs hosted page) | **Scheduled right before M3** (not blocking M1). Auth design finalized. |

**Exit:** all spikes documented; defaults recorded; "Benchmark Gate" table filled for at least one mid-spec Windows laptop; repo scaffolding + CI + empty migrations committed.

## 3. Definition of Done (applies to every feature)

- [ ] Meets its requirement IDs in the PRD and the relevant doc sections
- [ ] Long operations emit progress events; ETA behaves per PROGRESS_AND_ETA
- [ ] Failure paths mapped to the error catalog with user text (id) and actions
- [ ] Unit/integration tests; security-critical items also have the tests in SECURITY §13
- [ ] No secrets, no PII in logs; logging redaction tested
- [ ] i18n keys (id + en); keyboard-accessible controls
- [ ] Docs updated; migration (if any) forward-only and tested
- [ ] Manually exercised on a Windows mid-spec machine

## 4. Test strategy

| Layer | What | Tooling |
|-------|------|---------|
| Unit (TS) | rules-engine, ETA estimator, composition math, schemas | Vitest |
| Unit (Rust) | URL parser, ticket verify, render-args builder, sidecar parsers | cargo test (+ snapshot tests) |
| Property/fuzz | template import, URL parsing, filename sanitizing | fast-check / cargo-fuzz |
| Integration | Job runner + fake sidecars; stage resume; outbox retry | Rust integration tests |
| DB | RLS, `rpc_begin_export` incl. concurrency, payment functions | pgTAP + parallel client scripts |
| Edge Functions | webhook fixtures (valid/forged/duplicate/out-of-order), `begin-export` | Deno tests |
| E2E | Windows runner: install → fake-auth → generate fixture → export → verify | Tauri WebDriver / scripted smoke |
| Load | `begin-export` and webhook endpoint | k6 (target: comfortably above projected peak ×10) |
| Golden render | Composition → FFmpeg args snapshots; ffprobe metadata of fixture output; assert no GPL-only filters used (denylist: `docs/benchmarks/gpl-filters-denylist.txt`) | Snapshot tests |
| Security | Checklist in SECURITY §13 | CI + manual |
| ETA accuracy | Recorded traces; real-world error stats | Simulator + telemetry |

Test fixtures: short clips we own or that are freely licensed. Never commit downloaded third-party videos.

## 5. M1 — Core local pipeline (no accounts)

**Scope & 9 Implementation Steps:**
1. **Scaffold:** Tauri 2 skeleton, typed IPC (`tauri-specta`), SQLite DB, base UI shell (Tailwind + React).
2. **Sidecar Manager + Component Installer + Hardware Defaults (M1.1d-lite fast-path to beta):** sidecar execution inside Windows Job Object; user-facing **Komponen** screen + disk-space check + DataRoot picker (`bootstrap.json`); passive RAM-aware speech default (`RAM ≥ 8 GB` → `small-q5_1`, else `base`) + lazy first-export encoder chain (`h264_nvenc → h264_qsv → h264_amf → h264_mf`) + conservative range ETA priors without an up-front benchmark or first-run wizard; local **Diagnostik** store (`error_events`), panic hook, rotating redacted logs, and offline copy/save diagnostics reports. *(Fast-path note: first-run onboarding wizard, up-front FFmpeg benchmark, accounts, and Midtrans payments are intentionally deferred so beta testers can run early and send diagnostics reports by copy-paste; accounts/payments ship in M3/M4, wizard/benchmark tuning restore in M2/M5).*
3. **URL Intake + Probe:** URL parsing/normalization, yt-dlp `-J` probe cache split (`--load-info-json`), error classification (`E_YT_*`), rejection of members-only/age-restricted content.
4. **Preview + Range Picker:** hosted bridge embed (`https://clipper-bridge.vercel.app`), postMessage handshake & timing extrapolation, range picker with nudges and ads guard notice, timecode-only fallback mode.
5. **Download Pipeline + Job Runner + Progress/ETA:** stage execution engine, cancellation/resume, ranged download via yt-dlp copy mode, FFmpeg `time=` progress parser, ETA estimator.
6. **Editor:** trim with in/out handles, frame nudges, fill layout (draggable crop), looped playback, preview proxy for non-AVC1 sources (`h264_mf`).
7. **Captions Operation:** separate operation on trimmed clip, whisper.cpp `small-q5_1` CPU AVX2 integration, DTW word timing (`timingSource: 'dtw'`), ASS generation.
8. **Render + Export with FakeTicketProvider:** `TicketProvider` interface with fake implementation (swappable in M3), filtergraph builder, hardware encoder chain (`nvenc` → `qsv` → `amf` → `mf`), ASS subtitle burn-in, two-pass linear loudness normalization (−14 LUFS), low-disk pre-flight guard.
9. **Sync + E2E Tests:** known marker fixture; preview and export alignment within ±1 frame; captions align with speech; full end-to-end pipeline verification.

**M1 Preview Acceptance Tests:**
- **Bridge Handshake & Readiness:** bridge iframe loads `https://clipper-bridge.vercel.app`, establishes postMessage handshake with origin validation, and emits `bridgeReady` within 6 s.
- **Timing Extrapolation:** host does not rely on a fixed 16 Hz update rate (measured 4–17 Hz depending on stream); host smoothly extrapolates display time (`time + elapsed × playbackRate`).
- **Seek Latency:** seek latency is measured from `seekTo` issuance until player state transitions to `PLAYING` after `BUFFERING`.
- **Timecode-Only Fallback Mode:** if yt-dlp probe reports `playable_in_embed=false`, or bridge reports error 101/150 (after control check), or bridge does not become ready within 6 s, UI switches cleanly to timecode-only mode with message *"Pratinjau tidak tersedia untuk video ini"*, start/end timecode inputs with ±1/±5 s nudges, and estimated download time.
- **Ads Detection & Range Guard:** if player reported duration differs from probe duration by > ±2 s, "set start/end here" buttons are disabled and a warning notice is displayed.
- **Configuration Error Trapping:** if bridge throws YouTube Error 153 (configuration/origin mismatch), log `E_PREVIEW_CONFIG`, auto-switch to timecode-only mode, and record aggregate telemetry (`preview_fallback{reason: 'preview_config'}`).

**Exit:** a mid-spec laptop goes link → range → generate → trim → captions → exported 9:16 captioned clip; every stage shows stage/progress/ETA; cancel/resume works (cancelling captions never blocks editing); failures show catalog messages; sync test passes (known marker in source; preview and export align within ±1 frame; captions align with speech).
**Test cases for caption flow & local pipeline:**
- First-run probe & benchmark: records which encoders actually work on the target machine, logs unsupported fallbacks, and seeds `stage_stats` with measured speeds.
- Trim → captions → export end-to-end.
- Shrink trim: existing captions remain valid without re-transcribing (words outside trimmed bounds hidden).
- Extend trim within transcribed coverage: captions remain valid.
- Extend trim beyond transcribed coverage: marks captions stale (`captions.status = 'stale'`) and offers re-run.
- Re-trim after editing words: user text/timing overrides survive re-trims.
- Multi-segment timeline: independent padded transcription (+ ~2 s padding per segment) mapped to single output timeline.

## 6. M2 — Editor, templates, checklist

Layouts (blur-fit, split), caption editing + styles, logo/text overlays, template CRUD + built-ins, rules engine + pre-export checklist, post-render verification, post-text copy, undo/redo + autosave + crash recovery, silence removal/cut (S), audio normalize (S), local-file source (S — promote to Must after S1 if downloader shows fragility, otherwise first post-MVP item; see PRD Q4).
**Exit:** a campaign template reproduces a rule-compliant clip; checklist blocks violations; preview matches export on golden fixtures.

## 7. M3 — Accounts, devices, credits

**Prerequisite:** S7 decision recorded (auth flow incl. CAPTCHA approach).

Auth (Google PKCE + OTP), device registration/limits, `get_account_status`, `begin-export` + `rpc_begin_export`, ticket signing/verification, entitlements, per-device free caps, caps/limits from `app_config`, outbox for finalize, min-version gate, credit UI (indicator, confirm dialog, out-of-credits). `admin_grant_plan` available.
**Exit:** all credit/ticket/RLS tests in SECURITY §13 pass; concurrency test proves no over-spend; a manually granted pass works in the app.

## 8. M4 — Payments

`create-checkout`, Midtrans sandbox flow, `midtrans-webhook` (signature + Status API + idempotent apply), `reconcile-orders`, order states in UI, finish page, purchase history (S), refund/chargeback handling, then **production keys** after legal texts are ready.
**Exit:** all payment tests pass; a real small transaction in production verified end-to-end; reconciliation alerts tested.

## 9. M5 — Hardening and release engineering

Authenticode signing, NSIS/MSI installer, Tauri updater + channels, signed component manifest + rollout/rollback, Sentry (opt-in), telemetry (opt-in) + consent screen, diagnostics bundle, SBOM + in-app "About/Licenses" screen (displaying `THIRD_PARTY_NOTICES.md` and user replacement instructions for FFmpeg), Cloudflare R2 permanent source mirror (FFmpeg exact source tarball + build info per LGPLv3), migrate YouTube embed bridge from `https://clipper-bridge.vercel.app` to an owned domain (`player.<domain>`) with production-only `frame-ancestors` before launch, rate limits + alerts, load tests, runbooks, security review (second reviewer), privacy policy/terms/refund pages.
**Exit:** SECURITY §15 checklist complete; clean-machine install/upgrade/rollback tested.

## 10. M6 — Closed beta and launch

10–20 clippers (include the mid-spec laptop users), feedback channel (chat group), weekly build cadence. Track: activation, time-to-first-export, export success, ETA accuracy, crashes, downloader failure rate, support questions. Use beta to settle pass price/daily credits (Q1). **Go/no-go:** crash-free sessions and export success meet agreed thresholds; no open security findings; payment flow verified; legal texts live.

## 11. Post-MVP backlog (ordered by expected value)

1. Admin dashboard (ADMIN_DASHBOARD.md) · 2. Credit packs (`pool='bonus'`) · 3. Local-file source (if not shipped) · 4. AI highlight suggestions (server-side or local) as a paid differentiator · 5. Face-aware reframe (local ONNX) · 6. Live capture ("clip last 60 s") · 7. macOS build · 8. Template marketplace/community sharing · 9. Campaign submission/earnings tracker · 10. Storyboard-thumbnail scrubber for embed-disabled videos (PRD §10).

## 12. Open questions log

Add items here instead of guessing (see AGENTS.md §4). Initial list: PRD §11 (Q1–Q6); S1–S7 outcomes; whether to ship QuickJS instead of Deno (resolved in S1: QuickJS-NG chosen); x264 legal decision; final pass price/credits; support channel and SLA.
