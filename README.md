# Clipper Helper (working title)

A Windows desktop app that turns a YouTube livestream VOD into a campaign-ready vertical (9:16) clip in minutes.

> Paste a VOD link → preview → pick a time range → **Generate** (download + transcript + captions) → edit with a campaign template → pass the campaign checklist → **Export**.

Built for clippers who take part in clipping campaigns (Motionklip-style programs). Indonesian-first: UI language, IDR pricing, local payment methods.

## Status

Pre-implementation. This folder is the source of truth for product, architecture, data and security.
When code and docs disagree, fix one of them **in the same PR**.

## Getting started (Windows 10/11 x64)

### Prerequisites
- **Node.js**: v20+ LTS or v22+
- **pnpm**: v9+ or v11+ (`npm install -g pnpm`)
- **Rust**: stable MSVC toolchain (`rustup default stable-x86_64-pc-windows-msvc`) with components `rustfmt`, `clippy` and Visual Studio C++ Build Tools
- **WebView2**: pre-installed on Windows 10 (1803+) and Windows 11

### Setup & Development
```powershell
# 1. Install workspace dependencies
pnpm install

# 2. Run the desktop app in development mode (launches Vite + Tauri 2 window)
pnpm tauri dev

# 3. Run all workspace TypeScript unit tests & security guard
pnpm test

# 4. Run Rust unit tests and clippy
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings

# 5. Run typecheck and linting
pnpm run typecheck
pnpm run lint
```

## Read order

| # | File | What it answers |
|---|------|-----------------|
| 1 | [AGENTS.md](AGENTS.md) | Rules every AI agent / contributor must follow |
| 2 | [docs/PRD.md](docs/PRD.md) | What we build, for whom, what is in/out of scope |
| 3 | [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | How the system is built and why; scalability; licensing |
| 4 | [docs/DATA_MODEL.md](docs/DATA_MODEL.md) | Server schema, RLS, SQL functions, local SQLite, JSON schemas |
| 5 | [docs/SECURITY.md](docs/SECURITY.md) | Threat model, credit/payment security, hardening, test cases |
| 6 | [docs/PROGRESS_AND_ETA.md](docs/PROGRESS_AND_ETA.md) | Progress + time-estimate UX contract, error catalog |
| 7 | [docs/ROADMAP.md](docs/ROADMAP.md) | Milestones, spikes, definition of done, test strategy |
| 8 | [docs/ADMIN_DASHBOARD.md](docs/ADMIN_DASHBOARD.md) | Future web dashboard + what to prepare now |

## Decision log

| ID | Decision | Why |
|----|----------|-----|
| D-01 | Desktop-first, **Windows 10/11 x64 only** for MVP | Processing runs on the user's machine → near-zero compute cost, no uploading of multi-GB files. macOS later (see ARCHITECTURE §19). |
| D-02 | **Tauri 2** shell, React + TypeScript + Tailwind UI, **Rust core**, sidecar binaries | Small installer, same web stack the team already uses, security-critical logic stays in compiled Rust. |
| D-03 | **YouTube VOD only** for MVP, behind a `SourceProvider` interface | Where most livestreams are watched today; interface keeps other platforms cheap to add. |
| D-04 | **Credits are spent at export only.** Generate, editing and re-export are free | Matches user value: you pay for a finished clip. |
| D-05 | Free and paid export quality is **identical**; tiers differ only in **daily credits** (free = 3/day) | Product decision. AI features may become paid differentiators later. |
| D-06 | Paid tier = **30-day pass, one-time payment** (no auto-renew) via **Midtrans**; credit packs later | Avoids recurring-billing complexity and fits local payment habits. |
| D-07 | Daily credits reset **00:00 WIB**, no rollover; computed from the ledger, no cron | Simple, race-free, config-driven. |
| D-08 | Free re-export via an **entitlement** bound to source video + time segments (± tolerance) | Honors "re-edit is free" without letting one credit cover many different clips. |
| D-09 | **No automatic refunds.** Failed/cancelled exports are retried free via the entitlement; admins can adjust | Removes refund-abuse paths. |
| D-10 | **Supabase (Singapore)** is authoritative for auth, plans, credits, payments. App verifies **signed export tickets** | Server owns money; client never trusted. |
| D-11 | **LGPLv3 BtbN shared FFmpeg build + hardware encoders only** (`h264_nvenc` → `h264_qsv` → `h264_amf` → `h264_mf`); `libopenh264` and `x264` unapproved | Licensing safety and patent caution (see ARCHITECTURE §15, docs/compliance/FFMPEG.md). |
| D-12 | **yt-dlp + QuickJS-NG JS runtime** as downloader, updated independently via a signed manifest | YouTube breaks extractors often; the downloader must be hot-updatable. QuickJS-NG (MIT, 1.87 MB) chosen in S1; Deno may ship later as a fallback via signed manifest. |
| D-13 | Preview via **official YouTube embed through the hosted bridge** (`https://clipper-bridge.vercel.app`, production Vercel deployment; move to owned domain before launch in M5) with **timecode-only fallback mode**; direct embed and loopback bridge NOT pursued for MVP; storyboard scrubber moved to post-MVP; verified in spike S2 | Compliant preview without downloading the full VOD. |
| D-14 | A versioned **Composition JSON** is the single source of truth for preview and render | Guarantees preview == export; testable. |
| D-15 | Every long operation must show **stage, progress and time estimate** | Product requirement; see PROGRESS_AND_ETA.md. |
| D-16 | Admin web dashboard later; until then **manual plan grants via an SQL function** | Lets friends/beta users get the paid tier immediately. |
| D-17 | Docs and code in English; UI **Indonesian-first**, i18n-ready | Agents work better in English; users are Indonesian. |

## Target repo layout

```
clipper-helper/
├─ AGENTS.md
├─ README.md
├─ docs/
├─ apps/
│  ├─ desktop/          # Tauri 2: src/ (React UI), src-tauri/ (Rust core)
│  ├─ web/              # Astro: landing, legal pages, payment-finish page (later: account portal)
│  └─ admin/            # FUTURE: admin dashboard
├─ packages/
│  ├─ shared/           # zod schemas, TS types: composition, template, ticket, error codes
│  ├─ rules-engine/     # pure TS campaign-rule evaluator (no I/O)
│  └─ i18n/             # Indonesian (default) + English strings
├─ supabase/
│  ├─ migrations/       # forward-only SQL
│  ├─ functions/        # Edge Functions (Deno)
│  └─ tests/            # pgTAP + function tests
└─ tools/               # release scripts, manifest signer, benchmark harness
```

## Glossary

| Term | Meaning |
|------|---------|
| VOD | Recorded livestream available on demand |
| Range | The start–end window the user selects from the VOD (max 10 min by default) |
| Generate | Free pipeline: download range → audio → transcript → initial composition |
| Composition | JSON describing the clip (segments, layout, captions, overlays, audio) |
| Template | Campaign preset: format, caption style, branding, post text, rules |
| Credit | Unit consumed by an export that is not covered by an entitlement |
| Entitlement | Server record that a user already paid for specific segments of a source video |
| Export ticket | Short-lived, server-signed permission to render one composition |
| Pass | 30-day paid plan with a higher daily credit allowance |
| Sidecar | External binary shipped/managed by the app (yt-dlp, FFmpeg, whisper.cpp, QuickJS-NG) |
