import * as fs from 'node:fs';
import * as path from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const BRIDGE_DIR = path.resolve(__dirname, '../../../bridge');

const REGISTERED_LISTENERS: Array<{
  type: string;
  listener: EventListenerOrEventListenerObject;
}> = [];

function cleanupBridgeListeners(): void {
  while (REGISTERED_LISTENERS.length > 0) {
    const entry = REGISTERED_LISTENERS.pop();
    if (entry) {
      window.removeEventListener(entry.type, entry.listener);
    }
  }
}

function loadBridgeDom(locationHref = 'https://clipper-bridge.vercel.app/') {
  cleanupBridgeListeners();
  const html = fs.readFileSync(path.join(BRIDGE_DIR, 'index.html'), 'utf8');
  const scriptSource = fs.readFileSync(path.join(BRIDGE_DIR, 'bridge.js'), 'utf8');

  document.documentElement.innerHTML = html;
  Object.defineProperty(window, 'location', {
    configurable: true,
    value: new URL(locationHref),
  });
  (window as unknown as Record<string, unknown>).__CLIPPER_SKIP_YT_SCRIPT__ = true;

  const posted: Array<{ message: unknown; targetOrigin: string }> = [];
  const parentMock = {
    postMessage: vi.fn((message: unknown, targetOrigin: string) => {
      posted.push({ message, targetOrigin });
    }),
  } as unknown as Window;

  Object.defineProperty(window, 'parent', {
    configurable: true,
    value: parentMock,
  });

  const origAddEventListener = window.addEventListener.bind(window);
  const spy = vi.spyOn(window, 'addEventListener').mockImplementation(((
    type: string,
    listener: EventListenerOrEventListenerObject
  ) => {
    REGISTERED_LISTENERS.push({ type, listener });
    origAddEventListener(type, listener);
  }) as typeof window.addEventListener);

  try {
    // Evaluate real bridge.js inside this jsdom window
    const fn = new Function(scriptSource);
    fn();
  } finally {
    spy.mockRestore();
  }

  return { posted, parentMock };
}

function dispatchHostMessage(origin: string, source: unknown, data: unknown): void {
  const ev = new MessageEvent('message', {
    data,
    origin,
    source: source as MessageEventSource | null,
  });
  window.dispatchEvent(ev);
}

