# Security

## 1. Assets and honest limits

**Assets:** credits/plans (revenue), payment integrity, user accounts, signing keys, update channel (code execution on user PCs), templates, user privacy.

**Fundamental limit:** the app runs on the user's machine, so *no client-side protection is unbreakable*. A skilled attacker can patch the binary or run the bundled FFmpeg by hand on a downloaded source. Our goals are therefore:

1. Make **casual** bypass (editing local files/DB, clock changes, reinstalling, multiple accounts, sharing accounts, replaying requests) ineffective.
2. Make **skilled** bypass require real effort and give a result that lacks server-side value (template sync/sharing, rule updates, future AI features, updates).
3. Keep every **money and credit decision on the server**, so the worst case of a cracked client is lost revenue from that user — never other users' data or money.

Accepted residual risks are listed in §14.

## 2. Threat model

| # | Actor / attack | Mitigation | Residual |
|---|----------------|-----------|----------|
| T1 | Casual user edits local DB/files to fake credits | Local balance is display-only; export needs a server-signed ticket | None for honest-client path |
| T2 | Changes system clock to get a new "day" | Day computed in Postgres (`Asia/Jakarta`); ticket `iat/exp` from server | None |
| T3 | Replays/duplicates `begin-export` requests (race) | Per-user row lock, idempotency key, atomic ledger insert | None |
| T4 | Exports one clip, then swaps content in same project for free | Entitlement tied to **source video + absolute time segments** (multirange); tolerance applies to original segments only; per-entitlement caps | Free edits within the tolerance window (intended) |
| T5 | Multi-account farming of free credits | Verified email/Google login, signup rate limits/CAPTCHA, **per-device free cap across accounts**, abuse flags | Determined actors with many devices |
| T6 | Account sharing | Device limit (default 2), swap cooldown, concurrent-device anomaly flags | Friends sharing within limit |
| T7 | Forged/replayed payment webhook | Signature check + Status API confirmation + idempotent apply + amount/user binding | None if implemented as specified |
| T8 | Client-supplied price/plan tampering | Price from `plans` on server; client sends only `plan_code` | None |
| T9 | Payment "success" redirect treated as proof | Redirect page grants nothing; only verified webhook/reconcile grants | None |
| T10 | Fake server (hosts/proxy) returns forged tickets | Ed25519 signature verified with embedded public keys; TLS | Patched binary with swapped key (accepted) |
| T11 | Patched/cracked client skips ticket check | Verification in compiled Rust; code signing; server still limits begin-export; min-version enforcement | **Accepted** (§14) |
| T12 | Running FFmpeg manually on cached source | Cannot prevent; value of workflow/templates/checklist/sync remains server-linked | **Accepted** |
| T13 | Malicious template/share code (huge sizes, path traversal, odd fonts) | Strict schema validation, size caps, bundled-font allowlist, assets by owned id only | Low |
| T14 | Compromised CDN / MITM replacing sidecars | **Signed manifest** + SHA-256 per file, pinned keys, TLS | Key compromise (see runbook) |
| T15 | Supply-chain compromise (npm/crates/CI) | Pinned versions + lockfiles, audit in CI, minimal deps, protected release keys/branches, 2FA | Residual industry risk |
| T16 | SQL/RLS mistakes expose data | Default deny, no direct table access, pgTAP RLS tests, `SECURITY DEFINER` hygiene | Review needed on every migration |
| T17 | Webview XSS / malicious embedded content | Strict CSP, no remote scripts, sanitized text rendering, `frame-src` limited to bridge origin, no `dangerouslySetInnerHTML` | Low |
| T18 | Command injection via URL/filenames | Rust URL parser + host allowlist; arg arrays; path sanitizing; no shell | Low |
| T19 | Token theft from disk | Refresh token in Windows Credential Manager; short-lived access tokens; device revoke | Malware on user PC |
| T20 | Admin account takeover (future dashboard) | Separate role, MFA, audited RPCs, IP allowlist optional | Low |

## 3. Principles

Server authority · least privilege · default deny · validate at every boundary · idempotency everywhere money/credits move · no secrets in clients · defense in depth · log and alert on anomalies · prefer boring, well-reviewed mechanisms.

