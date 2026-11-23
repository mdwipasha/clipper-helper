# Spike S2: Manual Verification Checklist for Human Inspection

**Instructions for Reviewer:**
Please open `http://127.0.0.1:8765` in your browser to interactively test the candidates in the harness. Inspect the visual presentation, ad behavior, controls overlap, and audio. Fill in the empty cells below with your observations (`Pass`, `Fail`, `Yes`, `No`, or descriptive notes). Do not alter the test candidate descriptions.

---

## 1. Candidate Visual & Audio Verification Matrix

| Candidate & Group | Target ID / URL | Test Case / Inspection Point | Ads Shown? | `currentTime` / `duration` During Ads | Visual Glitches / Controls Overlap | Audio Playback & Sync | Status / Notes |
|---|---|---|---|---|---|---|---|
| **(a) Short 10m** | `WufkeDaJisQ` | Cold start playback, 30s seek, transport controls | | | | | |
| **(a) Medium 40m** | `3snfJrlSyqo` | Mid-length playback, scrubber drag, pause/play toggles | | | | | |
| **(a) Live VOD 10h** | `XwQwhjzAc_k` | Marathon timeline scrubber smoothness, audio sync | | | | | |
| **(c)(d) Monetized 3h47m** | `9s6imDGjy08` | Pre-roll / mid-roll ad occurrence & behavior | | | | | |
| **(c)(d) Monetized 3h47m** | `9s6imDGjy08` | Scrubber jump to ~3h offset (10,800s) | | | | | |
| **(d) Long VOD 5.5h** | `QOJIILaJKNc` | Rapid seeking (t=100s, 5000s, 15000s) | | | | | |
| **(d) Long VOD 5.5h** | `QOJIILaJKNc` | Long seek (~3h / 10,800s) visual & audio recovery | | | | | |
| **(e) Members-Only** | `KBdGeakRgqg` | Error overlay display & Fallback UX card transition | | | | | |
| **(e) Age-Restricted** | `6kLq3WMV1nU` | Error overlay display & Fallback UX card transition | | | | | |
| **(b) Embed-Disabled** | *(User URL)* | Owner-disabled restriction & Fallback scrubber UX | | | | | |

---

## 2. Detailed Inspection Guidelines

### A. Group (c)(d) Monetized Long VOD (`9s6imDGjy08` - IShowSpeed)
- **Ads Served:** Record whether YouTube serves a pre-roll or mid-roll video ad.
  - Cell `Ads Shown?`: `[ ] Yes  /  [ ] No`
- **Timecode Behavior During Ads:**
  - Does `currentTime` report the ad's elapsed time, stay frozen at 0, or show the main video time?
  - Does `duration` report the ad's length (e.g. 15s/30s) or the 13,668s stream length?
  - Cell `currentTime / duration During Ads`: `[                                                ]`
- **Transition Smoothness:**
  - When the ad ends or is skipped, does playback resume cleanly without black screens or frozen frames?
  - Cell `Visual Glitches / Controls Overlap`: `[                                                ]`

### B. Group (d) Long VOD Seeking (`QOJIILaJKNc` - 5h28m)
- **Rapid Scrubbing:**
  - Click `Go 30s`, `Go 5m`, and `Go ~3h (10800s)`.
  - Is there any controls flicker, canvas tearing, or player stall?
  - Cell `Visual Glitches`: `[                                                ]`
- **Audio Resync:**
  - Does audio resume in sync with the video frames after a 3-hour seek?
  - Cell `Audio Playback & Sync`: `[                                                ]`

### C. Group (e) & (b) Fallback UX Inspection
- **Error Presentation:**
  - For `KBdGeakRgqg` and `6kLq3WMV1nU`, does the bridge display the Indonesian error overlay (`E_YT_EMBED_RESTRICTED`, Code 150)?
  - Does the host application smoothly display the `#fallbackView` card?
  - Does the fallback view prevent dead-ends by clearly stating the restriction and offering storyboard browsing?
  - Cell `Status / Notes`: `[                                                ]`
