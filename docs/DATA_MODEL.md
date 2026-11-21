# Data Model

Conventions: `uuid` PKs via `gen_random_uuid()`; `timestamptz` everywhere; money as integer IDR; sets of values enforced with `CHECK`; "today" = `Asia/Jakarta` calendar day computed in Postgres. All DDL below is the **intended shape**; implement as forward-only migrations with pgTAP tests.

## 1. Server tables (Postgres)

```sql
create extension if not exists pgcrypto;

-- Identity ---------------------------------------------------------------
create table public.profiles (
  id uuid primary key references auth.users(id) on delete cascade,
  display_name text,
  status text not null default 'active' check (status in ('active','suspended')),
  suspended_reason text,
  created_at timestamptz not null default now()
);
-- row created by trigger on auth.users insert

-- Plans (config-driven) --------------------------------------------------
create table public.plans (
  code text primary key,                          -- 'free', 'pass_30'
  name text not null,
  tier_rank int not null,                         -- higher wins when grants overlap
  daily_credits int not null check (daily_credits >= 0),
  device_limit int not null default 2 check (device_limit > 0),
  price_idr int check (price_idr > 0),            -- null = not purchasable
  duration_days int check (duration_days > 0),    -- null = no expiry (free)
  is_active boolean not null default true,
  features jsonb not null default '{}'
);
insert into public.plans(code,name,tier_rank,daily_credits,device_limit,price_idr,duration_days) values
 ('free','Free',0,3,2,null,null),
 ('pass_30','Pass 30 Hari',10,20,2,null,30);   -- price_idr NULL = not purchasable via checkout;
                                                -- until M4 sets a price, the pass can only be granted
                                                -- by admin functions (intended for beta)

-- Orders / payments ------------------------------------------------------
create table public.orders (
  id uuid primary key default gen_random_uuid(),
  order_code text not null unique,                -- sent to Midtrans as order_id (<= 50 chars)
  user_id uuid not null references public.profiles(id),
  plan_code text not null references public.plans(code),
  amount_idr int not null check (amount_idr > 0), -- copied from plans at creation, never from client
  status text not null default 'created'
    check (status in ('created','pending','paid','failed','expired','cancelled','refunded','chargeback')),
  snap_token text,
  redirect_url text,
  payment_type text,
  midtrans_transaction_id text,
  created_at timestamptz not null default now(),
  expires_at timestamptz,
  paid_at timestamptz,
  last_synced_at timestamptz
);
create index orders_user on public.orders(user_id, created_at desc);
create index orders_open on public.orders(created_at) where status in ('created','pending');

create table public.payment_events (
  id bigint generated always as identity primary key,
  order_code text not null,
  dedupe_key text not null unique,   -- sha256(order_code|transaction_status|status_code|gross_amount|fraud_status)
  payload jsonb not null,
  signature_valid boolean not null,
  verified_with_status_api boolean not null default false,
  received_at timestamptz not null default now(),
  processed_at timestamptz,
  process_result text
);

-- Plan grants (purchases AND admin grants share this) --------------------
create table public.plan_grants (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references public.profiles(id) on delete cascade,
  plan_code text not null references public.plans(code),
  starts_at timestamptz not null,
  ends_at timestamptz not null,
  source text not null check (source in ('purchase','admin_grant','promo')),
  order_id uuid references public.orders(id),
  granted_by uuid references auth.users(id),
  note text,
  revoked_at timestamptz,
  created_at timestamptz not null default now(),
  check (ends_at > starts_at),
  check (source <> 'purchase' or order_id is not null)
);
create unique index plan_grants_one_per_order on public.plan_grants(order_id) where order_id is not null;
create index plan_grants_user_active on public.plan_grants(user_id, ends_at desc) where revoked_at is null;

-- Devices ----------------------------------------------------------------
create table public.devices (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references public.profiles(id) on delete cascade,
  fp_hash text not null,                         -- sha256(machine id + app salt); never raw
  label text, os text, app_version text,
  first_seen_at timestamptz not null default now(),
  last_seen_at timestamptz not null default now(),
  revoked_at timestamptz,
  unique (user_id, fp_hash)
);
create index devices_fp on public.devices(fp_hash);

-- Entitlements, exports, ledger -----------------------------------------
create table public.entitlements (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references public.profiles(id) on delete cascade,
  source_provider text not null check (source_provider in ('youtube','local_file')),
  source_video_id text not null,                 -- YouTube id, or content fingerprint for local files
  segments int8multirange not null,              -- ABSOLUTE VOD time (ms) originally paid for; never mutated
  created_by_export uuid,
  created_at timestamptz not null default now(),
  reexport_count int not null default 0,
  last_reexport_at timestamptz
);
create index entitlements_lookup on public.entitlements(user_id, source_provider, source_video_id);

create table public.exports (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references public.profiles(id),
  device_id uuid not null references public.devices(id),
  entitlement_id uuid not null references public.entitlements(id),
  idempotency_key text not null,
  project_id uuid not null,                      -- client-generated
  comp_hash text not null,
  template_hash text,
  clip_duration_ms int not null check (clip_duration_ms > 0),
  charged boolean not null,                      -- true = consumed a credit, false = entitlement reuse
  app_version text not null,
  status text not null default 'issued' check (status in ('issued','finalized','failed')),
  error_code text,
  render_ms int,
  issued_at timestamptz not null default now(),
  finalized_at timestamptz,
  unique (user_id, idempotency_key)
);
create index exports_user_time on public.exports(user_id, issued_at desc);
create index exports_entitlement on public.exports(entitlement_id, issued_at desc);

create table public.credit_ledger (
  id bigint generated always as identity primary key,
  user_id uuid not null references public.profiles(id) on delete cascade,
  kind text not null check (kind in ('export_consume','admin_adjust')),
  pool text not null default 'daily' check (pool in ('daily')),  -- 'bonus' reserved for credit packs (Phase 2)
  delta int not null,                            -- -1 per consumed export; +n for admin adjust
  local_day date not null,                       -- Asia/Jakarta day this entry counts toward
  export_id uuid references public.exports(id),
  plan_code text not null,                       -- plan in effect when written
  device_fp_hash text,                           -- for per-device free caps
  created_by uuid,
  note text,
  created_at timestamptz not null default now()
);
create index credit_ledger_user_day on public.credit_ledger(user_id, local_day);
create index credit_ledger_device_free_day on public.credit_ledger(device_fp_hash, local_day) where plan_code = 'free';

-- Templates ---------------------------------------------------------------
create table public.templates (
  id uuid primary key default gen_random_uuid(),
  owner_id uuid not null references public.profiles(id) on delete cascade,
  name text not null check (char_length(name) between 1 and 80),
  schema_version int not null,
  body jsonb not null check (octet_length(body::text) <= 65536),
  body_hash text not null,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  deleted_at timestamptz
);
create table public.template_assets (
  id uuid primary key default gen_random_uuid(),
  template_id uuid not null references public.templates(id) on delete cascade,
  storage_path text not null, mime text not null check (mime in ('image/png','image/webp')),
  size_bytes int not null check (size_bytes <= 2097152), sha256 text not null
);
create table public.template_shares (
  code text primary key,                          -- 10-char random, unguessable
  template_id uuid not null references public.templates(id) on delete cascade,
  created_by uuid not null references public.profiles(id),
  expires_at timestamptz, max_imports int, import_count int not null default 0, revoked_at timestamptz
);

-- Config, manifests, admin, telemetry -----------------------------------
create table public.app_config (
  key text primary key, value jsonb not null,
  updated_at timestamptz not null default now(), updated_by uuid
);
-- keys: min_app_version, maintenance, limits, ticket, flags
-- Seed defaults (require AppConfigSchema zod validation in packages/shared):
insert into public.app_config(key, value) values
 ('min_app_version', '"0.0.0"'),
 ('maintenance',     '{"enabled":false,"message":null}'),
 ('limits',          '{"max_range_seconds":300,"max_clip_seconds":300,"reexport_tolerance_ms":15000,"reexport_daily_cap":30,"reexport_total_cap":200,"free_device_daily_cap":5,"max_segments":20,"device_swap_cooldown_days":7}'), -- max_range_seconds: proposed 300 (5 min) for beta, pending user decision; hard max configurable up to 600
 ('ticket',          '{"ttl_seconds":900,"active_kid":null,"next_kid":null}'),
 ('flags',           '{}');

create table public.component_manifests (
  id uuid primary key default gen_random_uuid(),
  component text not null,                        -- 'yt-dlp','deno','ffmpeg','whisper-cpu','whisper-vulkan','model-small',...
  version text not null, platform text not null default 'win-x64',
  channel text not null default 'stable', url text not null,
  size_bytes bigint not null, sha256 text not null,
  min_app_version text, max_app_version text,
  rollout_percent int not null default 100 check (rollout_percent between 0 and 100),
  is_active boolean not null default true, created_at timestamptz not null default now(),
  unique (component, version, platform, channel)
);  -- the served manifest is signed by the manifest key at request time

create table public.admin_users (
  user_id uuid primary key references auth.users(id) on delete cascade,
  role text not null check (role in ('owner','support','viewer')),
  created_at timestamptz not null default now()
);
create table public.admin_audit_log (
  id bigint generated always as identity primary key,
  admin_id uuid,                                  -- null when run from SQL console
  action text not null, target_user_id uuid, payload jsonb not null default '{}',
  ip inet, created_at timestamptz not null default now()
);

create table public.events (                      -- opt-in telemetry, partition by month
  id bigint generated always as identity, user_id uuid, device_fp_hash text,
  name text not null, props jsonb not null default '{}', app_version text,
  created_at timestamptz not null default now(),
  primary key (id, created_at)
) partition by range (created_at);

create table public.abuse_flags (
  id bigint generated always as identity primary key,
  user_id uuid not null, device_fp_hash text, flag text not null, detail jsonb,
  created_at timestamptz not null default now(), resolved_at timestamptz
);
```