## 4. Authentication and sessions

- **Methods:** Google OAuth with PKCE via the system browser + deep link (`clipperhelper://auth/callback`, validate `state`, reject unknown callbacks); email OTP in-app. Spike S7 decides how CAPTCHA (Turnstile) is applied to OTP/signup (webview vs hosted page).
- **Tokens:** short-lived access JWT; refresh token in Windows Credential Manager (`keyring`); never in SQLite/logs/localStorage.
- **Device registration** (`register-device`): requires auth; `fp_hash = sha256(MachineGuid + per-app salt)`, stored hashed; enforces `device_limit` (stale devices >60 days auto-freed); swap cooldown (default 7 days); user can revoke devices.
- **Account status:** `suspended` blocks `begin-export` and purchases.

## 5. Credit and entitlement enforcement (export tickets)

**Flow (see ARCHITECTURE §5.2):** app → `begin-export` (JWT + device + idempotency key + absolute segments + `comp_hash`) → `rpc_begin_export` (atomic, DATA_MODEL §5) → Edge Function signs ticket → app verifies → renders.

**Ticket (compact JWS, alg `EdDSA`/Ed25519):**
```
header: { alg:"EdDSA", kid:"k2026-1", typ:"CHX" }
payload: { v:1, sub:<user_id>, did:<device_id>, eid:<export_id>, pid:<project_id>,
           ch:<comp_hash>, ch2:<template_hash|null>, chg:<true|false>,
           iat, exp (= iat + ticket.ttl_seconds, default 900), nonce }
```
**App verification before spawning FFmpeg:** signature with embedded public key for `kid`; `exp` not passed (server-time-skew tolerance via `get-app-config` server time); `sub` = signed-in user; `did` = this device; `ch` = SHA-256 of the **current** canonical composition (any edit after approval needs a new ticket — free if covered by the entitlement); `pid` matches. Verification code lives in Rust.

**Charging model:** the credit is consumed (or entitlement matched) at ticket issue; no refunds (D-09). An interrupted/failed render is retried free with a new ticket because the entitlement exists. Support/admin can adjust via audited admin tools.

**Why charge-at-issue:** charging at completion lets a client obtain a ticket, render, and never report. Charging first makes "skip the report" pointless.

**Key management**
- Ticket signing private key only in Edge Function secrets; **separate** keys for tickets, component manifests, and app updater.
- Embed the active and next public keys in the app (`kid`); rotate by shipping the next key in advance, then switching `active_kid` in `app_config`; revoke old keys via `min_app_version`.
- Never log tickets or JWTs.

**Offline:** editing works offline; export requires online approval; plan snapshot cached for display only.

**Version gating:** `X-App-Version` checked server-side; below `min_app_version` → `E_APP_OUTDATED` (blocks export/purchase, prompts update). Old vulnerable versions are retired through this.

## 6. Anti-abuse

| Control | Detail |
|---------|--------|
| Signup hardening | Verified email or Google; disposable-email blocklist; per-IP/device signup rate limits; CAPTCHA (S7). |
| Per-device free cap | Free credits consumed per `fp_hash` per day across accounts ≤ `free_device_daily_cap` (default 5). |
| Rate limits | `begin-export` (per user/device/IP), `register-device`, OTP, `create-checkout`, template share/import. |
| Entitlement caps | Per-entitlement daily/total re-export caps; max segments; max clip length. |
| Anomaly flags (`abuse_flags`) | Many devices/accounts per fp_hash; exports/hour far above plan; repeated `E_NO_CREDITS` hammering; refund/chargeback; webhook signature failures. |
| Responses | Flag → alert → manual review; temporary throttle; `suspended` status; device revoke; kill switch via `app_config.maintenance` / `min_app_version`. |

## 7. Payment security (Midtrans Snap)

*Confirm all details against current Midtrans documentation at implementation time.*

