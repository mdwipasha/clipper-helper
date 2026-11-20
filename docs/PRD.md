# Product Requirements (PRD)

Priority legend: **M** = must for MVP, **S** = should (MVP if capacity allows), **C** = could (post-MVP).
Numbers marked *(config)* live in the database (`plans`, `app_config`) and can change without an app release.

## 1. Problem and vision

Clippers in campaign programs earn by publishing short vertical clips cut from long livestreams. The manual loop — find the moment, download, cut, reframe to 9:16, caption, add branding, check campaign rules, export — is slow, repetitive, and easy to get wrong.

**Vision:** the fastest path from a YouTube VOD link to a rule-compliant clip, for anyone, on an ordinary Windows laptop.

**Positioning:** not a campaign marketplace and not a full video editor. Our edge is the **workflow** (link → range → template → checklist → export) and being **Indonesian-first** (language, IDR pricing, QRIS/e-wallet/VA payments). General editors are free and more capable; we win by being faster and campaign-aware.

## 2. Users and context

- **Primary:** Indonesian clippers producing several clips per day. Windows laptops, **mid-spec** (≈ 4–6 cores, 8–16 GB RAM, integrated GPU typical). YouTube is the main VOD source. Internet quality varies. Comfortable with consumer editors, not command lines.
- **Secondary:** small groups sharing one campaign template.

## 3. Goals and non-goals (MVP)

**Goals**
- G1. A new user exports a first clip with no outside help.
- G2. The user **always knows what the app is doing, how far along it is, and roughly how long it will take.**
- G3. Credits, plans and payments cannot be trivially bypassed or abused.
- G4. The system stays cheap to run and scales without re-architecture.

**Non-goals (MVP)**
Campaign marketplace / discovery; submitting or tracking posts; live capture; sources other than YouTube; macOS / web / mobile; AI highlight detection; face tracking; music library; cloud rendering; collaboration; auto-posting to social platforms.

## 4. Core flow (Phase 1)

| Step | Screen | What happens |
|------|--------|--------------|
| 0 | First run | Sign in → register device → hardware probe + short benchmark → download components (downloader, JS runtime, FFmpeg, speech model) with progress. |
| 1 | Home | Paste YouTube link; recent projects; credit indicator. |
| 2 | Source & range | Metadata fetched; official-player preview; user sets start/end (button from current time, timecode, ±nudge). |
| 3 | Template | Choose a campaign template (or a built-in generic one). |
| 4 | Generate | Free. Download range → audio → transcript → initial composition. Stage + progress + ETA shown throughout. |
| 5 | Editor | Trim, layout, captions, logo/text. Autosave, undo/redo. |
| 6 | Checklist & export | Template rules evaluated; dialog states whether this export costs 1 credit or is free (entitlement). Online approval → render with progress + ETA → post-render verification. |
| 7 | Done | Open folder, copy post text (caption + hashtags), "make another clip from this range". |

> [!NOTE]
> **Milestone mapping:** Step 0 account steps (sign in, device registration) arrive in M3. In M1 the first run is hardware probe + benchmark + component download only.

## 5. Functional requirements

### Accounts and devices
| ID | Pri | Requirement |
|----|-----|-------------|
| FR-AUTH-1 | M | Sign in with Google (PKCE) and email OTP. Session persists in OS secure storage. Sign out. |
| FR-AUTH-2 | M | Device registered at first sign-in. Active devices per account limited *(config, default 2)*. Over the limit → user can revoke an old device from the app. |
| FR-AUTH-3 | M | Account screen: plan, credits used/left today, reset time (00:00 WIB), pass expiry, devices. |

### Source and range
| ID | Pri | Requirement |
|----|-----|-------------|
| FR-SRC-1 | M | Accept `watch`, `live`, `shorts`, `youtu.be` URLs; normalize to a video id. Playlists/channels rejected with a clear message. |
| FR-SRC-2 | M | Fetch metadata (title, channel, duration, thumbnail, live state). Specific messages for: ongoing live, private/deleted, members-only, age-restricted, region-blocked, premiere not started. |
| FR-SRC-3 | M | Preview with the official player (embed) with current-time readout. If embedding fails, fall back to storyboard thumbnails + timecode inputs. |
| FR-SRC-4 | M | Range picker: "set start/end here", timecode inputs, ±1 s/±5 s nudges, shows duration. Max range **10 min** *(config)*. Validates 0 ≤ start < end ≤ video duration. Displays estimated download time at range selection: "Perkiraan unduh: ~N menit" (based on measured download rate × range duration). |
| FR-SRC-5 | S | Open a local video file as the source; same flow after range. Serves as resilience when the downloader breaks. **Decision rule (Q4):** after S1, if the downloader shows fragility (frequent extractor failures or throttling), promote to Must and schedule in M2; otherwise first post-MVP item. |