## 2. Row-Level Security

Default deny. Clients get **no direct table access** except where listed.

```sql
revoke all on all tables    in schema public from anon, authenticated;
revoke all on all functions in schema public from public, anon, authenticated;
alter default privileges in schema public revoke all on tables    from anon, authenticated;
alter default privileges in schema public revoke all on functions from public, anon, authenticated;
-- then grant EXECUTE only on the client RPCs, and table privileges only as below.
alter table public.templates enable row level security;
create policy templates_own on public.templates for all to authenticated
  using (owner_id = auth.uid()) with check (owner_id = auth.uid());
```

| Table | `authenticated` client | Everything else |
|-------|------------------------|-----------------|
| `profiles`, `plan_grants`, `orders`, `exports`, `entitlements`, `devices`, `credit_ledger` | **No direct access.** Read via `get_account_status()`, `list_devices()`, `list_orders()` | Edge Functions / admin RPCs (service role) |
| `plans` | `select` where `is_active` (RLS) | Admin |
| `templates`, `template_assets` | CRUD own rows (RLS: `owner_id = auth.uid()`) | – |
| `template_shares` | none (Edge Functions `share-template`/`import-template`) | – |
| `payment_events`, `admin_*`, `app_config`, `component_manifests`, `events`, `abuse_flags` | none | service role only |

