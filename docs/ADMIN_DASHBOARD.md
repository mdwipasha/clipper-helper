# Admin Dashboard (future — not in MVP)

Focus now is the desktop app. This document records the dashboard's purpose and **what the app/server must already support** so no rework is needed later.

## 1. Purposes

1. **Grant plans** to specific accounts (e.g. friends/beta testers) without payment.
2. **Monitor** activity: users, exports, errors, payments, app versions, abuse.
3. **Operate**: change config/feature flags, minimum version, component rollout; support users (devices, credit adjustments).

## 2. What is already prepared in the schema

| Need | Prepared by |
|------|-------------|
| Manual paid-tier access | `plan_grants` (`source='admin_grant'`, `granted_by`, `note`, optional revoke) + `admin_grant_plan()` SQL function usable **today** |
| Credit goodwill adjustments | `credit_ledger.kind='admin_adjust'` (+delta on the current WIB day) |
| Audit trail | `admin_audit_log` for every admin write |
| Roles | `admin_users(role: owner/support/viewer)` |
| Monitoring data | `exports`, `credit_ledger`, `orders`, `payment_events`, `devices`, `abuse_flags`, `events` |
| Remote control | `app_config` (min version, maintenance, limits, flags), `component_manifests` (rollout %, active flag) |

## 3. Planned screens

- **Users:** search by email/id → detail (plan, grants, devices, recent exports, ledger, orders, flags). Actions: grant/revoke plan, adjust credits, revoke device, suspend/unsuspend.
- **Orders/Payments:** list with Midtrans status; reconcile now; mark refund/chargeback effects.
- **Metrics:** DAU/MAU, exports/day, new vs returning, conversion free→pass, error codes by stage, ETA accuracy, app/component versions, device classes, downloader failure rate.
- **Abuse:** flags queue with context; bulk actions.
- **Config:** edit `app_config` and component rollout with confirmation + audit.
- **Templates (later):** moderation of shared templates.

## 4. Security requirements

- Separate admin app/origin; Supabase Auth user must exist in `admin_users`; **MFA required**; short sessions; optional IP allowlist.
- No direct table writes from the browser. All writes via `SECURITY DEFINER` admin RPCs that (a) check role from `admin_users`, (b) validate inputs, (c) write `admin_audit_log` with admin id and IP.
- Role matrix: `viewer` = read only; `support` = grant/adjust/revoke/suspend within limits (e.g. grants ≤ 90 days); `owner` = everything incl. config and roles.
- Dangerous actions (plan revoke, suspend, config changes) require a reason; show diff before confirm.
- Never expose service-role keys in the browser; server-side data access only (RPC with the admin's JWT).

## 5. Stack options (decide when starting)

| Option | Pros | Cons |
|--------|------|------|
| **Supabase-native web app** (Astro/React + Supabase JS, admin RPCs + RLS) — *recommended* | Single auth system, small blast radius, reuses `packages/shared`, deploy on Vercel | You build tables/charts yourself (use a table/chart library) |
| Laravel + Filament over the same Postgres | Very fast CRUD/admin UI; familiar | Second runtime, needs a DB role with broad access (larger blast radius), two auth systems to reconcile |

Whichever is chosen: keep migrations owned by `supabase/migrations`, and restrict the dashboard to the admin RPCs/views listed in this document.

## 6. Interim operation (before the dashboard exists)

All manual operations use the dedicated `SECURITY DEFINER` admin functions (never raw `INSERT`/`UPDATE`):

- Grant a plan: `select public.admin_grant_plan('friend@example.com', 'pass_30', 30, 'beta tester');`
- Revoke a grant: `select public.admin_revoke_grant('<grant_id>', 'reason text');`
- Adjust credits: `select public.admin_adjust_credits('user@example.com', 3, 'compensation for bug');`

Run via the Supabase SQL editor (service role). Each function writes `admin_audit_log` automatically. Keep these snippets in `docs/runbooks/`.