### Generate
| ID | Pri | Requirement |
|----|-----|-------------|
| FR-GEN-1 | M | Generate = download range (≤1080p, prefer H.264/AAC) → extract audio → transcribe (id / en / auto) → build composition from the template. Free and unlimited; **one at a time**. |
| FR-GEN-2 | M | Resumable and cached: same range + quality reuses artifacts; interrupted downloads resume. |
| FR-GEN-3 | M | Components auto-download on first run and update independently of the app (signed manifest). |

### Editor
| ID | Pri | Requirement |
|----|-----|-------------|
| FR-EDIT-1 | M | Trim with in/out handles, frame-level nudge, looped playback. |
| FR-EDIT-2 | M | Layouts: **fill** (draggable crop), **blur-fit**, **split** (two regions, e.g. gameplay + facecam). Live preview. |
| FR-EDIT-3 | M | Captions from transcript: edit text/timing, style from template with overrides, language switch. |
| FR-EDIT-4 | M | Logo/watermark overlay (position, size, opacity) and fixed text overlay. |
| FR-EDIT-5 | S | Cut out a middle section; remove silences. |
| FR-EDIT-6 | S | Audio gain + loudness normalization. |
| FR-EDIT-7 | M | Undo/redo, autosave, crash recovery. |
| FR-EDIT-8 | C | Face-aware auto reframe (local ONNX) — post-MVP. |

### Templates and checklist
| ID | Pri | Requirement |
|----|-----|-------------|
| FR-TPL-1 | M | Create/edit/duplicate/delete campaign templates per schema (DATA_MODEL §8). Schema-validated. |
| FR-TPL-2 | M | Built-in starter templates (generic TikTok / Reels / Shorts). |
| FR-TPL-3 | S | Sync templates to the account; share by short code; import with validation. |
| FR-TPL-4 | M | Template carries post text (caption template, hashtags, mentions, disclosure) with a copy button. |
| FR-CHK-1 | M | Pre-export checklist from template rules. **Errors block export; warnings do not.** |
| FR-CHK-2 | M | Post-render verification with ffprobe (duration, resolution, fps, size, audio stream). |

### Export and credits
| ID | Pri | Requirement |
|----|-----|-------------|
| FR-EXP-1 | M | Export 1080×1920 H.264/AAC MP4 with `+faststart` to a chosen folder; filename pattern. |
| FR-EXP-2 | M | Encoder auto-selected: `h264_nvenc` → `h264_qsv` → `h264_amf` → `h264_mf` → `libopenh264` (if S5 confirms) → `x264` (if legally approved) → fail with `E_ENCODER_UNAVAILABLE` and actionable message. User is told which encoder is active and why if slow. |
| FR-EXP-3 | M | Export needs **online approval** (ticket). Offline → clear message; editing still works offline. |
| FR-EXP-4 | M | Re-export is free when covered by an entitlement (same source + segments ± tolerance). Caption/layout/template edits never cost credits. |
| FR-CRD-1 | M | Free plan: **3 credits/day** *(config)*, reset 00:00 WIB, countdown shown. |
| FR-CRD-2 | M | Pass plan: higher daily credits *(config)*. |
| FR-CRD-3 | M | Out of credits → blocking dialog with "Buy pass" and "Resets in hh:mm". |
| FR-CRD-4 | M | Before export, the dialog states exactly: "This export uses 1 credit (N left)" or "Free — you already exported this clip". |
| FR-CRD-5 | M | Credits are **not auto-refunded**; retries of the same clip are free. Support/admin can adjust. |

### Payments
| ID | Pri | Requirement |
|----|-----|-------------|
| FR-PAY-1 | M | Buy pass: checkout opens in the system browser (Midtrans Snap). App updates automatically when payment is confirmed. |
| FR-PAY-2 | M | Pending / expired / failed / cancelled states handled and explained. |
| FR-PAY-3 | S | Purchase history. |
| FR-PAY-4 | M | Admin-granted plans (no payment) work identically after refresh. |

### System
| ID | Pri | Requirement |
|----|-----|-------------|
| FR-PRG-1 | M | Progress contract of `PROGRESS_AND_ETA.md` for every operation. |
| FR-SYS-1 | M | Signed auto-update for the app; independent component updates; minimum-version enforcement by server. |
| FR-SYS-2 | M | Settings: output folder, language, encoder, speech model, cache limit + cleanup. |
| FR-SYS-3 | M | "Export diagnostics" bundle; crash reporting and telemetry **opt-in** with clear text. |
| FR-SYS-4 | M | Disk-space guard before download/render. |