Every table has RLS **enabled** even when no policy exists (belt and braces).

## 3. SQL functions (client-callable, `SECURITY DEFINER`, `set search_path = public, pg_temp`)

```sql
-- Effective plan = highest tier_rank among active, unrevoked grants; else 'free'
create function public.get_effective_plan(p_user uuid, p_at timestamptz default now())
returns public.plans language sql stable security definer set search_path = public, pg_temp as $$
  select p.* from public.plan_grants g join public.plans p on p.code = g.plan_code
   where g.user_id = p_user and g.revoked_at is null and g.starts_at <= p_at and g.ends_at > p_at
   order by p.tier_rank desc, g.ends_at desc limit 1
$$;  -- wrapper falls back to the 'free' plan row when this returns no row
```

`get_account_status()` (uses `auth.uid()`): returns plan code/name, `daily_credits`, `used_today`, `remaining`, `resets_at`, active pass end, active device count/limit. It never returns other users' data.

## 4. "Today" and remaining credits

```sql
-- today in WIB
select (now() at time zone 'Asia/Jakarta')::date as v_day;
-- next reset (timestamptz)
select (((now() at time zone 'Asia/Jakarta')::date + 1)::timestamp at time zone 'Asia/Jakarta') as resets_at;
-- used today (admin_adjust entries with +delta reduce usage)
select greatest(0, -coalesce(sum(delta),0)) from public.credit_ledger
 where user_id = $1 and local_day = $2 and pool = 'daily';
-- remaining = max(0, plan.daily_credits - used_today)
```
No cron is needed for resets.

## 5. Export approval — `rpc_begin_export` (service role; called by Edge Function `begin-export`)

