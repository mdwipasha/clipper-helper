# Product Requirements (PRD)

Priority legend: **M** = must for MVP, **S** = should (MVP if capacity allows), **C** = could (post-MVP).
Numbers marked *(config)* live in the database (`plans`, `app_config`) and can change without an app release.

## 1. Problem and vision

Clippers in campaign programs earn by publishing short vertical clips cut from long livestreams. The manual loop — find the moment, download, cut, reframe to 9:16, caption, add branding, check campaign rules, export — is slow, repetitive, and easy to get wrong.

**Vision:** the fastest path from a YouTube VOD link to a rule-compliant clip, for anyone, on an ordinary Windows laptop.

**Positioning:** not a campaign marketplace and not a full video editor. Our edge is the **workflow** (link → range → template → checklist → export) and being **Indonesian-first** (language, IDR pricing, QRIS/e-wallet/VA payments). General editors are free and more capable; we win by being faster and campaign-aware.

## 2. Users and context

- **Primary:** Indonesian clippers producing several clips per day. Windows laptops, **Reference machine R1** as benchmark baseline (Ryzen 5 PRO 5650U, 32 GB RAM; minimum supported floor TBD after 4-thread / 8 GB testing in beta). YouTube is the main VOD source. Internet quality varies. Comfortable with consumer editors, not command lines.
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
| 4 | Generate | Free. Download range → (preview proxy if needed) → initial composition without captions. Stage + progress + ETA shown throughout. |
| 5 | Editor | Trim, layout, captions ("Buat caption" button after trim; also offered when Captions tab opens), logo/text. Autosave, undo/redo. |
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
| FR-SRC-1 | M | Accept `http`/`https` URLs for `youtube.com` / `www.` / `m.` (`/watch?v=`, `/live/`, `/shorts/`, `/embed/`) and `youtu.be/<id>`; normalize strictly from the 11-char ID to canonical `https://www.youtube.com/watch?v=<id>` and extract optional `t`/`start` as `suggestedStartSeconds`. Treat `v=` + `list=` as a single video; reject userinfo, extra ports, lookalike/punycode/non-ASCII hosts, internal whitespace/bidi/control chars, double-encoded IDs, `> 2048` chars, and pure playlist/channel/`@handle`/results links with `E_YT_URL_INVALID`. |
| FR-SRC-2 | M | Supervised `yt-dlp` + `qjs` probe fetches typed `ProbeInfo` (sanitized title/channel ≤200 chars, duration, `live_status`, `availability`, `playable_in_embed` → `preview_mode: 'embed' | 'timecode_only'`, zero format URLs exposed to UI) and creates a local SQLite project (`status = 'probed'`; Home lists last 10 with delete). Specific plain-Indonesian error messages for `ongoing_live`, `premiere_not_started`, `post_live`, `private`, `deleted`, `members_only`, `age_restricted`, `region_blocked`, `E_YT_BOT_CHECK`, `E_YT_RATE_LIMITED`, and retryable `E_NET_OFFLINE`/`E_NET_TIMEOUT`. |
| FR-SRC-3 | M | Preview with official YouTube embed (`≥ 200×200`, 16:9, external controls only) through the hardened hosted bridge (`https://clipper-bridge.vercel.app`, Protocol v1, D-13; move to owned domain in M5). Deduplicate probed projects cleanly per `(provider, video_id)`. If embedding fails (`playable_in_embed=false`, YT error 101/150 confirmed against control video, protocol mismatch, or no `ready` within 6 s), switch cleanly to timecode-only mode (FR-SRC-4) with a `"Buka video di browser"` opener link restricted to `https://www.youtube.com/watch?v=<id>[&t=…]`. Storyboard scrubber moved to post-MVP. |
| FR-SRC-4 | M | Range picker (`Beranda` → `"Lanjut"` → `"Pilih rentang"`): start/end timecode inputs (`HH:MM:SS`, `MM:SS`, `S`, optional `.s`), `"Set mulai di sini"` / `"Set akhir di sini"`, ±1 s / ±5 s nudges, custom no-thumbnail draggable timeline bar, `"Putar rentang"` loop via extrapolated clock (4–17 Hz), initial start from `suggestedStartSeconds`, ads guard (`|player - probe| > 2 s` disables set buttons), live download ETA (`range / 1.66 + 10 s`), max range **300 s beta** (`limits.max_range_seconds`), and SQLite persistence (`range_start_ms`, `range_end_ms`, `status = 'ranged'`) restored on reopen. `"Lanjut ke unduh"` stays disabled (`"Segera hadir"`) until M1.4. |
| FR-SRC-5 | S | Open a local video file as the source; same flow after range. Serves as resilience when the downloader breaks. **Decision rule (Q4):** after S1, if the downloader shows fragility (frequent extractor failures or throttling), promote to Must and schedule in M2; otherwise first post-MVP item. |

