/**
 * Reference Transcript Review & Human Correction Tool
 * Operates offline on 60s sample WAV files and draft references.
 */

// Application State
let currentSampleId = 'gaming_live';
let activeLineIndex = 0;
let isLoopingLine = false;
let playbackSpeed = 1.0;
let userEdits = {
  gaming_live: {},
  gaming_slang: {},
  podcast_mixed: {}
};

// DOM Elements
const audioPlayer = document.getElementById('audioPlayer');
const sampleTitle = document.getElementById('sampleTitle');
const sampleBadge = document.getElementById('sampleBadge');
const linesList = document.getElementById('linesList');
const timeDisplay = document.getElementById('timeDisplay');
const scrubSlider = document.getElementById('scrubSlider');
const playPauseBtn = document.getElementById('playPauseBtn');
const loopBtn = document.getElementById('loopBtn');
const saveBtn = document.getElementById('saveBtn');
const toast = document.getElementById('toast');

// Initialize
function init() {
  setupTabs();
  setupAudioListeners();
  setupGlobalControls();
  setupShortcuts();
  loadSample(currentSampleId);
}

// Tab Switching
function setupTabs() {
  document.querySelectorAll('.tab-btn').forEach(btn => {
    btn.addEventListener('click', (e) => {
      const sampleId = e.target.dataset.sample;
      if (sampleId !== currentSampleId) {
        document.querySelectorAll('.tab-btn').forEach(b => b.classList.remove('active'));
        e.target.classList.add('active');
        loadSample(sampleId);
      }
    });
  });
}

// Load Selected Sample
function loadSample(sampleId) {
  currentSampleId = sampleId;
  const sample = DRAFT_SAMPLES[sampleId];
  if (!sample) return;

  sampleTitle.textContent = sample.title;
  sampleBadge.textContent = `${sample.segments.length} segments`;

  // Set audio source with fallback handling
  audioPlayer.src = sample.audioFile;
  audioPlayer.playbackRate = playbackSpeed;
  audioPlayer.load();

  audioPlayer.onerror = () => {
    console.warn(`Primary audio path ${sample.audioFile} failed. Trying fallback ${sample.fallbackAudio}...`);
    if (audioPlayer.src !== sample.fallbackAudio) {
      audioPlayer.src = sample.fallbackAudio;
      audioPlayer.load();
    }
  };

  // Render lines
  renderLines(sampleId);
  setActiveLine(0, false);
}

// Render Transcript Lines
function renderLines(sampleId) {
  const sample = DRAFT_SAMPLES[sampleId];
  linesList.innerHTML = '';

  sample.segments.forEach((seg, idx) => {
    const edit = userEdits[sampleId][idx] || {
      text: seg.text,
      skipped: false,
      modified: false
    };

    const card = document.createElement('div');
    card.className = `line-card ${idx === activeLineIndex ? 'active' : ''} ${edit.skipped ? 'skipped' : ''}`;
    card.id = `line-${idx}`;
    card.dataset.index = idx;

    const duration = (seg.end - seg.start).toFixed(1);

    card.innerHTML = `
      <div class="line-meta">
        <span class="line-tag" title="Click to play segment">
          ▶ ${seg.tag}
        </span>
        <span class="line-duration">${duration}s (${seg.start.toFixed(1)}s - ${seg.end.toFixed(1)}s)</span>
        <button class="btn btn-small line-play-btn" title="Play segment">
          Play Line
        </button>
      </div>
      <div class="line-body">
        <textarea class="text-input" id="input-${idx}" rows="2" placeholder="Enter corrected text...">${escapeHtml(edit.text)}</textarea>
        <div class="line-actions">
          <button class="btn btn-small btn-unintelligible" data-idx="${idx}" title="Insert [?] at cursor">
            + [?] (unintelligible)
          </button>
          <button class="btn btn-small btn-skip ${edit.skipped ? 'is-skipped' : ''}" data-idx="${idx}" title="Exclude this utterance from final reference">
            ${edit.skipped ? '✓ Skipped (Excluded)' : 'Skip Line'}
          </button>
          <button class="btn btn-small btn-reset" data-idx="${idx}" title="Reset to original draft">
            Reset
          </button>
          <span class="diff-indicator ${edit.modified ? 'visible' : ''}" id="diff-${idx}">
            ✏ Modified
          </span>
        </div>
      </div>
    `;

    // Event Listeners for this card
    const playBtn = card.querySelector('.line-play-btn');
    const tagBtn = card.querySelector('.line-tag');
    const textarea = card.querySelector('.text-input');
    const unintellBtn = card.querySelector('.btn-unintelligible');
    const skipBtn = card.querySelector('.btn-skip');
    const resetBtn = card.querySelector('.btn-reset');

    const playLineAction = () => {
      setActiveLine(idx, true);
    };

    playBtn.addEventListener('click', playLineAction);
    tagBtn.addEventListener('click', playLineAction);

    textarea.addEventListener('focus', () => {
      setActiveLine(idx, false);
    });

    textarea.addEventListener('input', (e) => {
      const val = e.target.value;
      const isMod = (val !== seg.text);
      userEdits[sampleId][idx] = {
        ...userEdits[sampleId][idx],
        text: val,
        modified: isMod
      };
      const diffInd = document.getElementById(`diff-${idx}`);
      if (diffInd) {
        diffInd.classList.toggle('visible', isMod);
      }
    });

    unintellBtn.addEventListener('click', () => {
      insertAtCursor(textarea, '[?]');
      textarea.dispatchEvent(new Event('input'));
      textarea.focus();
    });

    skipBtn.addEventListener('click', () => {
      const curr = userEdits[sampleId][idx] || { text: seg.text, skipped: false, modified: false };
      curr.skipped = !curr.skipped;
      userEdits[sampleId][idx] = curr;
      card.classList.toggle('skipped', curr.skipped);
      skipBtn.classList.toggle('is-skipped', curr.skipped);
      skipBtn.textContent = curr.skipped ? '✓ Skipped (Excluded)' : 'Skip Line';
    });

    resetBtn.addEventListener('click', () => {
      textarea.value = seg.text;
      userEdits[sampleId][idx] = {
        text: seg.text,
        skipped: false,
        modified: false
      };
      card.classList.remove('skipped');
      skipBtn.classList.remove('is-skipped');
      skipBtn.textContent = 'Skip Line';
      const diffInd = document.getElementById(`diff-${idx}`);
      if (diffInd) diffInd.classList.remove('visible');
    });

    linesList.appendChild(card);
  });
}