**Checkout flow**
1. App calls `create-checkout` (JWT, `plan_code`, app version).
2. Edge Function: auth → rate-limit → `create_order` (price from `plans`, unique `order_code`, e.g. `CH-<ulid>`) → `POST` to Snap `/snap/v1/transactions` (sandbox `app.sandbox.midtrans.com`, production `app.midtrans.com`) with HTTP Basic auth (server key) and body: `transaction_details{order_id, gross_amount}`, `item_details`, `customer_details` (minimal), `expiry` (e.g. 60 min), `callbacks.finish` = our static finish page.
3. Return `redirect_url` → app opens it in the **system browser**. Card data never touches our app or servers.
4. App shows "Waiting for payment…" and polls `get_account_status` every 3–5 s for ~2 min, then switches to "We'll activate it as soon as payment is confirmed".

**Webhook (`midtrans-webhook`) — exact order of operations**
1. Parse body; reject oversized/malformed payloads.
2. **Verify signature:** `SHA512(order_id + status_code + gross_amount + ServerKey)` equals `signature_key` (constant-time compare; use `gross_amount` exactly as received). Invalid → log + `abuse_flags` + respond 401/403 (no retry value).
3. Compute `dedupe_key`; insert into `payment_events` (unique) — duplicates return 200 without reprocessing.
4. **Confirm with Status API** (`GET /v2/{order_id}/status` on the matching environment): status, `status_code`, `gross_amount`, `fraud_status` must agree. Never trust the notification body alone.
5. Load our `orders` row by `order_code`: amount equals stored `amount_idr`; currency IDR; order not already terminal-conflicting.
6. State handling: `settlement` (or `capture` with `fraud_status=accept`) → `apply_paid_order`; `pending` → `pending`; `expire`/`cancel`/`deny` → terminal non-paid; `refund`/`partial_refund`/`chargeback` → `revoke_order` + flag; `challenge` → hold for manual review.
7. Respond **2xx only after durable write**; on transient failure respond 5xx so the gateway retries.

**Reconciliation (`reconcile-orders`, hourly):** for orders in `created/pending` older than a few minutes, call the Status API and apply the same state logic; daily compare gateway settlement totals with `orders`. Covers lost/late webhooks.

**Other controls:** server key only in Edge Function secrets, separate sandbox/production keys; HTTPS only; no IP allow-listing reliance; raw event log retained; unique `order_code`; purchases rate-limited per user; an unpaid order never grants anything; **go-live checklist** in §15.

## 8. Supabase hardening

- RLS enabled on **every** table; default deny; clients read via RPCs scoped by `auth.uid()` (DATA_MODEL §2–3).
- `SECURITY DEFINER` functions: `set search_path = public, pg_temp`, validate inputs, no dynamic SQL from user input, `revoke … from public` then grant minimal.
- Service-role key only in Edge Function secrets/CI; never in repo, app, or web bundle.
- Edge Functions: authenticate first, zod-validate body, limit body size, CORS locked to our origins (the app uses native requests), stable error codes, no stack traces to clients.
- Separate Supabase projects for dev/staging/prod; production migrations through CI only; PITR/backups enabled; MFA for all dashboard users.
- Storage: template assets bucket private; signed URLs; MIME/size checks; no executable types.

## 9. Desktop app hardening

- **Tauri v2 capabilities:** explicit allowlist of IPC commands; no wildcard shell/fs; asset protocol scoped to the workspace; devtools disabled in release; CSP: `default-src 'self'`, no `unsafe-eval`, `connect-src` limited to our API/CDN, `frame-src` limited to the bridge origin (`https://clipper-bridge.vercel.app`).
- **Bridge security requirements (D-13):**
  - No `'*'` postMessage targets: host sends commands targeted strictly to the bridge origin; bridge posts events back targeted strictly to the validated host origin.
  - Handshake & origin validation: bidirectional handshake verifies `event.origin` against an allowlist (`http://tauri.localhost`, `https://tauri.localhost`, `tauri://localhost`) and validates `event.source === iframe.contentWindow`.
  - Schema validation & size limits: schema-validate every message (TypeScript / zod schema) at the boundary; enforce a strict 64 KB payload size limit.
  - CSP hardening: remove `'unsafe-inline'` from bridge `script-src` in production deployment.
  - Frame-ancestors scoping: production bridge `frame-ancestors` allows Tauri origins only (`https://tauri.localhost http://tauri.localhost tauri://localhost`); `localhost` / `127.0.0.1` are strictly excluded from production and reserved for separate development deployments.
  - App CSP isolation: desktop app CSP enforces `frame-src https://clipper-bridge.vercel.app` only. Direct YouTube frames are blocked.
