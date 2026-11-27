# ClipperHelper Embed Bridge

Host-agnostic static YouTube embed bridge for ClipperHelper (`D-13`, `SECURITY.md §9`, `S2-preview.md`).

Provides an authentic HTTPS origin (`https://clipper-bridge.vercel.app`, moving to an owned custom domain in M5) and sends `Referrer-Policy: strict-origin-when-cross-origin` to YouTube's IFrame Player API (`https://www.youtube.com/iframe_api`), eliminating Error 153-class failures inside Tauri WebView2.

## 1. Files

- `index.html` — Static HTML container declaring `<meta name="clipper-bridge-protocol" content="1">` and `<meta name="clipper-bridge-version" content="1.0.0">` with zero inline scripts/styles and zero analytics.
- `style.css` — External dark canvas styles (no `'unsafe-inline'`).
- `bridge.js` — Hardened Protocol v1 `postMessage` relay (plain JS, zero runtime dependencies).
- `vercel.json` — Production Vercel security headers (`frame-ancestors` strictly restricted to `https://tauri.localhost`, `http://tauri.localhost`, `tauri://localhost`; no `'unsafe-inline'`; no `localhost`).
- `vercel.dev.json` — Separate local/DEV preview Vercel config adding `http://localhost:*` / `http://127.0.0.1:*` to `frame-ancestors`.
- `_headers` — Identical production headers for Cloudflare Pages or compatible static hosts.

## 2. Protocol v1 (`postMessage`)

- **Size & rate limits:** Maximum message payload `2048` bytes; sliding window rate limit `60` messages/sec; unknown message types are dropped.
- **Handshake:** Bridge ignores all non-`hello` commands until it receives `hello{v:1, nonce}` from an allowlisted Tauri origin (`tauri://localhost`, `http://tauri.localhost`, `https://tauri.localhost`) where `event.source === window.parent`. Every outbound message uses that validated parent origin as `targetOrigin` (never `'*'`).

### Host → Bridge
| `type` | Fields | Constraints |
|---|---|---|
| `hello` | `{ v: 1, nonce: string }` | Must arrive from allowlisted origin & `window.parent`; `nonce` matches `/^[A-Za-z0-9_-]{4,64}$/` |
| `load` | `{ videoId: string, startSeconds?: number }` | `videoId` matches `/^[A-Za-z0-9_-]{11}$/`; `0 <= startSeconds <= 86400` |
| `play` | `{}` | — |
| `pause` | `{}` | — |
| `seek` | `{ seconds: number }` | `0 <= seconds <= 86400` |
| `setRate` | `{ rate: number }` | `0.25 <= rate <= 2.0` |
| `mute` | `{ bool: boolean }` | Boolean required |

### Bridge → Host
| `type` | Fields | Meaning |
|---|---|---|
| `ready` | `{ v: 1, protocolVersion: 1, version: '1.0.0', nonce: string }` | Handshake acknowledged; host verifies `v === 1` |
| `state` | `{ state: 'loading' \| 'ready' \| 'playing' \| 'paused' \| 'buffering' \| 'ended' \| 'cued' }` | Player lifecycle/playback transition |
| `time` | `{ current: number, duration: number, rate: number, ts: number }` | Sampled current/duration/rate with host wall clock `ts` |
| `error` | `{ code: number }` | Raw YouTube API error code (`2`, `5`, `100`, `101`, `150`, `153`) |

## 3. Deploying & Rolling Back (Vercel / Cloudflare Pages)

### Production Deployment (Git-Connected Vercel Project)
1. In Vercel Dashboard, import the repository (`mdwipasha/clipper-helper`).
2. Set **Root Directory** to `apps/bridge`.
3. Set **Framework Preset** to `Other` (no build command needed; static files served directly).
4. Keep **Output Directory** as `.` (`apps/bridge`).
5. Production pushes to `main` automatically deploy [`vercel.json`](./vercel.json), which locks `frame-ancestors` strictly to the three Tauri origins (`https://tauri.localhost`, `http://tauri.localhost`, `tauri://localhost`).
6. Verify headers after deploy:
   ```powershell
   curl.exe -I https://clipper-bridge.vercel.app
   ```
   Confirm `Referrer-Policy: strict-origin-when-cross-origin`, `X-Content-Type-Options: nosniff`, `Permissions-Policy`, and no `'unsafe-inline'` or `localhost` in `Content-Security-Policy`.

### Separate Development / Local Preview Deployment
- Production `https://clipper-bridge.vercel.app` strictly rejects `http://localhost:1420` (`pnpm tauri dev`).
- **Important (Vercel Preview SSO):** Because `apps/bridge` is linked to the `clipper-bridge` Vercel project, running `vercel --local-config vercel.dev.json` creates a **Preview** deployment on `clipper-bridge`. By default, Vercel Deployment Protection blocks Preview URLs with `HTTP 302 -> vercel.com/sso-api` and `X-Frame-Options: DENY`. Either:
  1. Disable **Vercel Authentication** for Preview Deployments in Vercel (*Settings → Deployment Protection*), then run:
     ```powershell
     cd apps\bridge
     vercel --local-config vercel.dev.json
     ```
  2. Or deploy to a separate dev Vercel project (`--prod`) using `vercel.dev.json`.
- Then run `pnpm tauri dev` from the **same** PowerShell session with `$env:CLIPPER_BRIDGE_URL` set to that public dev URL:
  ```powershell
  $env:CLIPPER_BRIDGE_URL = "https://<your-public-dev-bridge>.vercel.app"
  pnpm tauri dev
  ```
- **To test directly against production `https://clipper-bridge.vercel.app`**, build and launch the bundled app (which runs at `http://tauri.localhost`, matching `vercel.json` `frame-ancestors`):
  ```powershell
  $env:CLIPPER_MIN_MANIFEST_SEQUENCE = "1"
  pnpm --filter @clipper-helper/desktop tauri build --no-bundle
  .\apps\desktop\src-tauri\target\release\clipper-helper-desktop.exe
  ```
- **Never** merge `vercel.dev.json` origins into production `vercel.json`.

### Rollback
1. Open the Vercel project's **Deployments** tab, locate the previous known-good deployment, and click **Promote to Production** (instant edge rollback, zero rebuild wait).
2. Or revert the breaking commit on `main` via `git revert <sha>` and let Git-connected Vercel auto-redeploy.