Inputs: `p_user, p_device_id, p_fp_hash, p_idem, p_project, p_provider, p_video_id, p_segments jsonb ([[start_ms,end_ms],…] absolute VOD time), p_comp_hash, p_template_hash, p_app_version`.
Outputs: `{export_id, entitlement_id, charged, remaining, resets_at, plan_code}` or raises a coded error: `E_ACCOUNT_SUSPENDED`, `E_DEVICE_INVALID`, `E_REQUEST_INVALID`, `E_NO_CREDITS`, `E_DEVICE_LIMIT_FREE`, `E_REEXPORT_CAP`.

Algorithm (all in **one transaction**):

1. **Lock the user:** `select 1 from profiles where id = p_user and status='active' for update` (not found → `E_ACCOUNT_SUSPENDED`). This serializes concurrent requests per user.
2. **Idempotency:** if `exports(user_id, idempotency_key)` exists → return its stored result (no second charge).
3. **Device check:** `devices.id = p_device_id and user_id = p_user and fp_hash = p_fp_hash and revoked_at is null`; else `E_DEVICE_INVALID`. Touch `last_seen_at`.
4. **Validate request:** segment count ≤ `max_segments`; each `0 ≤ start < end ≤ 12 h`; total duration ≤ `max_clip_seconds`; `p_video_id` matches `^[A-Za-z0-9_-]{11}$` for YouTube; else `E_REQUEST_INVALID`.
5. **Build multirange** of the request:
   ```sql
   v_req := (select range_agg(int8range((x->>0)::bigint, (x->>1)::bigint, '[)'))
             from jsonb_array_elements(p_segments) x);
   ```
6. **Find entitlement** (same user + provider + video) whose **original segments expanded by tolerance** cover the request:
   ```sql
   select e.id into v_ent from public.entitlements e
    where e.user_id = p_user and e.source_provider = p_provider and e.source_video_id = p_video_id
      and (select range_agg(int8range(greatest(lower(r) - v_tol, 0), upper(r) + v_tol, '[)'))
             from unnest(e.segments) r) @> v_req
    order by e.created_at desc limit 1;
   ```
   Tolerance (`reexport_tolerance_ms`, default 15 000) applies to the **original** segments only — entitlements are never extended, so repeated small extensions cannot crawl across a stream.
7. **If found (free re-export):** enforce `reexport_daily_cap` (per entitlement per day) and `reexport_total_cap`; increment `reexport_count`; `charged = false`.
8. **Else (new clip → needs a credit):**
   - `v_plan := effective plan`; `v_used := used today`; if `v_used >= v_plan.daily_credits` → `E_NO_CREDITS` (include `resets_at`).
   - **Free plan only:** count today's consumed credits for `p_fp_hash` across all accounts (`credit_ledger where device_fp_hash = p_fp_hash and plan_code='free'`); if ≥ `free_device_daily_cap` → `E_DEVICE_LIMIT_FREE`.
   - Insert `entitlements` (segments = `v_req`, **no tolerance baked in**), insert `exports` (charged = true), insert `credit_ledger (-1, local_day = v_day, plan_code, device_fp_hash)`.
9. Insert/return the `exports` row.

`rpc_finalize_export(p_user, p_export, p_status, p_render_ms, p_error_code)`: updates only the caller's `issued` export to `finalized`/`failed`. **No refund logic** (D-09). Failures remain visible for support/analytics.

## 6. Payments functions (service role)

- `create_order(p_user, p_plan)` → reads `plans.price_idr` (rejects non-purchasable), creates `orders` with a unique `order_code`, returns the row. Called by `create-checkout` before it calls Midtrans.
- `apply_paid_order(p_order_code, p_tx_id, p_payment_type)` — **idempotent**:
  1. `select … from orders where order_code = $1 for update`; if already `paid` return.
  2. Set `status='paid'`, `paid_at`, `midtrans_transaction_id`, `payment_type`.
  3. Insert `plan_grants` (`source='purchase'`, `order_id`) with `starts_at = greatest(now(), coalesce((select max(ends_at) from plan_grants where user_id=… and plan_code=… and revoked_at is null), now()))` and `ends_at = starts_at + duration_days`. The partial unique index on `order_id` makes double-grant impossible.
- `revoke_order(p_order_code, p_reason)` — on refund/chargeback: order → `refunded`/`chargeback`, grant `revoked_at = now()`, insert an `abuse_flags` row for review.

### Manual grant (works today, before the dashboard exists)

