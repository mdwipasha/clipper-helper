# AGENTS.md — Rules for AI agents and contributors

Read `README.md` first, then the docs it lists. These rules override convenience.

## 1. Project invariants (never violate)

1. **The server is the only authority for credits, plans, entitlements and payments.** The client displays state; it never decides it. Never cache a balance as truth.
2. **No secrets in client code or the installer.** Forbidden in `apps/desktop` and `apps/web`: Supabase service-role key, Midtrans server key, ticket signing private key, any admin credential. Allowed: Supabase URL + anon key, ticket **public** keys.
3. **Tables `credit_ledger`, `exports`, `entitlements`, `orders`, `payment_events`, `plan_grants`, `admin_audit_log` are written only by `SECURITY DEFINER` functions or Edge Functions using the service role.** Never add an RLS policy that lets `authenticated` write to them. Manual admin operations (grants, revokes, credit adjustments) must also use the dedicated admin SQL functions (`admin_grant_plan`, `admin_revoke_grant`, `admin_adjust_credits`), never raw `INSERT`/`UPDATE`.
4. **Midtrans webhooks:** verify signature → confirm via Midtrans Status API → apply idempotently inside one DB transaction. A redirect/"success" page is never proof of payment. Prices come from the server (`plans`), never from the client.
5. **Composition JSON is the single source of truth** for preview and render. Render logic lives in the Rust core (composition → FFmpeg args + ASS file). React components only *preview*.
6. **Every long-running operation emits progress events** per `docs/PROGRESS_AND_ETA.md` (stage, units, rate, ETA). A bare spinner is a bug.
7. **Sidecars are spawned only from the Rust core**, with argument arrays (never shell strings), inside a Windows Job Object, with validated inputs (URL host allowlist, sanitized paths).
8. **Templates, imported share codes, transcripts, filenames and every JSON from the network are untrusted.** Validate with the zod/JSON schemas in `packages/shared` at every boundary; enforce size limits.
9. **Time and day boundaries come from the server.** "Today" = `Asia/Jakarta` calendar day computed in Postgres. Never trust the local clock for credits or tickets.
10. **Money is integer IDR.** No floats.
11. **Errors use the catalog** in `docs/PROGRESS_AND_ETA.md §8` (`E_*` codes). User-facing text is Indonesian via i18n keys; logs and code are English.
12. **No PII or video URLs in logs** unless redacted by the logging layer. No video content or transcripts leave the machine.

## 2. Security-critical code needs extra care

These areas must have tests written **before/with** the code and a human review before merge:
`rpc_begin_export`, entitlement matching, ticket signing/verification, `midtrans-webhook`, `apply_paid_order`, device registration, RLS policies, updater/manifest verification, sidecar spawning, template import.

Do not "simplify" a check because it looks redundant. If a check seems wrong, open a note in `docs/ROADMAP.md → Open questions` instead of removing it.

## 3. Conventions

- **TypeScript:** `strict`, no `any`, no non-null `!` without comment. Shared types/schemas only in `packages/shared`. Prefer zod schemas as the source, infer types.
- **Rust:** `clippy -D warnings`, `rustfmt`, `thiserror` for errors, `tracing` for logs. No `unwrap()`/`expect()` outside tests and startup.
- **IPC:** typed commands/events generated from Rust (e.g. `tauri-specta`); do not hand-write duplicate types.
- **SQL:** forward-only migrations in `supabase/migrations`, each with a pgTAP test for new functions. `SECURITY DEFINER` functions must `set search_path = public, pg_temp` and validate `auth.uid()` / inputs.
- **Edge Functions:** authenticate first, validate body with zod, rate-limit, return stable error codes, never echo secrets/stack traces.
- **State:** Zustand (+ immer) for editor; TanStack Query for server calls. Undo/redo via command pattern on the composition.
- **i18n:** all strings are keys; default `id`, fallback `en`.
- **Config over constants:** limits, plan numbers, tolerances live in `plans` / `app_config`, not hard-coded.
- **Dependencies:** pin exact versions, commit lockfiles, justify every new dependency in the PR; prefer permissive licenses (MIT/Apache/BSD/ISC). Check `docs/ARCHITECTURE.md §15` before adding native binaries.

## 4. Workflow

1. Work **one milestone at a time** (`docs/ROADMAP.md`). Do not build ahead of the current milestone.
2. Small PRs. Each PR: what changed, which doc section it implements, tests, screenshots for UI.
3. Update docs in the same PR when behavior changes. Spec ambiguity → add to *Open questions*, pick the documented default, keep going.
4. Never commit secrets, real user data, or downloaded videos. Test fixtures are short, licensed/own clips.
5. Definition of Done is in `docs/ROADMAP.md §3`. A feature without progress UX, error handling and tests is not done.

## 5. Things you must not do

- Do not add telemetry that is not listed in `docs/SECURITY.md §11`.
- Do not add auto-refund logic, client-side credit checks as the *only* gate, or "temporary" bypass flags in release builds.
- Do not shell out with string concatenation, load remote code, or enable devtools in production builds.
- Do not use `--cookies-from-browser` or ask users for platform credentials (out of scope; privacy risk).
- Do not bundle GPL-licensed binaries without a recorded legal decision (`docs/ARCHITECTURE.md §15`).
- Do not use GPL-only FFmpeg filters (see `docs/benchmarks/gpl-filters-denylist.txt`); render filters must exist in the pinned build's capability JSON; CI fails otherwise.
- Do not implement features listed as Non-goals / Post-MVP.