- **Sidecar execution:** spawned only by Rust; absolute paths inside the install/bin dir; arguments as arrays; inputs validated (URL allowlist, numeric ranges, sanitized output paths inside workspace); Windows **Job Object** with kill-on-close so no orphan processes; timeouts and output-size limits; stderr parsed, never executed.
- **Workspace path safety:** workspace root = `%LOCALAPPDATA%\ClipperHelper\`. All stored paths are POSIX-style, relative to the root, and must match `^(cache|projects)/[A-Za-z0-9._/-]+$` — no `..`, no leading `/`, no drive letters, no UNC paths. Only `resolve_workspace_path()` in Rust converts stored paths to OS paths; it canonicalizes and verifies the result stays inside the root (including across symlinks and NTFS junctions).
- **Integrity:** verify SHA-256 of sidecars at startup (cheap size+mtime check, full hash after updates); refuse to run unexpected binaries.
  - *LGPLv3 user replacement tension:* LGPLv3 §6 grants users the right to modify and replace the FFmpeg library. To balance integrity protection against tampering with license compliance, the app provides an expert setting **"Use custom FFmpeg"** (off by default). When enabled, it skips the SHA-256 integrity check for the FFmpeg sidecar only (`ffmpeg.exe`, `ffprobe.exe`, and shared DLLs), displays a persistent in-app warning banner, and logs the override in diagnostics. This override **never** applies to other sidecars (`yt-dlp`, `qjs`, `whisper`, models), which remain strictly integrity-verified.
- **Install location:** per-user install directory not writable by other users; avoid DLL search-path hijacking (load by absolute path, set safe DLL directories).
- **Signing:** Authenticode signing for installer and binaries; Tauri updater requires signed artifacts; signing keys live in CI secret storage/cloud signing, with protected release branches.
- **Deep links:** accept only `clipperhelper://auth/callback`; validate `state` and PKCE verifier; ignore anything else.
- **Secrets at rest:** Windows Credential Manager only. No tokens in logs, crash reports or diagnostics (redaction tests).
- **Untrusted content:** transcript text, video titles and template strings rendered as text, never HTML; FFmpeg inputs only from our own temp/workspace files; text overlays escaped for ASS/drawtext.

## 10. Supply chain

Pinned exact versions + lockfiles (`pnpm`, `Cargo.lock`); `cargo audit`/`npm audit` and license check in CI; Dependabot/Renovate with review; minimal dependency policy; SBOM generated per release; reproducible-as-practical builds; GitHub org 2FA, protected branches, required reviews on `supabase/`, `src-tauri/`, `.github/`; signing keys never available to PR builds; component manifest signed offline or in a restricted workflow.

## 11. Privacy (UU PDP-aligned) and telemetry

| Data | Where | Purpose | Retention |
|------|-------|---------|-----------|
| Email, user id, display name | Supabase Auth/profiles | Account | Until deletion request |
| Device hash, OS, app version | `devices` | Device limit, abuse | While account exists; stale auto-freed |
| Video id + absolute segments | `entitlements`, `exports` | Free re-export, billing integrity | Account lifetime |
| Orders / payment metadata | `orders`, `payment_events` | Accounting, disputes | Per accounting/legal requirements |
| Template bodies/assets | `templates` | Sync/share | Until deleted |
| Telemetry events (**opt-in**) | `events` | Quality, ETA priors | 90 days |
| Crash reports (**opt-in**) | Sentry | Stability | Per Sentry retention |

**Never leaves the device:** video files, audio, transcripts, caption text, project contents. **Telemetry events (complete list):** `app_start`, `generate_done{stage_ms, device_class, model, ok/error_code}`, `export_done{encoder, render_ms, ok/error_code}`, `component_update{component, ok}`, `caption_edit_rate{model, total_words, edited_words}` (aggregate counts only, no text), `preview_fallback{reason}` (aggregate counts of switches to timecode-only mode; `reason`: `'embed_disabled'` | `'error_101_150'` | `'timeout'` | `'preview_config'`), `error{code, stage}`. No URLs, titles, or text. Provide: privacy policy, in-app consent screen, data export/deletion request path.