```sql
create or replace function public.admin_grant_plan(p_email text, p_plan text, p_days int, p_note text default null)
returns uuid language plpgsql security definer set search_path = public, auth, pg_temp as $$
declare v_user uuid; v_start timestamptz; v_id uuid;
begin
  select id into v_user from auth.users where lower(email) = lower(p_email);
  if v_user is null then raise exception 'user not found: %', p_email; end if;
  if not exists (select 1 from public.plans where code = p_plan) then raise exception 'unknown plan'; end if;
  v_start := greatest(now(), coalesce((select max(ends_at) from public.plan_grants
              where user_id = v_user and plan_code = p_plan and revoked_at is null), now()));
  insert into public.plan_grants(user_id, plan_code, starts_at, ends_at, source, note)
  values (v_user, p_plan, v_start, v_start + make_interval(days => p_days), 'admin_grant', coalesce(p_note,'manual grant (SQL)'))
  returning id into v_id;
  insert into public.admin_audit_log(admin_id, action, target_user_id, payload)
  values (null, 'grant_plan', v_user, jsonb_build_object('plan', p_plan, 'days', p_days, 'grant_id', v_id));
  return v_id;
end $$;
revoke all on function public.admin_grant_plan(text,text,int,text) from public, anon, authenticated;
-- usage (Supabase SQL editor, service role):
-- select public.admin_grant_plan('friend@example.com', 'pass_30', 30, 'beta tester');

create or replace function public.admin_revoke_grant(p_grant_id uuid, p_reason text)
returns void language plpgsql security definer set search_path = public, pg_temp as $$
declare v_grant record;
begin
  select * into v_grant from public.plan_grants where id = p_grant_id;
  if v_grant is null then raise exception 'grant not found: %', p_grant_id; end if;
  if v_grant.revoked_at is not null then raise exception 'grant already revoked'; end if;
  update public.plan_grants set revoked_at = now() where id = p_grant_id;
  insert into public.admin_audit_log(admin_id, action, target_user_id, payload)
  values (null, 'revoke_grant', v_grant.user_id,
    jsonb_build_object('grant_id', p_grant_id, 'plan', v_grant.plan_code, 'reason', p_reason));
end $$;
revoke all on function public.admin_revoke_grant(uuid, text) from public, anon, authenticated;
-- usage: select public.admin_revoke_grant('<grant_id>', 'beta ended');

create or replace function public.admin_adjust_credits(p_email text, p_delta int, p_note text)
returns bigint language plpgsql security definer set search_path = public, auth, pg_temp as $$
declare v_user uuid; v_day date; v_plan text; v_id bigint;
begin
  select id into v_user from auth.users where lower(email) = lower(p_email);
  if v_user is null then raise exception 'user not found: %', p_email; end if;
  v_day := (now() at time zone 'Asia/Jakarta')::date;
  select coalesce((select code from public.plans p join public.plan_grants g on g.plan_code = p.code
    where g.user_id = v_user and g.revoked_at is null and g.starts_at <= now() and g.ends_at > now()
    order by p.tier_rank desc limit 1), 'free') into v_plan;
  insert into public.credit_ledger(user_id, kind, pool, delta, local_day, plan_code, created_by, note)
  values (v_user, 'admin_adjust', 'daily', p_delta, v_day, v_plan, null, coalesce(p_note, 'admin adjustment'))
  returning id into v_id;
  insert into public.admin_audit_log(admin_id, action, target_user_id, payload)
  values (null, 'adjust_credits', v_user, jsonb_build_object('delta', p_delta, 'day', v_day, 'ledger_id', v_id));
  return v_id;
end $$;
revoke all on function public.admin_adjust_credits(text, int, text) from public, anon, authenticated;
-- usage: select public.admin_adjust_credits('user@example.com', 3, 'compensation for bug');
```

## 7. Composition JSON (v1)