// Active Line Management
function setActiveLine(idx, play = false) {
  const sample = DRAFT_SAMPLES[currentSampleId];
  if (!sample || idx < 0 || idx >= sample.segments.length) return;

  activeLineIndex = idx;

  document.querySelectorAll('.line-card').forEach((c, i) => {
    c.classList.toggle('active', i === idx);
  });

  const seg = sample.segments[idx];
  if (play) {
    audioPlayer.currentTime = seg.start;
    audioPlayer.play().catch(e => console.log('Audio play interrupted:', e));
  }
}

// Audio Engine Listeners
function setupAudioListeners() {
  audioPlayer.addEventListener('timeupdate', () => {
    const cur = audioPlayer.currentTime;
    const dur = audioPlayer.duration || 60;

    // Update time readout and slider
    timeDisplay.textContent = `${formatTime(cur)} / ${formatTime(dur)}`;
    scrubSlider.value = (cur / dur) * 100;

    // Loop logic
    if (isLoopingLine) {
      const sample = DRAFT_SAMPLES[currentSampleId];
      if (sample && sample.segments[activeLineIndex]) {
        const seg = sample.segments[activeLineIndex];
        if (cur >= seg.end || cur < seg.start - 0.2) {
          audioPlayer.currentTime = seg.start;
          audioPlayer.play();
        }
      }
    }
  });

  audioPlayer.addEventListener('play', () => {
    playPauseBtn.innerHTML = '⏸ Pause';
  });

  audioPlayer.addEventListener('pause', () => {
    playPauseBtn.innerHTML = '▶ Play';
  });

  audioPlayer.addEventListener('ended', () => {
    playPauseBtn.innerHTML = '▶ Play';
  });
}

// Global UI Controls
function setupGlobalControls() {
  // Play / Pause
  playPauseBtn.addEventListener('click', () => {
    if (audioPlayer.paused) {
      audioPlayer.play();
    } else {
      audioPlayer.pause();
    }
  });

  // Loop Line Toggle
  loopBtn.addEventListener('click', () => {
    isLoopingLine = !isLoopingLine;
    loopBtn.classList.toggle('btn-active', isLoopingLine);
    loopBtn.textContent = isLoopingLine ? '🔁 Loop Line: ON' : '🔁 Loop Line: OFF';
    showToast(isLoopingLine ? 'Looping current line enabled' : 'Looping disabled');
  });

  // Scrub Slider
  scrubSlider.addEventListener('input', (e) => {
    const pct = parseFloat(e.target.value);
    const dur = audioPlayer.duration || 60;
    audioPlayer.currentTime = (pct / 100) * dur;
  });

  // Playback Speed Controls
  document.querySelectorAll('.speed-btn').forEach(btn => {
    btn.addEventListener('click', (e) => {
      document.querySelectorAll('.speed-btn').forEach(b => b.classList.remove('active'));
      e.target.classList.add('active');
      playbackSpeed = parseFloat(e.target.dataset.speed);
      audioPlayer.playbackRate = playbackSpeed;
    });
  });

  // Save Reference Button
  saveBtn.addEventListener('click', async () => {
    await saveCurrentReference();
  });
}