### Generate
| ID | Pri | Requirement |
|----|-----|-------------|
| FR-GEN-1 | M | Generate = download range (≤1080p, prefer H.264/AAC) → (preview proxy if needed) → build initial composition without captions from the template. Free and unlimited; **one at a time**. |
| FR-GEN-2 | M | Resumable and cached: same range + quality reuses artifacts; interrupted downloads resume. |
| FR-GEN-3 | M | Components auto-download on first run and update independently of the app (signed manifest). |

### Editor
| ID | Pri | Requirement |
|----|-----|-------------|
| FR-EDIT-1 | M | Trim with in/out handles, frame-level nudge, looped playback. |
| FR-EDIT-2 | M | Layouts: **fill** (draggable crop), **blur-fit**, **split** (two regions, e.g. gameplay + facecam). Live preview. |
| FR-EDIT-3 | M | Captions from trimmed clip: trigger "Buat caption" (separate free local operation) or auto-prompt on opening Captions tab; edit text/timing, style from template with overrides, language switch. Animation modes: 'none', 'phrase' (line-level reveal), 'word_highlight', 'pop'. Per-word animations ('word_highlight' and 'pop') require words with timingSource 'dtw'; when words lack 'dtw', both render as 'phrase'. |
| FR-EDIT-4 | M | Logo/watermark overlay (position, size, opacity) and fixed text overlay. |
| FR-EDIT-5 | S | Cut out a middle section; remove silences. |
| FR-EDIT-6 | S | Audio gain + loudness normalization. |
| FR-EDIT-7 | M | Undo/redo, autosave, crash recovery. |
| FR-EDIT-8 | C | Face-aware auto reframe (local ONNX) — post-MVP. |

### Captions
| ID | Pri | Requirement |
|----|-----|-------------|
| FR-CAP-1 | M | Captions use the current trim bounds: extracts audio ONLY for the trimmed segment(s) (+ ~2 s context padding each side), transcribes, and maps words back to the output timeline. |
| FR-CAP-2 | M | Shrinking the trim keeps captions valid without re-transcribing (words outside the new trim bounds are hidden). |
| FR-CAP-3 | M | Extending the trim beyond transcribed coverage marks captions stale and offers a re-run (MVP: re-transcribe the new clip; delta transcription is post-MVP). |
| FR-CAP-4 | M | User text/timing edits survive re-trims (edits are keyed by stable word ID and re-applied over derived words). |

> [!IMPORTANT]
> **Caption-Editing UX is Critical:** Because automated speech recognition (ASR) accuracy on informal Indonesian gaming slang, code-switching, and loud vocal transients is unproven (pending S3c human ground truth), users will frequently need to adjust transcribed words. The inline subtitle editor UX (keyboard navigation, click-to-edit, timing drag) must be frictionless and instant.

### Templates and checklist
| ID | Pri | Requirement |
|----|-----|-------------|
| FR-TPL-1 | M | Create/edit/duplicate/delete campaign templates per schema (DATA_MODEL §8). Schema-validated. |
| FR-TPL-2 | M | Built-in starter templates (generic TikTok / Reels / Shorts). |
| FR-TPL-3 | S | Sync templates to the account; share by short code; import with validation. |
| FR-TPL-4 | M | Template carries post text (caption template, hashtags, mentions, disclosure) with a copy button. |
| FR-CHK-1 | M | Pre-export checklist from template rules. **Errors block export; warnings do not.** The template rule `captions_present` blocks export only when the template requires captions. |
| FR-CHK-2 | M | Post-render verification with ffprobe (duration, resolution, fps, size, audio stream). |

### Export and credits
| ID | Pri | Requirement |
|----|-----|-------------|
| FR-EXP-1 | M | Export 1080×1920 H.264/AAC MP4 with `+faststart` to a chosen folder; filename pattern. |
| FR-EXP-2 | M | Encoder auto-selected: `h264_nvenc` → `h264_qsv` → `h264_amf` → `h264_mf` → fail with `E_ENCODER_UNAVAILABLE` and actionable message (`libopenh264` and `x264` are unapproved pending legal review). User is told which encoder is active and why if slow. |
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
| Max range per source | 5 min *(proposed beta default; hard cap 10 min config)* | 5 min *(proposed beta default; hard cap 10 min config)* |
| Max clip length | 5 min | 5 min |
| Price | – | **TBD** (decide after beta willingness-to-pay check) |
| Rollover | None | None |