describe('Hardened Bridge (apps/bridge) Protocol v1 & CSP', () => {
  const createdPlayers: Array<{
    options: Record<string, unknown>;
    playerInstance: Record<string, ReturnType<typeof vi.fn>>;
  }> = [];

  beforeEach(() => {
    createdPlayers.length = 0;
    vi.useFakeTimers();
    (window as unknown as Record<string, unknown>).YT = {
      Player: function (_el: string, options: Record<string, unknown>) {
        const inst = {
          cueVideoById: vi.fn(),
          playVideo: vi.fn(),
          pauseVideo: vi.fn(),
          seekTo: vi.fn(),
          setPlaybackRate: vi.fn(),
          mute: vi.fn(),
          unMute: vi.fn(),
          getCurrentTime: vi.fn(() => 42.5),
          getDuration: vi.fn(() => 600),
          getPlaybackRate: vi.fn(() => 1.5),
        };
        createdPlayers.push({ options, playerInstance: inst });
        return inst;
      },
    };
  });

  afterEach(() => {
    cleanupBridgeListeners();
    vi.useRealTimers();
    delete (window as unknown as Record<string, unknown>).__CLIPPER_SKIP_YT_SCRIPT__;
    delete (window as unknown as Record<string, unknown>).YT;
  });

  it('requires hello handshake from allowlisted Tauri origin + window.parent and never uses "*" targetOrigin', () => {
    const { posted, parentMock } = loadBridgeDom();
    expect(posted).toHaveLength(0);

    // 1. Disallowed origin rejected
    dispatchHostMessage('https://evil.example', parentMock, {
      type: 'hello',
      v: 1,
      nonce: 'abc12345',
    });
    expect(posted).toHaveLength(0);

    // 2. Non-parent source spoofing allowed origin rejected
    dispatchHostMessage('http://tauri.localhost', window, {
      type: 'hello',
      v: 1,
      nonce: 'abc12345',
    });
    expect(posted).toHaveLength(0);

    // 3. localhost rejected in production host mode
    dispatchHostMessage('http://localhost:1420', parentMock, {
      type: 'hello',
      v: 1,
      nonce: 'abc12345',
    });
    expect(posted).toHaveLength(0);

    // 4. Valid commands before hello rejected
    dispatchHostMessage('http://tauri.localhost', parentMock, {
      type: 'load',
      videoId: 'dQw4w9WgXcQ',
      startSeconds: 10,
    });
    expect(posted).toHaveLength(0);

    // 5. Valid hello from http://tauri.localhost completes handshake with strict targetOrigin
    dispatchHostMessage('http://tauri.localhost', parentMock, {
      type: 'hello',
      v: 1,
      nonce: 'nonce-1234',
    });
    expect(posted).toHaveLength(1);
    expect(posted[0]?.targetOrigin).toBe('http://tauri.localhost');
    expect(posted[0]?.targetOrigin).not.toBe('*');
    expect(posted[0]?.message).toMatchObject({
      type: 'ready',
      v: 1,
      nonce: 'nonce-1234',
    });

    // Bridge source code itself never contains postMessage(..., '*')
    const bridgeJs = fs.readFileSync(path.join(BRIDGE_DIR, 'bridge.js'), 'utf8');
    expect(bridgeJs).not.toMatch(/postMessage\([^)]*['"]\*['"]/);
  });

  it('allows localhost parent origin only on separate dev or local bridge hosts', () => {
    for (const devUrl of ['https://clipper-bridge-dev.vercel.app/', 'http://localhost:3000/']) {
      const devBridge = loadBridgeDom(devUrl);
      dispatchHostMessage('http://localhost:1420', devBridge.parentMock, {
        type: 'hello',
        v: 1,
        nonce: 'dev_nonce_1',
      });
      expect(devBridge.posted).toHaveLength(1);
      expect(devBridge.posted[0]?.targetOrigin).toBe('http://localhost:1420');
    }
  });

  it('supports all three production Tauri origins and strict schema / size / rate checks', () => {
    for (const origin of [
      'tauri://localhost',
      'http://tauri.localhost',
      'https://tauri.localhost',
    ]) {
      const { posted, parentMock } = loadBridgeDom();
      dispatchHostMessage(origin, parentMock, {
        type: 'hello',
        v: 1,
        nonce: 'valid_nonce_1',
      });
      expect(posted[0]?.targetOrigin).toBe(origin);
    }

    const { posted, parentMock } = loadBridgeDom();
    dispatchHostMessage('https://tauri.localhost', parentMock, {
      type: 'hello',
      v: 1,
      nonce: 'valid_nonce_2',
    });

    // Mark YT API ready
    (window as unknown as { onYouTubeIframeAPIReady?: () => void }).onYouTubeIframeAPIReady?.();

    // Oversized (> 2048 bytes) rejected
    dispatchHostMessage('https://tauri.localhost', parentMock, {
      type: 'load',
      videoId: 'dQw4w9WgXcQ',
      pad: 'x'.repeat(2100),
    });
    expect(createdPlayers).toHaveLength(0);

    // Invalid videoId rejected
    dispatchHostMessage('https://tauri.localhost', parentMock, {
      type: 'load',
      videoId: '../bad',
      startSeconds: 0,
    });
    expect(createdPlayers).toHaveLength(0);

    // Unknown type dropped
    dispatchHostMessage('https://tauri.localhost', parentMock, {
      type: 'eval',
      code: 'alert(1)',
    });
    expect(createdPlayers).toHaveLength(0);

    // Valid load creates player and posts stateChange / time with strict targetOrigin
    dispatchHostMessage('https://tauri.localhost', parentMock, {
      type: 'load',
      videoId: 'dQw4w9WgXcQ',
      startSeconds: 30,
    });
    expect(createdPlayers).toHaveLength(1);
    const events = createdPlayers[0]?.options.events as Record<string, (e?: unknown) => void>;
    events.onReady?.();
    expect(
      posted.some(
        (p) =>
          p.targetOrigin === 'https://tauri.localhost' &&
          (p.message as { type: string; state?: string }).type === 'state' &&
          (p.message as { state?: string }).state === 'ready'
      )
    ).toBe(true);

    // Valid play/pause/seek/setRate/mute work, invalid bounds ignored
    dispatchHostMessage('https://tauri.localhost', parentMock, { type: 'seek', seconds: -5 });
    dispatchHostMessage('https://tauri.localhost', parentMock, { type: 'seek', seconds: 90 });
    expect(createdPlayers[0]?.playerInstance.seekTo).toHaveBeenCalledTimes(1);
    expect(createdPlayers[0]?.playerInstance.seekTo).toHaveBeenCalledWith(90, true);

    dispatchHostMessage('https://tauri.localhost', parentMock, { type: 'setRate', rate: 99 });
    dispatchHostMessage('https://tauri.localhost', parentMock, { type: 'setRate', rate: 1.5 });
    expect(createdPlayers[0]?.playerInstance.setPlaybackRate).toHaveBeenCalledTimes(1);
    expect(createdPlayers[0]?.playerInstance.setPlaybackRate).toHaveBeenCalledWith(1.5);

    dispatchHostMessage('https://tauri.localhost', parentMock, { type: 'mute', bool: true });
    expect(createdPlayers[0]?.playerInstance.mute).toHaveBeenCalledTimes(1);

    // Rate limiting caps > 60 messages within 1s
    for (let i = 0; i < 80; i++) {
      dispatchHostMessage('https://tauri.localhost', parentMock, { type: 'play' });
    }
    expect(createdPlayers[0]?.playerInstance.playVideo.mock.calls.length ?? 0).toBeLessThanOrEqual(
      60
    );
  });

  it('enforces hardened CSP and headers in vercel.json, _headers, index.html, and separates localhost to vercel.dev.json', () => {
    const prodVercel = JSON.parse(fs.readFileSync(path.join(BRIDGE_DIR, 'vercel.json'), 'utf8'));
    const headersFile = fs.readFileSync(path.join(BRIDGE_DIR, '_headers'), 'utf8');
    const devVercel = JSON.parse(fs.readFileSync(path.join(BRIDGE_DIR, 'vercel.dev.json'), 'utf8'));
    const indexHtml = fs.readFileSync(path.join(BRIDGE_DIR, 'index.html'), 'utf8');

    // index.html has zero inline scripts or styles, zero analytics, and protocol meta tags
    expect(indexHtml).toContain('<meta name="clipper-bridge-protocol" content="1">');
    expect(indexHtml).toContain('<meta name="clipper-bridge-version" content="1.0.0">');
    expect(indexHtml).toContain('<meta name="referrer" content="strict-origin-when-cross-origin">');
    expect(indexHtml).not.toMatch(/<style\b/i);
    expect(indexHtml).not.toMatch(/<script\b(?![^>]*\bsrc=['"]bridge\.js['"])/i);
    expect(indexHtml).not.toMatch(/vercel-insights|gtag|analytics/i);

    const hdrs: Record<string, string> = {};
    for (const h of prodVercel.headers[0].headers) {
      hdrs[h.key] = h.value;
    }

    expect(hdrs['Referrer-Policy']).toBe('strict-origin-when-cross-origin');
    expect(hdrs['X-Content-Type-Options']).toBe('nosniff');
    expect(hdrs['Permissions-Policy']).toBeTruthy();

    const csp = hdrs['Content-Security-Policy']!;
    expect(csp).toBeTruthy();
    expect(csp).not.toContain("'unsafe-inline'");
    expect(csp).not.toContain("'unsafe-eval'");
    expect(csp).not.toContain('localhost:*');
    expect(csp).not.toContain('127.0.0.1');

    const frameAncestors = csp
      .match(/frame-ancestors\s+([^;]+)/)?.[1]
      ?.trim()
      .split(/\s+/);
    expect(frameAncestors).toEqual([
      'https://tauri.localhost',
      'http://tauri.localhost',
      'tauri://localhost',
    ]);

    // Cloudflare _headers mirrors the exact production CSP
    expect(headersFile).toContain(csp);
    expect(headersFile).toContain('Referrer-Policy: strict-origin-when-cross-origin');
    expect(headersFile).toContain('X-Content-Type-Options: nosniff');

    // Separate dev config includes localhost frame-ancestors without polluting prod
    const devCsp = devVercel.headers[0].headers.find(
      (h: { key: string; value: string }) => h.key === 'Content-Security-Policy'
    ).value;
    expect(devCsp).toContain('http://localhost:*');
    expect(devCsp).not.toContain("'unsafe-inline'");
  });
});