// Save Reference Handler (File System Access API with Download Fallback)
async function saveCurrentReference() {
  const sample = DRAFT_SAMPLES[currentSampleId];
  if (!sample) return;

  const outputLines = [];
  sample.segments.forEach((seg, idx) => {
    const edit = userEdits[currentSampleId][idx] || {
      text: seg.text,
      skipped: false
    };

    // If not skipped, include in output
    if (!edit.skipped) {
      const cleanText = edit.text.trim();
      outputLines.push(`${seg.tag} ${cleanText}`);
    }
  });

  const content = outputLines.join('\n') + '\n';
  const defaultFilename = sample.defaultOutput; // e.g. final_gaming_live.txt

  try {
    // Attempt File System Access API (Chrome, Edge, Opera)
    if ('showSaveFilePicker' in window) {
      const handle = await window.showSaveFilePicker({
        suggestedName: defaultFilename,
        types: [{
          description: 'Reference Transcript (*.txt)',
          accept: { 'text/plain': ['.txt'] }
        }]
      });

      const writable = await handle.createWritable();
      await writable.write(new Blob([content], { type: 'text/plain;charset=utf-8' }));
      await writable.close();

      showToast(`Saved successfully as ${handle.name}!`, 'success');
      return;
    }
  } catch (err) {
    if (err.name === 'AbortError') {
      showToast('Save cancelled.', 'error');
      return;
    }
    console.warn('showSaveFilePicker error or unavailable, falling back to download:', err);
  }

  // Fallback: Direct Browser Download
  try {
    const blob = new Blob([content], { type: 'text/plain;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = defaultFilename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
    showToast(`Downloaded ${defaultFilename} (move to spikes/s3-speech/references/)`, 'success');
  } catch (err) {
    alert(`Error saving file: ${err.message}`);
  }
}

// Keyboard Shortcuts
function setupShortcuts() {
  document.addEventListener('keydown', (e) => {
    // Ctrl + S: Save
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
      e.preventDefault();
      saveCurrentReference();
      return;
    }

    // Alt + L: Toggle Loop
    if (e.altKey && e.key.toLowerCase() === 'l') {
      e.preventDefault();
      loopBtn.click();
      return;
    }

    // Alt + Space or Ctrl + Space: Play/Pause current segment
    if ((e.altKey || e.ctrlKey) && e.code === 'Space') {
      e.preventDefault();
      if (audioPlayer.paused) {
        const seg = DRAFT_SAMPLES[currentSampleId].segments[activeLineIndex];
        audioPlayer.currentTime = seg.start;
        audioPlayer.play();
      } else {
        audioPlayer.pause();
      }
      return;
    }

    // Alt + ?: Insert [?] into active textarea
    if (e.altKey && (e.key === '?' || e.key === '/')) {
      const textarea = document.getElementById(`input-${activeLineIndex}`);
      if (textarea) {
        e.preventDefault();
        insertAtCursor(textarea, '[?]');
        textarea.dispatchEvent(new Event('input'));
      }
      return;
    }

    // Alt + Up / Alt + Down: Navigate lines
    if (e.altKey && (e.key === 'ArrowUp' || e.key === 'ArrowDown')) {
      e.preventDefault();
      const delta = e.key === 'ArrowUp' ? -1 : 1;
      const newIdx = Math.max(0, Math.min(DRAFT_SAMPLES[currentSampleId].segments.length - 1, activeLineIndex + delta));
      setActiveLine(newIdx, true);
      const input = document.getElementById(`input-${newIdx}`);
      if (input) {
        input.focus();
        input.scrollIntoView({ behavior: 'smooth', block: 'center' });
      }
    }
  });
}

// Utility: Insert text at cursor in textarea
function insertAtCursor(textarea, text) {
  const start = textarea.selectionStart;
  const end = textarea.selectionEnd;
  const val = textarea.value;
  textarea.value = val.substring(0, start) + text + val.substring(end);
  textarea.selectionStart = textarea.selectionEnd = start + text.length;
}

// Utility: Format seconds to mm:ss.sss
function formatTime(sec) {
  if (isNaN(sec)) return '00:00.000';
  const m = Math.floor(sec / 60);
  const s = sec % 60;
  return `${String(m).padStart(2, '0')}:${s.toFixed(3).padStart(6, '0')}`;
}

// Utility: Escape HTML
function escapeHtml(str) {
  return str
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#039;');
}

// Toast notification
let toastTimer = null;
function showToast(msg, type = 'info') {
  clearTimeout(toastTimer);
  toast.textContent = msg;
  toast.className = `toast show ${type}`;
  toastTimer = setTimeout(() => {
    toast.className = 'toast';
  }, 3500);
}

// Run on load
document.addEventListener('DOMContentLoaded', init);
