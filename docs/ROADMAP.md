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
| **S2 Preview** | Does the official YouTube embed work inside WebView2? Does the HTTPS **bridge page** avoid Error 153-class failures? Is storyboard fallback good enough? | Preview strategy confirmed; bridge page spec; fallback UX |
| **S3 Speech** | Which whisper.cpp build/model gives acceptable Indonesian quality and speed on a **representative mid-spec laptop**? Word-timestamp quality (DTW) and VAD? Is Vulkan worth it? | Default model + backend, rate priors, accuracy notes in `docs/benchmarks/` |
| **S4 Render** | Which encoders work (NVENC/QSV/AMF/MF)? ASS burn-in correctness with bundled fonts? Export time for 60 s 1080×1920? | Encoder priority list, render preset, time priors |
| **S5 Licensing** | Which FFmpeg build (LGPL, libass present, no GPL libs) and how to ship notices/source offer? Does the build expose `h264_mf` and the hardware encoders? Is `libopenh264` included under acceptable license terms? | Recorded decision in ARCHITECTURE §15; binary inventory; `ffmpeg -encoders` output saved in `docs/benchmarks/` |
| **S6 Platform** | Supabase project (Singapore), Midtrans sandbox round-trip, CI Windows build, signing pipeline (dev cert) | Environments + secrets layout + CI green |
| **S7 Login** | Google PKCE deep-link flow on Windows; email OTP; CAPTCHA feasibility (Turnstile in webview vs hosted page) | Auth design finalized |

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
| Golden render | Composition → FFmpeg args snapshots; ffprobe metadata of fixture output | Snapshot tests |
| Security | Checklist in SECURITY §13 | CI + manual |
| ETA accuracy | Recorded traces; real-world error stats | Simulator + telemetry |

Test fixtures: short clips we own or that are freely licensed. Never commit downloaded third-party videos.

## 5. M1 — Core local pipeline (no accounts)

Scope: Tauri skeleton, typed IPC, SQLite, job runner (stages, cache, resume, cancel), hardware probe + first-run wizard, component installer (dev manifest), URL intake + probe, preview (per S2), range picker, generate pipeline, minimal editor (trim, fill layout, captions auto, no templates), render + verify, full progress/ETA, error catalog plumbing. Use a **`TicketProvider` interface** with a fake implementation so M3 can swap in the real one without touching the render code.
**Exit:** a mid-spec laptop goes link → exported 9:16 captioned clip; every stage shows stage/progress/ETA; cancel/resume works; failures show catalog messages; sync test passes (known marker in source; preview and export align within ±1 frame; captions align with speech).

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

Authenticode signing, NSIS/MSI installer, Tauri updater + channels, signed component manifest + rollout/rollback, Sentry (opt-in), telemetry (opt-in) + consent screen, diagnostics bundle, SBOM + license notices screen, rate limits + alerts, load tests, runbooks, security review (second reviewer), privacy policy/terms/refund pages.
**Exit:** SECURITY §15 checklist complete; clean-machine install/upgrade/rollback tested.

## 10. M6 — Closed beta and launch

10–20 clippers (include the mid-spec laptop users), feedback channel (chat group), weekly build cadence. Track: activation, time-to-first-export, export success, ETA accuracy, crashes, downloader failure rate, support questions. Use beta to settle pass price/daily credits (Q1). **Go/no-go:** crash-free sessions and export success meet agreed thresholds; no open security findings; payment flow verified; legal texts live.

## 11. Post-MVP backlog (ordered by expected value)

1. Admin dashboard (ADMIN_DASHBOARD.md) · 2. Credit packs (`pool='bonus'`) · 3. Local-file source (if not shipped) · 4. AI highlight suggestions (server-side or local) as a paid differentiator · 5. Face-aware reframe (local ONNX) · 6. Live capture ("clip last 60 s") · 7. macOS build · 8. Template marketplace/community sharing · 9. Campaign submission/earnings tracker.

## 12. Open questions log

Add items here instead of guessing (see AGENTS.md §4). Initial list: PRD §11 (Q1–Q6); S1–S7 outcomes; whether to ship QuickJS instead of Deno (resolved in S1: QuickJS-NG chosen); x264 legal decision; final pass price/credits; support channel and SLA.
