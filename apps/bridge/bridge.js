/**
 * ClipperHelper Hardened Embed Bridge (Protocol v1)
 *
 * Host-agnostic YouTube embed relay running on an HTTPS origin with strict
 * handshake, origin allowlist, parent source validation, bounded schemas,
 * rate limiting, and explicit targetOrigin enforcement (NEVER '*').
 */
(function () {
  'use strict';

  const PROTOCOL_VERSION = 1;
  const BRIDGE_VERSION = '1.0.0';
  const MAX_MESSAGE_BYTES = 2048;
  const RATE_LIMIT_WINDOW_MS = 1000;
  const RATE_LIMIT_MAX_MESSAGES = 60;
  const MAX_TIME_SECONDS = 86400;
  const MIN_RATE = 0.25;
  const MAX_RATE = 2.0;
  const VIDEO_ID_REGEX = /^[A-Za-z0-9_-]{11}$/;
  const NONCE_REGEX = /^[A-Za-z0-9_-]{4,64}$/;

  const PROD_ALLOWED_ORIGINS = [
    'tauri://localhost',
    'http://tauri.localhost',
    'https://tauri.localhost',
  ];

  const INBOUND_TYPES = new Set([
    'hello',
    'load',
    'play',
    'pause',
    'seek',
    'setRate',
    'mute',
  ]);

  let player = null;
  let apiReady = false;
  let pendingLoad = null;
  let timeUpdateTimer = null;
  let handshakeCompleted = false;
  let validatedOrigin = null;
  let validatedSource = null;
  let inboundTimestamps = [];

  const loadingOverlay = document.getElementById('loading-overlay');
  const errorOverlay = document.getElementById('error-overlay');
  const errorTitle = document.getElementById('error-title');
  const errorMessage = document.getElementById('error-message');
  const errorCodeEl = document.getElementById('error-code');

  const PROD_HOSTS = ['clipper-bridge.vercel.app'];

  function isDevDeploymentHost() {
    const host = (window.location && window.location.hostname) || '';
    if (host === 'localhost' || host === '127.0.0.1') return true;
    return host.endsWith('.vercel.app') && !PROD_HOSTS.includes(host);
  }

  function isAllowedOrigin(origin) {
    if (typeof origin !== 'string' || !origin) return false;
    if (PROD_ALLOWED_ORIGINS.includes(origin)) return true;
    if (isDevDeploymentHost()) {
      return /^http:\/\/(localhost|127\.0\.0\.1)(:\d{1,5})?$/.test(origin);
    }
    return false;
  }

  function isRateLimited(nowMs) {
    inboundTimestamps = inboundTimestamps.filter((ts) => nowMs - ts < RATE_LIMIT_WINDOW_MS);
    if (inboundTimestamps.length >= RATE_LIMIT_MAX_MESSAGES) {
      return true;
    }
    inboundTimestamps.push(nowMs);
    return false;
  }

  function postValidated(payload) {
    if (!handshakeCompleted || !validatedOrigin || !validatedSource) {
      return false;
    }
    if (validatedOrigin === '*') {
      return false;
    }
    validatedSource.postMessage(
      Object.assign(
        {
          source: 'clipper-bridge',
          protocolVersion: PROTOCOL_VERSION,
          version: BRIDGE_VERSION,
        },
        payload
      ),
      validatedOrigin
    );
    return true;
  }

  function setOverlayError(title, message, codeText) {
    if (loadingOverlay) loadingOverlay.classList.add('hidden');
    if (errorOverlay) errorOverlay.classList.remove('hidden');
    if (errorTitle) errorTitle.textContent = title;
    if (errorMessage) errorMessage.textContent = message;
    if (errorCodeEl) errorCodeEl.textContent = codeText;
  }

  function clearOverlays(loading) {
    if (errorOverlay) errorOverlay.classList.add('hidden');
    if (loadingOverlay) {
      if (loading) {
        loadingOverlay.classList.remove('hidden');
      } else {
        loadingOverlay.classList.add('hidden');
      }
    }
  }

  function stopTimeUpdates() {
    if (timeUpdateTimer !== null) {
      clearInterval(timeUpdateTimer);
      timeUpdateTimer = null;
    }
  }

  function sendTimeUpdate() {
    if (!player || typeof player.getCurrentTime !== 'function') return;
    try {
      const current = Number(player.getCurrentTime() || 0);
      const duration = Number(player.getDuration() || 0);
      const rate =
        typeof player.getPlaybackRate === 'function' ? Number(player.getPlaybackRate() || 1) : 1;
      postValidated({
        type: 'time',
        current: Number.isFinite(current) ? Number(current.toFixed(3)) : 0,
        duration: Number.isFinite(duration) ? Number(duration.toFixed(3)) : 0,
        rate: Number.isFinite(rate) ? rate : 1,
        ts: Date.now(),
      });
    } catch (_err) {
      // Ignore transient getter errors during player initialization
    }
  }

  function startTimeUpdates() {
    stopTimeUpdates();
    sendTimeUpdate();
    // 100ms (~10 Hz, within measured 4-17 Hz envelope)
    timeUpdateTimer = setInterval(sendTimeUpdate, 100);
  }

  function mapYouTubeState(stateCode) {
    if (typeof stateCode !== 'number') return 'loading';
    switch (stateCode) {
      case 1:
        return 'playing';
      case 2:
        return 'paused';
      case 3:
        return 'buffering';
      case 0:
        return 'ended';
      case 5:
        return 'cued';
      default:
        return 'loading';
    }
  }

  function loadYouTubeVideo(videoId, startSeconds) {
    clearOverlays(true);
    if (!apiReady || !window.YT || typeof window.YT.Player !== 'function') {
      pendingLoad = { videoId: videoId, startSeconds: startSeconds };
      postValidated({ type: 'state', state: 'loading' });
      return;
    }

    if (player && typeof player.cueVideoById === 'function') {
      player.cueVideoById({
        videoId: videoId,
        startSeconds: startSeconds,
      });
      postValidated({ type: 'state', state: 'loading' });
      return;
    }

    player = new window.YT.Player('player', {
      videoId: videoId,
      playerVars: {
        enablejsapi: 1,
        origin: window.location.origin,
        playsinline: 1,
        rel: 0,
        modestbranding: 1,
        iv_load_policy: 3,
        controls: 1,
        autoplay: 0,
        start: Math.floor(startSeconds),
      },
      events: {
        onReady: function () {
          clearOverlays(false);
          postValidated({ type: 'state', state: 'ready' });
          sendTimeUpdate();
        },
        onStateChange: function (ev) {
          const state = mapYouTubeState(ev && ev.data);
          if (state === 'playing') {
            clearOverlays(false);
            startTimeUpdates();
          } else {
            stopTimeUpdates();
            sendTimeUpdate();
          }
          postValidated({ type: 'state', state: state });
        },
        onPlaybackRateChange: function () {
          sendTimeUpdate();
        },
        onError: function (ev) {
          stopTimeUpdates();
          const code = ev && typeof ev.data === 'number' ? ev.data : 5;
          setOverlayError(
            'Pratinjau tidak dapat dimuat',
            'Video menolak embed atau mengalami gangguan pemutaran.',
            'YT_' + String(code)
          );
          postValidated({ type: 'error', code: code });
        },
      },
    });
  }

  function measurePayloadBytes(raw) {
    if (typeof raw === 'string') return raw.length;
    try {
      return JSON.stringify(raw).length;
    } catch (_err) {
      return MAX_MESSAGE_BYTES + 1;
    }
  }

  function handleInboundMessage(event) {
    if (!event) return;
    if (!isAllowedOrigin(event.origin)) return;
    if (!window.parent || event.source !== window.parent) return;

    if (isRateLimited(Date.now())) return;
    if (measurePayloadBytes(event.data) > MAX_MESSAGE_BYTES) return;

    const data = event.data;
    if (!data || typeof data !== 'object' || Array.isArray(data)) return;
    if (typeof data.type !== 'string' || !INBOUND_TYPES.has(data.type)) return;

    if (data.type === 'hello') {
      if (data.v !== PROTOCOL_VERSION) return;
      if (typeof data.nonce !== 'string' || !NONCE_REGEX.test(data.nonce)) return;
      handshakeCompleted = true;
      validatedOrigin = event.origin;
      validatedSource = event.source;
      postValidated({
        type: 'ready',
        v: PROTOCOL_VERSION,
        nonce: data.nonce,
      });
      return;
    }

    // All non-hello messages are rejected until hello handshake succeeds from the validated parent.
    if (!handshakeCompleted || event.origin !== validatedOrigin || event.source !== validatedSource) {
      return;
    }

    switch (data.type) {
      case 'load': {
        if (typeof data.videoId !== 'string' || !VIDEO_ID_REGEX.test(data.videoId)) return;
        const startSeconds =
          data.startSeconds === undefined ? 0 : Number(data.startSeconds);
        if (!Number.isFinite(startSeconds) || startSeconds < 0 || startSeconds > MAX_TIME_SECONDS) {
          return;
        }
        loadYouTubeVideo(data.videoId, startSeconds);
        break;
      }

      case 'play': {
        if (player && typeof player.playVideo === 'function') {
          player.playVideo();
        }
        break;
      }

      case 'pause': {
        if (player && typeof player.pauseVideo === 'function') {
          player.pauseVideo();
        }
        break;
      }

      case 'seek': {
        if (typeof data.seconds !== 'number' || !Number.isFinite(data.seconds)) return;
        if (data.seconds < 0 || data.seconds > MAX_TIME_SECONDS) return;
        if (player && typeof player.seekTo === 'function') {
          player.seekTo(data.seconds, true);
          sendTimeUpdate();
        }
        break;
      }

      case 'setRate': {
        if (typeof data.rate !== 'number' || !Number.isFinite(data.rate)) return;
        if (data.rate < MIN_RATE || data.rate > MAX_RATE) return;
        if (player && typeof player.setPlaybackRate === 'function') {
          player.setPlaybackRate(data.rate);
        }
        break;
      }

      case 'mute': {
        const muted =
          typeof data.bool === 'boolean'
            ? data.bool
            : typeof data.muted === 'boolean'
              ? data.muted
              : null;
        if (muted === null) return;
        if (player) {
          if (muted && typeof player.mute === 'function') {
            player.mute();
          } else if (!muted && typeof player.unMute === 'function') {
            player.unMute();
          }
        }
        break;
      }

      default:
        break;
    }
  }

  window.addEventListener('message', handleInboundMessage);

  window.onYouTubeIframeAPIReady = function () {
    apiReady = true;
    if (pendingLoad) {
      const next = pendingLoad;
      pendingLoad = null;
      loadYouTubeVideo(next.videoId, next.startSeconds);
    } else if (loadingOverlay) {
      loadingOverlay.classList.add('hidden');
    }
  };

  if (typeof document !== 'undefined' && document.head && !window.__CLIPPER_SKIP_YT_SCRIPT__) {
    const tag = document.createElement('script');
    tag.src = 'https://www.youtube.com/iframe_api';
    tag.async = true;
    document.head.appendChild(tag);
  }
})();