Until M4 sets a price (`price_idr` in the `plans` table is `NULL` = not purchasable via checkout), the pass can only be granted by admin SQL functions (intended for beta). Purchase while a pass is active **queues** a new 30-day grant after the current one. Admin grants use the same mechanism. Beta default `max_range_seconds` = 300 (5 min) proposed pending user decision to keep download times ≲ 3 min; the hard maximum remains configurable up to 600 (10 min) in `app_config`.

## 7. Non-functional requirements

Targets marked ◇ are **provisional** until benchmarks across Reference machine R1 and lower-tier test machines (see `docs/benchmarks/README.md`); R1 is the reference, floor TBD.

| Area | Requirement |
|------|-------------|
| Responsiveness | Cold start to interactive ≤ 3 s ◇. UI never blocks on sidecars. |
| Throughput | Provisional goals ◇: download ≈ 1.66× real-time in copy mode (5 min range ≈ 3 min, ~94 MB at 1080p60 on Reference R1; YouTube paces un-chunked DASH by time; yt-dlp native reaches ≈8×; S1d proxy optional post-M1); transcript ≈ real-time or faster with the default model on R1; 60 s export ≲ 2× real-time with hardware encoding. |
| Progress clarity | Every operation > 2 s shows stage, progress and ETA (see PROGRESS_AND_ETA.md). |
| Reliability | Jobs resumable after crash/reboot; no corrupt outputs (write temp → verify → move). |
| Resource use | Heavy stages limited to hardware-appropriate concurrency; editor ≲ 600 MB RAM ◇. |
| Security | See SECURITY.md. Credits/payments enforced server-side. |
| Privacy | Video content and transcripts stay local. Telemetry opt-in, minimal, UU PDP-aligned. |
| i18n | Indonesian default, English fallback, all strings externalized. |
| Scalability | Server work per active user ≈ a handful of small requests per day; heavy artifacts served from CDN. See ARCHITECTURE §12. |
| Accessibility | Keyboard-operable editor controls; readable contrast; no reliance on color alone. |

## 8. Success metrics

Activation: % of new users who export a clip in their first session. Time from link paste to first export. Export success rate. Crash-free sessions. ETA accuracy (|estimate − actual| / actual, p50/p90). Caption edit rate (opt-in telemetry: fraction of words edited by the user per clip, tracked by speech model, as empirical proxy for ASR accuracy). Free→pass conversion. D7/D30 retention. Support tickets per 100 exports.

## 9. Risks

| Risk | Impact | Mitigation |
|------|--------|-----------|
| Downloader breaks when YouTube changes | Core flow dead | Independent signed component updates; local-file fallback (FR-SRC-5); monitor failure rate by version; fast-release process. |
| YouTube/ToS and copyright exposure | Legal / takedown | Terms: user responsible for rights; recommend campaign-provided material; keep source layer replaceable; legal review before public launch. |
| Mid-spec laptops too slow | Poor UX | Hardware probe + adaptive defaults; benchmark gate (M0); honest ETA. |
| Bypass of credits | Revenue leakage | Server tickets + entitlements + device caps (SECURITY.md); accept residual risk of local cracking. |
| Competing free editors | Weak differentiation | Stay focused on workflow, templates and checklist; do not chase editor feature parity. |
| Embed preview blocked (Error 153-class) | Range picking degraded | Hosted bridge page + timecode-only fallback mode (D-13, spike S2). |
| FFmpeg/x264/H.264 licensing | Legal | LGPL build + hardware encoders; legal review for any GPL component. |
| Payment edge cases | Lost revenue / angry users | Webhook + status-API confirmation, reconciliation job, admin tools. |

## 10. Post-MVP

macOS (Tauri build or web with limits), live capture ("clip last 60 s"), AI highlight suggestions, face-aware reframe, storyboard-thumbnail scrubber for embed-disabled videos, credit packs, campaign submission tracker, local-file source (if not in MVP), template marketplace, profanity handling in captions (template option to mask explicit words with a configurable blocklist, common in Indonesian gaming streams), admin dashboard (see ADMIN_DASHBOARD.md).

## 11. Open questions (with working defaults)

| # | Question | Default until decided |
|---|----------|----------------------|
| Q1 | Pass price and daily credits | Placeholders in `plans`; set after beta. |
| Q2 | Device limit per account | 2; swap cooldown 7 days. |
| Q3 | Range / clip caps | 10 min / 5 min. |
| Q4 | Is local-file import (FR-SRC-5) in MVP? | Yes if M1–M2 finish on time; otherwise first post-MVP item. |
| Q5 | Legal entity, ToS, privacy policy, refund policy texts | Required before payment go-live (Midtrans production). |
| Q6 | Login CAPTCHA approach (Turnstile in webview vs hosted page) | Decide in spike S7. |
| Q7 | Profanity masking in captions: should the app offer an automated masking filter via template option or dictionary? | Post-MVP idea; in MVP, clippers manually edit offensive terms in the caption editor. |