**Third-party network exposure note:** Third-party binary artifact hosts (e.g. GitHub Releases via `github.com` and `release-assets.githubusercontent.com`, HuggingFace via `huggingface.co`) observe requesting users' IP addresses during component downloads. To protect user privacy, production builds configure and prefer our self-hosted Cloudflare R2 CDN (`cdn.clipperhelper.com`) as primary source, keeping third-party hosts strictly as verified fallback mirrors declared in the signed manifest.

## 12. Logging, monitoring, incident response

- **Server:** log function name, user id, error code, latency; never tokens/keys/raw card or full webhook secrets. Alerts: spike in `begin-export` errors, webhook signature failures, reconciliation mismatches, abuse flags, component failure rate by version.
- **Runbooks** (maintain in `docs/runbooks/`): *ticket key compromise* (publish next key, rotate `active_kid`, raise `min_app_version`); *Midtrans key leak* (rotate in dashboard, update secret, review `payment_events`); *bad release* (halt rollout, rollback manifest, raise/lower min version); *downloader outage* (push fixed component, enable local-file path, banner via `app_config`); *mass abuse* (tighten caps, flag/suspend, review ledger).

## 13. Security test plan (must exist before launch)

**Credits/entitlements:** 50 concurrent `begin_export` for the same user never exceed daily credits · same idempotency key never double-charges · day boundary at 23:59/00:00 WIB · re-export within tolerance is free and does not extend the entitlement · request outside tolerance or in a different video charges · multirange gaps (two far segments) cannot be covered by a bounding interval · caps enforced · suspended/revoked-device users rejected · per-device free cap across accounts · min-version gate.
**Tickets:** wrong `kid`, bad signature, expired, wrong user, wrong device, wrong `comp_hash`, wrong project → all rejected by the app; tampered payload rejected.
**Payments:** forged signature · valid signature but mismatched amount · valid notification whose Status API disagrees · duplicate notifications · out-of-order (`settlement` then `pending`) · expire/cancel/deny · refund/chargeback revokes grant · double purchase queues correctly · reconcile recovers a lost webhook · pending order grants nothing.
**RLS:** each table accessed as anon/authenticated user A/user B/service role; verify no cross-user reads and no direct writes to protected tables.
**App:** URL parser fuzz (non-YouTube hosts, userinfo tricks, unicode) · template import fuzz/size bombs · path traversal in asset refs/filenames · `resolve_workspace_path` traversal cases (`..\`, `C:\`, `\\server\share`, NTFS alternate data streams e.g. `file::$DATA`, reserved device names `CON`/`NUL`/`PRN`/`AUX`/`COM1`–`COM9`/`LPT1`–`LPT9`) · sidecar arg injection attempts · CSP violations · log/diagnostic redaction · updater rejects unsigned/downgraded artifacts · manifest rejects bad hash/signature.

## 14. Accepted residual risks

A cracked client can skip ticket verification and render with bundled tools; mitigations are version gating, server-side value, signed updates and monitoring — not prevention. Friends can share an account within the device limit. Attackers with many real devices can farm free credits slowly. These are weighed against user friction; revisit with data after beta.

## 15. Go-live security checklist

- [ ] All §13 tests pass in CI · [ ] RLS audit by a second reviewer · [ ] Secrets only in secret stores; repo scanned (e.g. gitleaks)
- [ ] Midtrans production keys + notification URL configured; sandbox keys absent from prod · [ ] Reconciliation job running and alerting
- [ ] Code signing certificate in place; updater keys backed up securely · [ ] Manifest/ticket keys generated, `next_kid` prepared
- [ ] Privacy policy, terms, refund policy published; telemetry consent screen implemented
- [ ] Backups/PITR verified by restore test · [ ] Runbooks written · [ ] Admin accounts have MFA · [ ] Rate limits and alerts live
