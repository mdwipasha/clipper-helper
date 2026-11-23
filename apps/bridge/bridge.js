/**
 * ClipperHelper Embed Bridge (Spike S2 / M1)
 * 
 * Host-agnostic YouTube embed bridge running on an authentic HTTPS origin
 * (Vercel, Cloudflare Pages, or custom domain).
 * 
 * Solves Error 153-class failures by supplying a genuine origin and
 * 'strict-origin-when-cross-origin' referrer to YouTube IFrame API.
 * 
 * Relays player state, timestamps, and errors to the Tauri host application via postMessage.
 */

(function () {
  'use strict';

  // State
  let player = null;
  let isPlayerReady = false;
  let timeUpdateInterval = null;
  let currentVideoId = null;

  // DOM Elements
  const loadingOverlay = document.getElementById('loading-overlay');
  const errorOverlay = document.getElementById('error-overlay');
  const errorTitle = document.getElementById('error-title');
  const errorMessage = document.getElementById('error-message');
  const errorCode = document.getElementById('error-code');

  // Parse URL Parameters
  const urlParams = new URLSearchParams(window.location.search);
  const initialVideoId = urlParams.get('v') || urlParams.get('videoId');
  const initialStart = parseFloat(urlParams.get('start') || '0');
  const initialEnd = urlParams.has('end') ? parseFloat(urlParams.get('end')) : undefined;
  const initialAutoplay = urlParams.get('autoplay') === '1' ? 1 : 0;
  const showControls = urlParams.get('controls') === '0' ? 0 : 1;

  /**
   * Post message to parent window (Tauri app)
   */
  function postToParent(type, data = {}) {
    if (window.parent && window.parent !== window) {
      const message = {
        source: 'clipper-bridge',
        version: '1.0.0',
        type: type,
        timestamp: Date.now(),
        ...data
      };
      // Send to parent using '*' to support custom schemes (tauri://, http://tauri.localhost, etc.)
      window.parent.postMessage(message, '*');
    }
  }

  /**
   * YouTube API State Mapping
   */
  const STATE_NAMES = {
    '-1': 'UNSTARTED',
    '0': 'ENDED',
    '1': 'PLAYING',
    '2': 'PAUSED',
    '3': 'BUFFERING',
    '5': 'CUED'
  };

  /**
   * Start high-frequency time update polling while playing
   * (Emits ~16 events/s [60ms] for fluid scrubber synchronization)
   */
  function startTimeUpdates() {
    stopTimeUpdates();
    sendTimeUpdate();
    timeUpdateInterval = setInterval(sendTimeUpdate, 60);
  }

  function stopTimeUpdates() {
    if (timeUpdateInterval) {
      clearInterval(timeUpdateInterval);
      timeUpdateInterval = null;
    }
  }

  function sendTimeUpdate() {
    if (player && typeof player.getCurrentTime === 'function') {
      try {
        const currentTime = player.getCurrentTime() || 0;
        const duration = player.getDuration() || 0;
        postToParent('timeUpdate', {
          currentTime: Number(currentTime.toFixed(3)),
          duration: Number(duration.toFixed(3))
        });
      } catch (e) {
        // Player may be unmounting or transitioning
      }
    }
  }

  /**
   * Display error card on embed failure
   */
  function showError(title, message, codeStr) {
    loadingOverlay.classList.add('hidden');
    errorOverlay.classList.remove('hidden');
    errorTitle.textContent = title;
    errorMessage.textContent = message;
    errorCode.textContent = codeStr;
  }

  function clearError() {
    errorOverlay.classList.add('hidden');
  }

  /**
   * Initialize YouTube Player
   */
  function initPlayer(videoId, startSec = 0, endSec = undefined, autoplay = 0) {
    currentVideoId = videoId;
    clearError();
    loadingOverlay.classList.remove('hidden');

    const playerVars = {
      enablejsapi: 1,
      origin: window.location.origin,
      playsinline: 1,
      rel: 0,
      modestbranding: 1,
      iv_load_policy: 3,
      controls: showControls,
      autoplay: autoplay
    };

    if (startSec > 0) {
      playerVars.start = Math.floor(startSec);
    }
    if (typeof endSec === 'number' && endSec > startSec) {
      playerVars.end = Math.ceil(endSec);
    }

    if (player) {
      // Re-use existing player
      if (autoplay === 1) {
        player.loadVideoById({
          videoId: videoId,
          startSeconds: startSec,
          endSeconds: endSec
        });
      } else {
        player.cueVideoById({
          videoId: videoId,
          startSeconds: startSec,
          endSeconds: endSec
        });
      }
      return;
    }

    player = new window.YT.Player('player', {
      videoId: videoId || undefined,
      playerVars: playerVars,
      events: {
        onReady: onPlayerReady,
        onStateChange: onPlayerStateChange,
        onPlaybackRateChange: onPlaybackRateChange,
        onError: onPlayerError
      }
    });
  }

  function onPlayerReady(event) {
    isPlayerReady = true;
    loadingOverlay.classList.add('hidden');

    const duration = player.getDuration() || 0;
    postToParent('playerReady', {
      videoId: currentVideoId,
      duration: Number(duration.toFixed(3))
    });

    sendTimeUpdate();
  }

  function onPlayerStateChange(event) {
    const stateCode = event.data;
    const stateName = STATE_NAMES[String(stateCode)] || 'UNKNOWN';

    if (stateCode === window.YT.PlayerState.PLAYING) {
      loadingOverlay.classList.add('hidden');
      startTimeUpdates();
    } else {
      stopTimeUpdates();
      sendTimeUpdate(); // final frame update
    }

    postToParent('stateChange', {
      state: stateName,
      code: stateCode,
      currentTime: player && typeof player.getCurrentTime === 'function' ? Number(player.getCurrentTime().toFixed(3)) : 0,
      duration: player && typeof player.getDuration === 'function' ? Number(player.getDuration().toFixed(3)) : 0
    });
  }

  function onPlaybackRateChange(event) {
    postToParent('playbackRateChange', {
      rate: event.data
    });
  }

  function onPlayerError(event) {
    stopTimeUpdates();
    loadingOverlay.classList.add('hidden');

    const code = event.data;
    let errCode = 'E_YT_UNKNOWN';
    let errTitle = 'Kesalahan Pemutaran Video';
    let errMsg = 'Terjadi kesalahan saat memutar video YouTube.';

    switch (code) {
      case 2:
        errCode = 'E_YT_INVALID_PARAM';
        errTitle = 'Parameter Video Tidak Valid';
        errMsg = 'ID video mengandung karakter tidak valid atau format URL salah.';
        break;
      case 5:
        errCode = 'E_YT_HTML5_ERROR';
        errTitle = 'Kesalahan Pemutar HTML5';
        errMsg = 'Pemutar HTML5 YouTube mengalami kesalahan pada sistem browser ini.';
        break;
      case 100:
        errCode = 'E_YT_NOT_FOUND';
        errTitle = 'Video Tidak Ditemukan';
        errMsg = 'Video telah dihapus, dijadikan privat, atau tidak tersedia.';
        break;
      case 101:
      case 150:
        errCode = 'E_YT_EMBED_RESTRICTED';
        errTitle = 'Pemutaran Embed Ditolak Pemilik';
        errMsg = 'Pemilik video menonaktifkan pemutaran di luar YouTube. Gunakan mode Storyboard/Thumbnail untuk memilih potongan klip.';
        break;
    }

    showError(errTitle, errMsg, `${errCode} (Code ${code})`);

    postToParent('error', {
      code: code,
      errorType: errCode,
      title: errTitle,
      message: errMsg
    });
  }

  /**
   * Handle incoming messages from Parent Window
   */
  window.addEventListener('message', function (event) {
    // Validate message payload structure
    const data = event.data;
    if (!data || typeof data !== 'object') return;

    // Accept messages with type or action
    const action = data.action || data.type;
    if (!action) return;

    switch (action) {
      case 'ping':
        postToParent('pong', { echo: data.payload });
        break;

      case 'loadVideo':
      case 'cueVideo':
        if (data.videoId) {
          const start = typeof data.startSeconds === 'number' ? data.startSeconds : (data.start || 0);
          const end = typeof data.endSeconds === 'number' ? data.endSeconds : data.end;
          const autoplay = data.autoplay ? 1 : 0;
          initPlayer(data.videoId, start, end, autoplay);
        }
        break;

      case 'play':
        if (player && typeof player.playVideo === 'function') {
          player.playVideo();
        }
        break;

      case 'pause':
        if (player && typeof player.pauseVideo === 'function') {
          player.pauseVideo();
        }
        break;

      case 'seekTo':
        if (player && typeof player.seekTo === 'function') {
          const seconds = typeof data.seconds === 'number' ? data.seconds : parseFloat(data.time || '0');
          const allowSeekAhead = data.allowSeekAhead !== false;
          player.seekTo(seconds, allowSeekAhead);
          sendTimeUpdate();
        }
        break;

      case 'setVolume':
        if (player && typeof player.setVolume === 'function') {
          const vol = Math.max(0, Math.min(100, data.volume || 0));
          player.setVolume(vol);
        }
        break;

      case 'mute':
        if (player && typeof player.mute === 'function') {
          player.mute();
        }
        break;

      case 'unMute':
        if (player && typeof player.unMute === 'function') {
          player.unMute();
        }
        break;

      case 'setPlaybackRate':
        if (player && typeof player.setPlaybackRate === 'function') {
          player.setPlaybackRate(data.rate || 1.0);
        }
        break;

      case 'getCurrentTime':
        sendTimeUpdate();
        break;

      default:
        // Unknown command
        break;
    }
  });

  /**
   * Load YouTube IFrame API
   */
  window.onYouTubeIframeAPIReady = function () {
    if (initialVideoId) {
      initPlayer(initialVideoId, initialStart, initialEnd, initialAutoplay);
    } else {
      loadingOverlay.classList.add('hidden');
    }
  };

  // Inject IFrame API script tag (YouTube IFrame API loader is hosted at youtube.com)
  const tag = document.createElement('script');
  tag.src = 'https://www.youtube.com/iframe_api';
  tag.async = true;
  const firstScriptTag = document.getElementsByTagName('script')[0];
  firstScriptTag.parentNode.insertBefore(tag, firstScriptTag);

  // Announce bridge readiness to parent immediately
  postToParent('bridgeReady', {
    hasInitialVideo: Boolean(initialVideoId),
    initialVideoId: initialVideoId || null
  });

})();