```ts
type Rect = { x: number; y: number; w: number; h: number };   // normalized 0..1 of the source frame

interface Composition {
  schemaVersion: 1;
  id: string;                                   // project id (uuid)
  canvas: { width: 1080; height: 1920; fps: 30 | 60 };
  sources: Array<{
    id: string; provider: 'youtube' | 'local_file'; videoId: string;   // local_file: content fingerprint
    path: string;                               // POSIX-style, relative to workspace root; must match ^(cache|projects)/[A-Za-z0-9._/-]+$ (see ARCHITECTURE §9)
    absoluteStartMs: number;                    // where this file starts in the original VOD; working definition: absoluteStartMs = requestedStartMs (S1c verified relative audio sync; M1 sync test gates absolute alignment)
    durationMs: number; width: number; height: number; fps: number;
  }>;
  timeline: Array<{ id: string; sourceId: string; inMs: number; outMs: number }>; // source-relative; ordered; ≤ max_segments
  layout:
    | { mode: 'fill'; crop: Rect }
    | { mode: 'blur_fit'; blur: number }
    | { mode: 'split'; top: Rect; bottom: Rect; ratio: number };        // ratio = top height share, 0.3..0.7
  captions: {
    enabled: boolean;
    status: 'none' | 'running' | 'ready' | 'stale';                     // 'stale' when timeline extends beyond transcribed coverage
    language: 'id' | 'en' | 'auto';
    words: Array<{ id: string; t0: number; t1: number; text: string }>; // OUTPUT-timeline ms, DERIVED from transcript + timeline + edits
    edits: Array<{ wordId: string; text?: string; t0?: number; t1?: number; deleted?: boolean }>; // user overrides surviving re-trims
    style: CaptionStyle;                                                // resolved from template + overrides
  };
  overlays: Array<
    | { type: 'logo'; assetId: string; anchor: Anchor; marginPct: number; widthPct: number; opacity: number }
    | { type: 'text'; text: string; anchor: Anchor; marginPct: number; sizePct: number; color: string; startMs?: number; endMs?: number }>;
  audio: { gainDb: number; normalize: { enabled: boolean; targetLufs: number } };
  template: { id: string; hash: string } | null;
  export: { preset: 'standard' | 'small_file'; audioKbps: number };
  // 'standard' = quality-targeted (per-encoder rate-control in Rust render module; mapping table filled by S4).
  // 'small_file' = bitrate-capped to meet template.format.maxFileSizeMB with margin.
  // If estimated size at 'standard' exceeds the template limit, 'small_file' is selected automatically.
  // Users cannot set CRF/bitrate directly in MVP; fps and resolution come from the template.
}
```
Absolute VOD segments sent to the server = `timeline[i]` mapped through `sources[].absoluteStartMs + inMs/outMs`.

### Source Transcripts & Derived Words

Transcripts live in **SOURCE time** as a cached workspace artifact (kind `transcript`):
```ts
interface SourceTranscript {
  sourceId: string;
  coverage: Array<[number, number]>;                                   // [startMs, endMs][] in source-relative ms covered by transcription
  words: Array<{ id: string; t0: number; t1: number; text: string }>; // source-relative ms with stable UUIDs
}
```
- **Derivation:** `captions.words` (output-timeline ms) is dynamically derived whenever the timeline changes:
  1. Filter source transcript words overlapping `[timeline.inMs, timeline.outMs]`.
  2. Shift timings to output-timeline ms (`t - inMs + segmentOffsetMs`).
  3. Re-apply user overrides from `captions.edits` matched by `wordId` (`text`, nudged `t0/t1`, or `deleted`).
- **Trim updates:**
  - **Shrinking trim:** keeps existing captions valid without re-transcribing (`captions.status = 'ready'`). Words falling outside new trim boundaries are excluded from `captions.words`.
  - **Extending trim:** if new segment boundaries fall within `transcript.coverage`, captions remain valid. If extending beyond `coverage`, `captions.status = 'stale'` and the UI offers "Perbarui caption" (MVP: re-transcribe the new clip; delta transcription is post-MVP).
  - **Re-trim after user edits:** user edits are preserved in `captions.edits` and re-applied over the derived words.
- **Canonical Hash:** `comp_hash` (SHA-256) covers the derived `captions.words` and `captions.edits`.

## 8. Campaign template JSON (v1)

