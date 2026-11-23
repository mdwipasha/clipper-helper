/**
 * S2 Preview Test Harness (app.js)
 * 
 * Host application controller communicating with https://clipper-bridge.vercel.app
 * via HTML5 postMessage API.
 * 
 * Measures:
 *  - Bridge init & player ready latency
 *  - Playback initiation & timeUpdate event frequency (events/sec)
 *  - Seek latency across short and ~3h (10,800s) offsets
 *  - Error trapping (101/150, 153, etc.) and fallback trigger behavior
 *  - Automated matrix execution & JSON persistence
 */

(function () {
  'use strict';

  // Config
  const BRIDGE_ORIGIN = 'https://clipper-bridge.vercel.app';
  const CANDIDATES = [
    { id: 'WufkeDaJisQ', group: '(a)', label: 'Short 10m (Deceit 2)', duration: 602 },
    { id: '3snfJrlSyqo', group: '(a)', label: 'Medium 40m (Podcast)', duration: 2415 },
    { id: 'XwQwhjzAc_k', group: '(a)', label: 'Live VOD 10h (GTA 6)', duration: 35942 },
    { id: '9s6imDGjy08', group: '(c)(d)', label: 'Monetized 3h47m (IShowSpeed)', duration: 13668, test3hSeek: true },
    { id: 'QOJIILaJKNc', group: '(d)', label: 'Long VOD 5.5h (GTA 6)', duration: 19728, test3hSeek: true },
    { id: 'KBdGeakRgqg', group: '(e)', label: 'Members-Only Control', duration: null, expectError: true },
    { id: '6kLq3WMV1nU', group: '(e)', label: 'Age-Restricted Control', duration: null, expectError: true }
  ];

  // State
  let activeVideoId = 'WufkeDaJisQ';
  let activeCandidate = CANDIDATES[0];
  let bridgeReady = false;
  let playerReady = false;
  let currentState = 'UNSTARTED';
  let currentDuration = 0;
  let currentTime = 0;
  let isMuted = false;

  // Latency & metric trackers
  let tLoadStart = 0;
  let tBridgeReady = 0;
  let tPlayerReady = 0;
  let tFirstPlay = 0;
  let seekSentTime = 0;
  let targetSeekOffset = null;
  let updateCount = 0;
  let lastUpdateSec = Date.now();
  let timeUpdateRate = 0;
  let lastSeekLatency = null;
  let lastError = null;

  // Test matrix results store
  const matrixResults = {};

  // DOM Elements
  const iframe = document.getElementById('bridgeIframe');
  const fallbackView = document.getElementById('fallbackView');
  const fallbackDuration = document.getElementById('fallbackDuration');
  const dismissFallbackBtn = document.getElementById('dismissFallbackBtn');

  // Metrics DOM
  const metricVideoId = document.getElementById('metricVideoId');
  const metricState = document.getElementById('metricState');
  const metricTimeBridgeReady = document.getElementById('metricTimeBridgeReady');
  const metricTimePlayerReady = document.getElementById('metricTimePlayerReady');
  const metricTimeToPlay = document.getElementById('metricTimeToPlay');
  const metricUpdateRate = document.getElementById('metricUpdateRate');
  const metricSeekLatency = document.getElementById('metricSeekLatency');
  const metricError = document.getElementById('metricError');
  const metricDiagnosis = document.getElementById('metricDiagnosis');
  const matrixCompletedCount = document.getElementById('matrixCompletedCount');
  const matrixResultsList = document.getElementById('matrixResultsList');
  const logStream = document.getElementById('logStream');

  // Transport DOM
  const playBtn = document.getElementById('playBtn');
  const pauseBtn = document.getElementById('pauseBtn');
  const seekBack5Btn = document.getElementById('seekBack5Btn');
  const seekFwd5Btn = document.getElementById('seekFwd5Btn');
  const seek30Btn = document.getElementById('seek30Btn');
  const seek300Btn = document.getElementById('seek300Btn');
  const seek3hBtn = document.getElementById('seek3hBtn');
  const scrubberSlider = document.getElementById('scrubberSlider');
  const timeReadout = document.getElementById('timeReadout');
  const muteToggleBtn = document.getElementById('muteToggleBtn');
  const volumeSlider = document.getElementById('volumeSlider');

  // Candidate buttons
  const candButtons = document.querySelectorAll('.cand-btn');
  const customVideoInput = document.getElementById('customVideoInput');
  const loadCustomBtn = document.getElementById('loadCustomBtn');
  const runAutoMatrixBtn = document.getElementById('runAutoMatrixBtn');
  const exportResultsBtn = document.getElementById('exportResultsBtn');

  /**
   * Helper: Format seconds to HH:MM:SS
   */
  function formatTime(sec) {
    if (isNaN(sec) || sec === null || sec === undefined) return '00:00.000';
    const s = Math.max(0, sec);
    const hrs = Math.floor(s / 3600);
    const mins = Math.floor((s % 3600) / 60);
    const secs = (s % 60).toFixed(3);
    const pad = (n, len = 2) => String(n).padStart(len, '0');
    if (hrs > 0) {
      return `${pad(hrs)}:${pad(mins)}:${pad(secs, 6)}`;
    }
    return `${pad(mins)}:${pad(secs, 6)}`;
  }

  /**
   * Append log message
   */
  function log(tag, message, level = 'info') {
    const el = document.createElement('div');
    el.className = `log-entry log-${level}`;
    const timeStr = new Date().toISOString().substring(11, 23);
    el.textContent = `[${timeStr}] [${tag}] ${message}`;
    logStream.appendChild(el);
    logStream.scrollTop = logStream.scrollHeight;
  }

  /**
   * Post command message to bridge iframe
   */
  function sendCommand(action, payload = {}) {
    if (!iframe || !iframe.contentWindow) return;
    const msg = {
      action: action,
      ...payload
    };
    iframe.contentWindow.postMessage(msg, '*');
    log('IPC_TX', JSON.stringify(msg));
  }

  /**
   * Reset candidate state
   */
  function resetMetrics(videoId) {
    activeVideoId = videoId;
    metricVideoId.textContent = videoId;
    bridgeReady = false;
    playerReady = false;
    currentState = 'UNSTARTED';
    metricState.textContent = 'UNSTARTED';
    metricState.className = 'badge';
    metricTimeBridgeReady.textContent = '-- ms';
    metricTimePlayerReady.textContent = '-- ms';
    metricTimeToPlay.textContent = '-- ms';
    metricUpdateRate.textContent = '-- events/s';
    metricSeekLatency.textContent = '-- ms';
    metricError.textContent = 'None';
    metricDiagnosis.textContent = 'Testing...';
    metricDiagnosis.className = 'badge';
    lastError = null;
    lastSeekLatency = null;
    tBridgeReady = 0;
    tPlayerReady = 0;
    tFirstPlay = 0;
    seekSentTime = 0;
    targetSeekOffset = null;
    updateCount = 0;
    tLoadStart = performance.now();

    // Hide fallback view
    fallbackView.classList.add('hidden');
  }

  /**
   * Load video by changing iframe src or postMessage
   */
  function loadVideo(videoId, startSec = 0, autoplay = 0) {
    resetMetrics(videoId);
    log('HARNESS', `Loading video: ${videoId} (start: ${startSec}s, autoplay: ${autoplay})`);
    // Reload iframe src to test cold bridge startup per candidate
    iframe.src = `${BRIDGE_ORIGIN}/?v=${videoId}&start=${startSec}&autoplay=${autoplay}`;
  }

  /**
   * Calculate time update rate once per second
   */
  setInterval(() => {
    const now = Date.now();
    const elapsed = (now - lastUpdateSec) / 1000;
    if (elapsed >= 0.9) {
      timeUpdateRate = (updateCount / elapsed).toFixed(1);
      metricUpdateRate.textContent = `${timeUpdateRate} events/s`;
      updateCount = 0;
      lastUpdateSec = now;
    }
  }, 1000);

  /**
   * Handle incoming messages from bridge iframe
   */
  window.addEventListener('message', function (event) {
    // Only accept messages from bridge
    const data = event.data;
    if (!data || typeof data !== 'object') return;
    if (data.source !== 'clipper-bridge') return;

    log('IPC_RX', `Type: ${data.type} | ${JSON.stringify(data)}`);

    const now = performance.now();

    switch (data.type) {
      case 'bridgeReady': {
        bridgeReady = true;
        tBridgeReady = Math.round(now - tLoadStart);
        metricTimeBridgeReady.textContent = `${tBridgeReady} ms`;
        log('METRIC', `Bridge Ready in ${tBridgeReady} ms`);
        break;
      }

      case 'playerReady': {
        playerReady = true;
        tPlayerReady = Math.round(now - tLoadStart);
        currentDuration = data.duration || 0;
        metricTimePlayerReady.textContent = `${tPlayerReady} ms`;
        scrubberSlider.max = currentDuration > 0 ? currentDuration : 100;
        timeReadout.textContent = `${formatTime(currentTime)} / ${formatTime(currentDuration)}`;
        metricDiagnosis.textContent = 'Player initialized successfully';
        metricDiagnosis.className = 'badge badge-accent';
        log('METRIC', `Player Ready in ${tPlayerReady} ms (duration: ${currentDuration}s)`);
        break;
      }

      case 'stateChange': {
        currentState = data.state;
        metricState.textContent = currentState;
        if (currentState === 'PLAYING') {
          metricState.className = 'badge badge-accent';
          if (tFirstPlay === 0 && tLoadStart > 0) {
            tFirstPlay = Math.round(now - tLoadStart);
            metricTimeToPlay.textContent = `${tFirstPlay} ms`;
            log('METRIC', `First playback transition in ${tFirstPlay} ms`);
          }
        } else if (currentState === 'PAUSED') {
          metricState.className = 'badge badge-warning';
        } else if (currentState === 'BUFFERING') {
          metricState.className = 'badge badge-info';
        }
        break;
      }

      case 'timeUpdate': {
        updateCount++;
        currentTime = data.currentTime || 0;
        if (data.duration && data.duration > 0) {
          currentDuration = data.duration;
          scrubberSlider.max = currentDuration;
        }
        scrubberSlider.value = currentTime;
        timeReadout.textContent = `${formatTime(currentTime)} / ${formatTime(currentDuration)}`;

        // Check if fulfilling a seek measurement
        if (targetSeekOffset !== null && seekSentTime > 0) {
          const diff = Math.abs(currentTime - targetSeekOffset);
          // If within 2 seconds of target or time jumped significantly past target
          if (diff <= 2.5 || (targetSeekOffset >= 10000 && currentTime >= 10790)) {
            lastSeekLatency = Math.round(now - seekSentTime);
            metricSeekLatency.textContent = `${lastSeekLatency} ms (to ${Math.round(currentTime)}s)`;
            log('METRIC', `Seek fulfilled in ${lastSeekLatency} ms to offset ${currentTime}s`);
            targetSeekOffset = null;
            seekSentTime = 0;
          }
        }
        break;
      }

      case 'error': {
        lastError = data;
        metricError.textContent = `${data.errorType} (Code ${data.code})`;
        metricState.textContent = 'ERROR';
        metricState.className = 'badge badge-danger';

        // Check if Group (a) known-embeddable controls succeeded
        const knownControlsPassed = matrixResults['WufkeDaJisQ']?.embedLoaded || matrixResults['3snfJrlSyqo']?.embedLoaded;

        if (data.code === 101 || data.code === 150) {
          if (knownControlsPassed) {
            metricDiagnosis.textContent = 'Confirmed: Channel Owner Restriction (Not Origin Error)';
            metricDiagnosis.className = 'badge badge-warning';
          } else {
            metricDiagnosis.textContent = 'Restriction reported (Verifying origin against Group a)';
            metricDiagnosis.className = 'badge badge-warning';
          }
          // Activate Fallback UX per PRD
          triggerFallbackView(activeVideoId, data);
        } else {
          metricDiagnosis.textContent = `Player Error ${data.code}`;
          metricDiagnosis.className = 'badge badge-danger';
        }

        log('ERROR', `Player Error received: Code ${data.code} (${data.errorType})`, 'warn');
        break;
      }
    }
  });

  /**
   * Activate Fallback UX
   */
  function triggerFallbackView(videoId, errData) {
    fallbackView.classList.remove('hidden');
    fallbackDuration.textContent = currentDuration > 0 ? formatTime(currentDuration) : 'Unavailable';
    log('FALLBACK', `Fallback UX activated for ${videoId}: ${errData.title}`);
  }

  dismissFallbackBtn.addEventListener('click', () => {
    fallbackView.classList.add('hidden');
    sendCommand('play');
  });

  // UI Event Handlers
  playBtn.addEventListener('click', () => sendCommand('play'));
  pauseBtn.addEventListener('click', () => sendCommand('pause'));
  seekBack5Btn.addEventListener('click', () => {
    const target = Math.max(0, currentTime - 5);
    triggerSeek(target);
  });
  seekFwd5Btn.addEventListener('click', () => {
    const target = currentTime + 5;
    triggerSeek(target);
  });
  seek30Btn.addEventListener('click', () => triggerSeek(30));
  seek300Btn.addEventListener('click', () => triggerSeek(300));
  seek3hBtn.addEventListener('click', () => triggerSeek(10800)); // ~3h

  scrubberSlider.addEventListener('input', (e) => {
    const val = parseFloat(e.target.value);
    timeReadout.textContent = `${formatTime(val)} / ${formatTime(currentDuration)}`;
  });
  scrubberSlider.addEventListener('change', (e) => {
    const val = parseFloat(e.target.value);
    triggerSeek(val);
  });

  muteToggleBtn.addEventListener('click', () => {
    if (isMuted) {
      sendCommand('unMute');
      isMuted = false;
      muteToggleBtn.textContent = '🔊';
    } else {
      sendCommand('mute');
      isMuted = true;
      muteToggleBtn.textContent = '🔇';
    }
  });

  volumeSlider.addEventListener('input', (e) => {
    const vol = parseInt(e.target.value, 10);
    sendCommand('setVolume', { volume: vol });
  });

  function triggerSeek(targetSec) {
    targetSeekOffset = targetSec;
    seekSentTime = performance.now();
    metricSeekLatency.textContent = 'seeking...';
    sendCommand('seekTo', { seconds: targetSec, allowSeekAhead: true });
    log('TRANSPORT', `Seeking to ${targetSec}s`);
  }

  // Candidate Selector Buttons
  candButtons.forEach(btn => {
    btn.addEventListener('click', () => {
      candButtons.forEach(b => b.classList.remove('active'));
      btn.classList.add('active');
      const vid = btn.dataset.id;
      const cand = CANDIDATES.find(c => c.id === vid) || { id: vid, label: btn.dataset.title, group: btn.dataset.group };
      activeCandidate = cand;
      loadVideo(vid, 0, 1);
    });
  });

  loadCustomBtn.addEventListener('click', () => {
    let raw = customVideoInput.value.trim();
    if (!raw) return;
    // Extract video ID if URL provided
    let vid = raw;
    if (raw.includes('v=')) {
      vid = raw.split('v=')[1].split('&')[0];
    } else if (raw.includes('youtu.be/')) {
      vid = raw.split('youtu.be/')[1].split('?')[0];
    }
    const customCand = { id: vid, group: '(b)', label: `Custom: ${vid}` };
    activeCandidate = customCand;
    candButtons.forEach(b => b.classList.remove('active'));
    loadVideo(vid, 0, 1);
  });

  /**
   * Run Single Candidate Test in Automated Sequence
   */
  async function testCandidate(cand) {
    return new Promise((resolve) => {
      log('TEST_RUNNER', `=== Starting test for ${cand.id} [Group ${cand.group}: ${cand.label}] ===`);
      activeCandidate = cand;
      loadVideo(cand.id, 0, 1);

      const result = {
        id: cand.id,
        group: cand.group,
        label: cand.label,
        embedLoaded: false,
        errorCode: null,
        errorType: null,
        timeToBridgeReadyMs: null,
        timeToPlayerReadyMs: null,
        timeToFirstPlayMs: null,
        timeUpdateRate: null,
        seek30sLatencyMs: null,
        seek3hLatencyMs: null,
        fallbackTriggered: false,
        diagnosis: 'Pending'
      };

      // Watch for ready or error for up to 8 seconds
      const pollStart = Date.now();
      const interval = setInterval(async () => {
        const elapsed = (Date.now() - pollStart) / 1000;

        // Check for error
        if (lastError) {
          clearInterval(interval);
          result.embedLoaded = false;
          result.errorCode = lastError.code;
          result.errorType = lastError.errorType;
          result.timeToBridgeReadyMs = tBridgeReady || null;
          result.fallbackTriggered = !fallbackView.classList.contains('hidden');

          // Check if known controls passed previously
          const knownControlsPassed = matrixResults['WufkeDaJisQ']?.embedLoaded || matrixResults['3snfJrlSyqo']?.embedLoaded;
          if (lastError.code === 101 || lastError.code === 150) {
            result.diagnosis = knownControlsPassed
              ? 'Owner restricted (confirmed by baseline)'
              : 'Owner restriction / origin check';
          } else {
            result.diagnosis = `Error ${lastError.code} (${lastError.errorType})`;
          }

          log('TEST_RUNNER', `Candidate ${cand.id} ended in error: ${result.diagnosis}`);
          recordCandidateResult(result);
          resolve(result);
          return;
        }

        // Check for playerReady
        if (playerReady && (currentState === 'PLAYING' || currentState === 'BUFFERING' || currentState === 'CUED')) {
          clearInterval(interval);
          result.embedLoaded = true;
          result.timeToBridgeReadyMs = tBridgeReady;
          result.timeToPlayerReadyMs = tPlayerReady;
          result.timeToFirstPlayMs = tFirstPlay || null;
          result.diagnosis = 'Embed Loaded & Verified';

          // Ensure muted for smooth automated autoplay compliance
          sendCommand('mute');
          sendCommand('play');
          await wait(1000);
          result.timeUpdateRate = parseFloat(timeUpdateRate) || null;

          triggerSeek(30);
          await wait(2500);
          result.seek30sLatencyMs = lastSeekLatency;

          // If test3hSeek is flagged, test ~3h (10800s) seek
          if (cand.test3hSeek && cand.duration && cand.duration > 11000) {
            log('TEST_RUNNER', `Testing ~3h seek (10,800s) on long video ${cand.id}...`);
            triggerSeek(10800);
            await wait(3500);
            result.seek3hLatencyMs = lastSeekLatency;
          }

          log('TEST_RUNNER', `Candidate ${cand.id} test complete.`);
          recordCandidateResult(result);
          resolve(result);
          return;
        }

        // Timeout fallback after 10 seconds
        if (elapsed > 10) {
          clearInterval(interval);
          result.embedLoaded = playerReady;
          result.timeToBridgeReadyMs = tBridgeReady || null;
          result.timeToPlayerReadyMs = tPlayerReady || null;
          result.diagnosis = playerReady ? 'Loaded (Timeout awaiting play)' : 'Timeout awaiting ready';
          log('TEST_RUNNER', `Candidate ${cand.id} timed out after 10s`);
          recordCandidateResult(result);
          resolve(result);
        }
      }, 250);
    });
  }

  function wait(ms) {
    return new Promise(r => setTimeout(r, ms));
  }

  function recordCandidateResult(res) {
    matrixResults[res.id] = res;
    renderMatrixResults();
  }

  function renderMatrixResults() {
    const keys = Object.keys(matrixResults);
    matrixCompletedCount.textContent = keys.length;
    matrixResultsList.innerHTML = '';

    keys.forEach(k => {
      const r = matrixResults[k];
      const card = document.createElement('div');
      card.className = `matrix-item ${r.embedLoaded ? 'item-pass' : 'item-fail'}`;
      card.innerHTML = `
        <div class="matrix-item-head">
          <strong>${r.group} ${r.id}</strong>: ${r.label}
          <span class="badge ${r.embedLoaded ? 'badge-accent' : 'badge-warning'}">${r.embedLoaded ? 'LOADED' : (r.errorCode ? `ERR ${r.errorCode}` : 'FAIL')}</span>
        </div>
        <div class="matrix-item-body">
          <span>Bridge: ${r.timeToBridgeReadyMs || '--'}ms</span> | 
          <span>Player: ${r.timeToPlayerReadyMs || '--'}ms</span> | 
          <span>Seek: ${r.seek30sLatencyMs || '--'}ms</span> ${r.seek3hLatencyMs ? `| <span>Seek 3h: ${r.seek3hLatencyMs}ms</span>` : ''} |
          <span>Diag: <em>${r.diagnosis}</em></span>
        </div>
      `;
      matrixResultsList.appendChild(card);
    });
  }

  /**
   * Run full matrix over all candidates sequentially
   */
  async function runFullMatrix() {
    runAutoMatrixBtn.disabled = true;
    runAutoMatrixBtn.textContent = '⏳ Running Matrix...';
    log('TEST_RUNNER', '========== STARTING AUTOMATED MATRIX RUN ==========');

    for (const cand of CANDIDATES) {
      await testCandidate(cand);
      await wait(1500); // cooldown between candidates
    }

    runAutoMatrixBtn.disabled = false;
    runAutoMatrixBtn.textContent = '▶ Re-run Full Automated Matrix';
    log('TEST_RUNNER', '========== AUTOMATED MATRIX FINISHED ==========');

    // Persist results to server
    saveResultsToServer();
  }

  /**
   * Save results payload via POST to server
   */
  async function saveResultsToServer() {
    try {
      const payload = {
        timestamp: new Date().toISOString(),
        hostOrigin: window.location.origin,
        bridgeOrigin: BRIDGE_ORIGIN,
        results: matrixResults
      };

      const res = await fetch('/api/save-results', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload, null, 2)
      });
      if (res.ok) {
        log('SAVE', 'Results successfully saved to server (spikes/s2-preview/results/matrix_results.json)');
      } else {
        log('SAVE', `Server save responded with status ${res.status}`, 'warn');
      }
    } catch (e) {
      log('SAVE', `Failed to post results to /api/save-results: ${e.message}`, 'warn');
    }
  }

  // Export Results Button
  exportResultsBtn.addEventListener('click', () => {
    const dataStr = 'data:text/json;charset=utf-8,' + encodeURIComponent(JSON.stringify(matrixResults, null, 2));
    const a = document.createElement('a');
    a.href = dataStr;
    a.download = `s2_matrix_results_${Date.now()}.json`;
    a.click();
  });

  runAutoMatrixBtn.addEventListener('click', runFullMatrix);

  // Auto-run if requested in URL query ?autotest=1
  const searchParams = new URLSearchParams(window.location.search);
  if (searchParams.get('autotest') === '1') {
    setTimeout(runFullMatrix, 1000);
  } else {
    // Initial load of default video
    loadVideo(activeVideoId, 0, 0);
  }

})();
