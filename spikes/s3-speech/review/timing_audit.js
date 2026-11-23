// Timing Audit Review Script (Spike S3d)
// Deterministic 30 words per sample, snippet playback [t0 - 0.3s, t0 + 0.6s], no pre-fill

(function() {
  let currentSampleId = "gaming_live";
  const userRatings = {
    gaming_live: {},
    gaming_slang: {},
    podcast_mixed: {}
  };

  const audio = document.getElementById("snippetAudio");
  const tabBtns = document.querySelectorAll(".tab-btn[data-sample]");
  const sampleTitleEl = document.getElementById("sampleTitle");
  const auditListEl = document.getElementById("auditWordsList");
  const progressBadge = document.getElementById("auditProgressBadge");
  const saveBtn = document.getElementById("saveAuditBtn");
  const toastEl = document.getElementById("toast");

  let stopTimer = null;

  function showToast(msg, isError = false) {
    toastEl.textContent = msg;
    toastEl.style.borderColor = isError ? "var(--danger)" : "var(--accent)";
    toastEl.classList.add("visible");
    setTimeout(() => {
      toastEl.classList.remove("visible");
    }, 2800);
  }

  function formatTime(s) {
    const mins = Math.floor(s / 60);
    const secs = (s % 60).toFixed(3);
    return `${String(mins).padStart(2, "0")}:${String(secs).padStart(6, "0")}`;
  }

  function playSnippet(startS, endS) {
    if (stopTimer) {
      clearTimeout(stopTimer);
      stopTimer = null;
    }
    audio.pause();
    audio.currentTime = Math.max(0, startS);
    audio.play().then(() => {
      const durMs = Math.max(100, (endS - startS) * 1000);
      stopTimer = setTimeout(() => {
        audio.pause();
      }, durMs);
    }).catch(err => {
      console.error("Audio play failed:", err);
      showToast("Audio playback failed: " + err.message, true);
    });
  }

  function updateProgress() {
    const ratings = userRatings[currentSampleId];
    const count = Object.keys(ratings).filter(k => ratings[k].choice).length;
    progressBadge.textContent = `${count} / 30 Audited`;
    if (count === 30) {
      progressBadge.className = "badge badge-success";
      progressBadge.style.backgroundColor = "var(--success)";
      progressBadge.style.color = "#fff";
    } else {
      progressBadge.className = "badge";
      progressBadge.style.backgroundColor = "var(--border-color)";
      progressBadge.style.color = "var(--text-secondary)";
    }
  }

  function renderWords() {
    const sample = AUDIT_DATA[currentSampleId];
    if (!sample) return;

    sampleTitleEl.textContent = `${sample.title} [30 sampled words, seed=42]`;
    audio.src = sample.audio;

    auditListEl.innerHTML = "";
    const sampleRatings = userRatings[currentSampleId];

    sample.words.forEach(w => {
      const card = document.createElement("div");
      card.className = "word-card" + (sampleRatings[w.rank]?.choice ? " audited" : "");
      card.id = `card_${w.rank}`;

      // Highlight target word in context
      const formattedCtx = w.context.replace(
        `[${w.word}]`,
        `<mark>[${w.word}]</mark>`
      );

      const savedChoice = sampleRatings[w.rank]?.choice || "";

      card.innerHTML = `
        <div class="word-header">
          <div class="word-title">
            <span>#${w.rank}. "${w.word}"</span>
            <span class="badge">${w.timingSource.toUpperCase()}</span>
          </div>
          <div class="word-timing">
            t0: ${formatTime(w.t0_s)} (${w.t0_ms} ms) &bull; Dur: ${w.dur_ms} ms
          </div>
        </div>

        <div class="word-context">
          ${formattedCtx}
        </div>

        <div class="audit-controls">
          <button class="btn btn-primary snippet-play-btn" data-start="${w.snippet_start_s}" data-end="${w.snippet_end_s}">
            ▶ Play Snippet (${w.snippet_start_s}s - ${w.snippet_end_s}s)
          </button>

          <div class="radio-group">
            <label class="radio-label">
              <input type="radio" name="rating_${w.rank}" value="Early" ${savedChoice === "Early" ? "checked" : ""}>
              <span>Early</span>
            </label>
            <label class="radio-label">
              <input type="radio" name="rating_${w.rank}" value="OK" ${savedChoice === "OK" ? "checked" : ""}>
              <span>OK</span>
            </label>
            <label class="radio-label">
              <input type="radio" name="rating_${w.rank}" value="Late" ${savedChoice === "Late" ? "checked" : ""}>
              <span>Late</span>
            </label>
            <label class="radio-label">
              <input type="radio" name="rating_${w.rank}" value="Can't tell" ${savedChoice === "Can't tell" ? "checked" : ""}>
              <span>Can't tell</span>
            </label>
          </div>
        </div>
      `;

      // Event listeners for radio buttons
      card.querySelectorAll(`input[name="rating_${w.rank}"]`).forEach(radio => {
        radio.addEventListener("change", (e) => {
          if (!sampleRatings[w.rank]) {
            sampleRatings[w.rank] = {};
          }
          sampleRatings[w.rank].choice = e.target.value;
          sampleRatings[w.rank].word = w.word;
          sampleRatings[w.rank].t0_ms = w.t0_ms;
          sampleRatings[w.rank].dur_ms = w.dur_ms;
          card.classList.add("audited");
          updateProgress();
        });
      });

      // Snippet play button
      const playBtn = card.querySelector(".snippet-play-btn");
      playBtn.addEventListener("click", () => {
        playSnippet(w.snippet_start_s, w.snippet_end_s);
      });

      auditListEl.appendChild(card);
    });

    updateProgress();
  }

  // Switch sample tabs
  tabBtns.forEach(btn => {
    btn.addEventListener("click", () => {
      tabBtns.forEach(b => b.classList.remove("active"));
      btn.classList.add("active");
      currentSampleId = btn.getAttribute("data-sample");
      renderWords();
    });
  });

  // Save audit results via File System Access API with download fallback
  async function saveAuditResults() {
    const sample = AUDIT_DATA[currentSampleId];
    const sampleRatings = userRatings[currentSampleId];

    const results = {
      sample_id: currentSampleId,
      sample_title: sample.title,
      model: "small-q5_1",
      sampling_seed: 42,
      total_words_sampled: sample.words.length,
      audited_count: Object.keys(sampleRatings).filter(k => sampleRatings[k].choice).length,
      saved_at: new Date().toISOString(),
      items: sample.words.map(w => ({
        rank: w.rank,
        word: w.word,
        t0_ms: w.t0_ms,
        t1_ms: w.t1_ms,
        dur_ms: w.dur_ms,
        timingSource: w.timingSource,
        snippet: [w.snippet_start_s, w.snippet_end_s],
        context: w.context,
        choice: sampleRatings[w.rank]?.choice || "UNAUDITED"
      }))
    };

    const jsonStr = JSON.stringify(results, null, 2);
    const filename = `timing_audit_${currentSampleId}.json`;

    if ("showSaveFilePicker" in window) {
      try {
        const handle = await window.showSaveFilePicker({
          suggestedName: filename,
          types: [{
            description: "JSON Audit File",
            accept: { "application/json": [".json"] }
          }]
        });
        const writable = await handle.createWritable();
        await writable.write(jsonStr);
        await writable.close();
        showToast(`Saved ${filename} successfully!`);
        return;
      } catch (err) {
        if (err.name === "AbortError") return;
        console.warn("FSA API failed, falling back to download:", err);
      }
    }

    // Fallback: browser download
    const blob = new Blob([jsonStr], { type: "application/json;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
    showToast(`Downloaded ${filename}`);
  }

  saveBtn.addEventListener("click", saveAuditResults);

  // Initial render
  renderWords();
})();