```ts
interface CampaignTemplate {
  schemaVersion: 1; id: string; name: string; description?: string; briefUrl?: string;
  platforms: Array<'tiktok' | 'instagram_reels' | 'youtube_shorts'>;
  format: { width: 1080; height: 1920; fps: 30 | 60; minDurationSec: number; maxDurationSec: number; maxFileSizeMB?: number };
  layout: { default: 'fill' | 'blur_fit' | 'split';
            safeZone: { top: number; bottom: number; left: number; right: number };    // fractions 0..0.4
            split?: { top: Rect; bottom: Rect; ratio: number } };
  captions: CaptionStyle & { enabled: boolean; language: 'id' | 'en' | 'auto' };
  branding: { logo?: { assetId: string; anchor: Anchor; marginPct: number; widthPct: number; opacity: number; mandatory: boolean };
              fixedText?: Array<{ text: string; anchor: Anchor; sizePct: number; color: string }> };
  audio: { normalize: { enabled: boolean; targetLufs: number }; music: 'allowed' | 'forbidden' };
  postText: { captionTemplate: string; hashtags: string[]; mentions: string[]; disclosure?: string };
  rules: Rule[];
}
type Anchor = 'top_left'|'top_right'|'bottom_left'|'bottom_right'|'top_center'|'bottom_center'|'center';
interface CaptionStyle {
  fontFamily: 'Montserrat' | 'Poppins' | 'Inter' | 'Anton';     // bundled OFL fonts only
  fontSizePct: number; weight: 400 | 600 | 700 | 800; color: string; outlineColor: string; outlinePx: number;
  shadow: boolean; positionAnchor: 'top' | 'center' | 'bottom'; offsetYPct: number;
  maxWordsPerLine: number; maxLines: 1 | 2 | 3; animation: 'none' | 'word_highlight' | 'pop';
  highlightColor: string; uppercase: boolean;
}
type Rule =
  | { id: string; type: 'duration_range'; minSec: number; maxSec: number; severity: 'error' | 'warn' }
  | { id: string; type: 'resolution'; width: number; height: number; severity: 'error' | 'warn' }
  | { id: string; type: 'fps'; value: 30 | 60; severity: 'error' | 'warn' }
  | { id: string; type: 'max_file_size'; mb: number; severity: 'error' | 'warn' }
  | { id: string; type: 'logo_present'; severity: 'error' | 'warn' }
  | { id: string; type: 'captions_present'; severity: 'error' | 'warn' }
  | { id: string; type: 'audio_present'; severity: 'error' | 'warn' }
  | { id: string; type: 'post_text_copied'; severity: 'warn' }           // UI acknowledgement
  | { id: string; type: 'manual_confirm'; label: string; severity: 'warn' };
```
**Rules are data, not code.** `packages/rules-engine` implements a closed set of rule types; unknown types fail validation. Templates are validated on create, import, sync and before render; caps: ≤ 20 rules, strings ≤ 500 chars, colors `#RRGGBB(AA)`, numbers range-checked, assets referenced only by owned `assetId`.

## 9. Local SQLite (desktop)

```sql
create table projects (
  id text primary key, title text, source_provider text, source_video_id text, source_url text, source_title text,
  range_start_ms integer, range_end_ms integer, template_id text,
  composition_json text, composition_hash text, status text,
  created_at integer, updated_at integer, last_export_at integer
);
create table jobs (
  id text primary key, project_id text, kind text,              -- generate|captions|export|model_download|component_update|app_update
  status text,                                                  -- queued|running|paused|succeeded|failed|cancelled
  stage text, progress_json text, error_code text, error_detail text,
  idempotency_key text, attempt integer default 0, created_at integer, started_at integer, finished_at integer
);
create table artifacts (
  id text primary key, project_id text, kind text,              -- source|audio|transcript(SourceTranscript)|proxy|waveform|output
  cache_key text unique, path text, size_bytes integer, sha256 text, created_at integer, last_used_at integer, pinned integer default 0
);
create table stage_stats (                                      -- ETA calibration
  id integer primary key autoincrement, device_profile text, stage text, variant text,   -- variant: model/encoder
  work_units real, wall_ms integer, created_at integer
);
create table templates_local (id text primary key, remote_id text, name text, schema_version integer, body_json text, body_hash text, updated_at integer, dirty integer default 0);
create table settings (key text primary key, value text);
create table outbox (id integer primary key autoincrement, kind text, payload text, created_at integer, attempts integer default 0, next_retry_at integer);
create table account_snapshot (id integer primary key check (id = 1), plan_code text, daily_credits integer, used_today integer, resets_at integer, pass_ends_at integer, fetched_at integer); -- DISPLAY ONLY
```
Tokens and the device secret are **not** in SQLite (Windows Credential Manager).

## 10. Indexes, retention, partitioning

- `events`: monthly partitions; keep 90 days; drop old partitions.
- `payment_events`, `admin_audit_log`, `credit_ledger`, `exports`: keep (financial/audit); archive after 24 months if needed.
- `devices`: auto-revoke when `last_seen_at` older than 60 days (frees the slot).
- Revisit partitioning of `credit_ledger`/`exports` beyond tens of millions of rows.