## 6. Plans and limits (all *(config)*)

| Item | Free | Pass (30 days) |
|------|------|----------------|
| Daily credits | 3 | **TBD** (placeholder 20) |
| Export quality | Same | Same |
| Active devices | 2 | 2 |
| Max range per source | 10 min | 10 min |
| Max clip length | 5 min | 5 min |
| Price | – | **TBD** (decide after beta willingness-to-pay check) |
| Rollover | None | None |

Until M4 sets a price (`price_idr` in the `plans` table is `NULL` = not purchasable via checkout), the pass can only be granted by admin SQL functions (intended for beta). Purchase while a pass is active **queues** a new 30-day grant after the current one. Admin grants use the same mechanism.

## 7. Non-functional requirements

Targets marked ◇ are **provisional** until the M0 benchmark on a representative mid-spec laptop; record results in `docs/benchmarks/`.

| Area | Requirement |
|------|-------------|
| Responsiveness | Cold start to interactive ≤ 3 s ◇. UI never blocks on sidecars. |
| Throughput | Provisional goals ◇: download ≈ 1.8–1.9× real-time in copy mode (measured on DEV fiber: 10 min range ≈ 5 min; mid-spec TBD; optimization open in S1c); transcript ≈ real-time or faster with the default model on mid-spec; 60 s export ≲ 2× real-time with hardware encoding. |
| Progress clarity | Every operation > 2 s shows stage, progress and ETA (see PROGRESS_AND_ETA.md). |
| Reliability | Jobs resumable after crash/reboot; no corrupt outputs (write temp → verify → move). |
| Resource use | Heavy stages limited to hardware-appropriate concurrency; editor ≲ 600 MB RAM ◇. |
| Security | See SECURITY.md. Credits/payments enforced server-side. |
| Privacy | Video content and transcripts stay local. Telemetry opt-in, minimal, UU PDP-aligned. |
| i18n | Indonesian default, English fallback, all strings externalized. |
| Scalability | Server work per active user ≈ a handful of small requests per day; heavy artifacts served from CDN. See ARCHITECTURE §12. |
| Accessibility | Keyboard-operable editor controls; readable contrast; no reliance on color alone. |

## 8. Success metrics

Activation: % of new users who export a clip in their first session. Time from link paste to first export. Export success rate. Crash-free sessions. ETA accuracy (|estimate − actual| / actual, p50/p90). Free→pass conversion. D7/D30 retention. Support tickets per 100 exports.

## 9. Risks

| Risk | Impact | Mitigation |
|------|--------|-----------|
| Downloader breaks when YouTube changes | Core flow dead | Independent signed component updates; local-file fallback (FR-SRC-5); monitor failure rate by version; fast-release process. |
| YouTube/ToS and copyright exposure | Legal / takedown | Terms: user responsible for rights; recommend campaign-provided material; keep source layer replaceable; legal review before public launch. |
| Mid-spec laptops too slow | Poor UX | Hardware probe + adaptive defaults; benchmark gate (M0); honest ETA. |
| Bypass of credits | Revenue leakage | Server tickets + entitlements + device caps (SECURITY.md); accept residual risk of local cracking. |
| Competing free editors | Weak differentiation | Stay focused on workflow, templates and checklist; do not chase editor feature parity. |
| Embed preview blocked (Error 153-class) | Range picking degraded | Bridge page + fallback (D-13, spike S2). |
| FFmpeg/x264/H.264 licensing | Legal | LGPL build + hardware encoders; legal review for any GPL component. |
| Payment edge cases | Lost revenue / angry users | Webhook + status-API confirmation, reconciliation job, admin tools. |

## 10. Post-MVP

macOS (Tauri build or web with limits), live capture ("clip last 60 s"), AI highlight suggestions, face-aware reframe, credit packs, campaign submission tracker, local-file source (if not in MVP), template marketplace, admin dashboard (see ADMIN_DASHBOARD.md).

## 11. Open questions (with working defaults)

| # | Question | Default until decided |
|---|----------|----------------------|
| Q1 | Pass price and daily credits | Placeholders in `plans`; set after beta. |
| Q2 | Device limit per account | 2; swap cooldown 7 days. |
| Q3 | Range / clip caps | 10 min / 5 min. |
| Q4 | Is local-file import (FR-SRC-5) in MVP? | Yes if M1–M2 finish on time; otherwise first post-MVP item. |
| Q5 | Legal entity, ToS, privacy policy, refund policy texts | Required before payment go-live (Midtrans production). |
| Q6 | Login CAPTCHA approach (Turnstile in webview vs hosted page) | Decide in spike S7. |
