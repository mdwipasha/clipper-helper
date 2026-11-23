# ClipperHelper Embed Bridge

Host-agnostic static YouTube embed bridge for ClipperHelper (Spike S2 / Milestone 1).

Runs on an authentic HTTPS origin (e.g. `https://<project>.vercel.app` or `https://player.clipperhelper.com`) to provide YouTube with a valid HTTP Referer (`strict-origin-when-cross-origin`), eliminating **Error 153**-class embed restrictions when embedded within the Tauri desktop application.

---

## 1. Directory Structure

```
apps/bridge/
├── index.html        # Clean HTML5 player container with error & loading states
├── bridge.js         # YouTube IFrame API wrapper & postMessage IPC relay
├── style.css         # Dark theme responsive canvas (zero scrollbars)
├── vercel.json       # Vercel deployment config (CSP frame-ancestors, headers)
├── _headers          # Cloudflare Pages deployment config (identical headers)
├── package.json      # Minimal workspace descriptor & local serve scripts
└── README.md         # Deployment & IPC specification
```

---

## 2. Deployment Instructions

### Option A: Vercel (Current)
1. Deploy via Vercel CLI from this directory:
   ```bash
   cd apps/bridge
   vercel --prod
   ```
   Or link the repository in the Vercel dashboard and set the **Root Directory** to `apps/bridge`.
2. The included [`vercel.json`](./vercel.json) automatically applies all required security headers:
   - `Content-Security-Policy`: restricts `frame-ancestors` to `'self'`, `https://tauri.localhost`, `http://tauri.localhost`, `tauri://localhost`, `http://localhost:*`, and `http://127.0.0.1:*`.
   - `Referrer-Policy: strict-origin-when-cross-origin` (required for YouTube IFrame API).
   - `X-Content-Type-Options: nosniff`.
3. Provide the resulting HTTPS URL (e.g. `https://clipper-bridge.vercel.app`) to configure `frame-src` in the Tauri desktop application.

### Option B: Cloudflare Pages (Future / Host-Agnostic)
1. Connect repo to Cloudflare Pages with build directory `apps/bridge`.
2. The included [`_headers`](./_headers) file is automatically read by Cloudflare Pages to provide identical headers.

### Option C: Custom Domain
1. Map a CNAME (e.g. `player.clipperhelper.com`) in Vercel or Cloudflare Pages.
2. Zero code changes required.

---

## 3. URL Parameters

The bridge supports initial configuration via URL query string:

| Parameter | Type | Default | Description |
|---|---|---|---|
| `v` or `videoId` | string | `null` | YouTube 11-character video ID (e.g. `dQw4w9WgXcQ`) |
| `start` | number | `0` | Initial playback start offset in seconds |
| `end` | number | `undefined` | Optional playback end boundary in seconds |
| `autoplay` | `0` \| `1` | `0` | Autoplay on load |
| `controls` | `0` \| `1` | `1` | Show native YouTube controls |

Example:
```
https://<bridge-domain>/?v=dQw4w9WgXcQ&start=30&end=90&autoplay=0
```

---

## 4. `postMessage` IPC Protocol

The bridge and parent window (Tauri WebView2) communicate via `window.postMessage`.

### 4.1 Inbound Commands (Parent $\to$ Bridge)

Send to `iframe.contentWindow.postMessage(command, '*')`:

| Command / `type` | Payload | Description |
|---|---|---|
| `loadVideo` | `{ videoId: string, startSeconds?: number, endSeconds?: number, autoplay?: boolean }` | Load and play video |
| `cueVideo` | `{ videoId: string, startSeconds?: number, endSeconds?: number }` | Cue video without autoplay |
| `play` | `{}` | Resume playback |
| `pause` | `{}` | Pause playback |
| `seekTo` | `{ seconds: number, allowSeekAhead?: boolean }` | Seek to timestamp |
| `setVolume` | `{ volume: number }` (0–100) | Adjust audio volume |
| `mute` | `{}` | Mute audio |
| `unMute` | `{}` | Unmute audio |
| `setPlaybackRate` | `{ rate: number }` (0.25–2.0) | Set playback speed |
| `getCurrentTime` | `{}` | Request immediate `timeUpdate` |
| `ping` | `{ payload?: any }` | Heartbeat check |

### 4.2 Outbound Events (Bridge $\to$ Parent)

Listen via `window.addEventListener('message', ...)`:

All messages emitted by the bridge carry `{ source: 'clipper-bridge', version: '1.0.0', type: string, ... }`:

| Event `type` | Data Fields | Emitted When |
|---|---|---|
| `bridgeReady` | `hasInitialVideo: boolean`, `initialVideoId: string\|null` | Bridge script loads and is ready for commands |
| `playerReady` | `videoId: string`, `duration: number` | YouTube player has finished initializing |
| `stateChange` | `state: string`, `code: number`, `currentTime: number`, `duration: number` | Playback state changes (`PLAYING`, `PAUSED`, `BUFFERING`, `ENDED`, etc.) |
| `timeUpdate` | `currentTime: number`, `duration: number` | Emitted every ~60 ms while playing for smooth scrubber sync |
| `playbackRateChange`| `rate: number` | Playback rate changed |
| `error` | `code: number`, `errorType: string`, `title: string`, `message: string` | YouTube API reports error (e.g. `E_YT_EMBED_RESTRICTED` on error 101/150) |
| `pong` | `echo: any` | Response to `ping` |

---

## 5. Local Testing

To test locally:
```powershell
python -m http.server 3000 -d apps/bridge
# Open http://localhost:3000/?v=dQw4w9WgXcQ in browser
```
