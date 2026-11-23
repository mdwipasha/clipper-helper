"""
Compare S3b capped detector vs uncapped detector vs valley-rise detector.
Analyze systematic bias and 12.24s drift root cause.
"""
import os
import wave
import json
import numpy as np

BASE_DIR = os.path.abspath(os.path.dirname(__file__))
AUDIO_DIR = os.path.join(BASE_DIR, "audio")
DTW_DIR = os.path.join(BASE_DIR, "results", "dtw_validation")

from whisper_to_words import convert_whisper_json_to_words

def load_wav(path):
    with wave.open(path, 'rb') as wf:
        data = wf.readframes(wf.getnframes())
        audio = np.frombuffer(data, dtype=np.int16).astype(np.float32) / 32768.0
        return audio, wf.getframerate()

def s3b_detector_capped(audio, sr, center_s, radius_s=0.25):
    """Verbatim implementation from S3b validate_dtw_onsets.py"""
    start_sample = max(0, int((center_s - radius_s) * sr))
    end_sample = min(len(audio), int((center_s + radius_s) * sr))
    segment = audio[start_sample:end_sample]
    if len(segment) == 0:
        return center_s

    frame_len = int(0.010 * sr)
    hop_len = int(0.002 * sr)
    num_frames = (len(segment) - frame_len) // hop_len
    if num_frames <= 1:
        return center_s

    energies = []
    times = []
    for i in range(num_frames):
        f = segment[i * hop_len : i * hop_len + frame_len]
        rms = np.sqrt(np.mean(f**2))
        energies.append(rms)
        t = (start_sample + i * hop_len + frame_len / 2) / sr
        times.append(t)

    energies = np.array(energies)
    times = np.array(times)

    floor = np.percentile(energies, 15)
    peak = np.max(energies)
    if peak <= floor * 1.2:
        diff = np.diff(energies)
        return times[np.argmax(diff)]

    threshold = floor + 0.15 * (peak - floor)
    above_th = np.where(energies >= threshold)[0]
    if len(above_th) > 0:
        # S3b CAP: <= 0.20
        valid_indices = [idx for idx in above_th if abs(times[idx] - center_s) <= 0.20]
        if valid_indices:
            onset_idx = valid_indices[0]
            while onset_idx > 0 and energies[onset_idx - 1] < energies[onset_idx] and energies[onset_idx] > floor:
                onset_idx -= 1
            return times[onset_idx]
        else:
            return times[above_th[0]]

    return times[np.argmin(np.abs(times - center_s))]

def s3b_detector_uncapped(audio, sr, center_s, radius_s=0.25):
    """Same as S3b but WITHOUT the 0.20 cap filter"""
    start_sample = max(0, int((center_s - radius_s) * sr))
    end_sample = min(len(audio), int((center_s + radius_s) * sr))
    segment = audio[start_sample:end_sample]
    if len(segment) == 0:
        return center_s

    frame_len = int(0.010 * sr)
    hop_len = int(0.002 * sr)
    num_frames = (len(segment) - frame_len) // hop_len
    if num_frames <= 1:
        return center_s

    energies = []
    times = []
    for i in range(num_frames):
        f = segment[i * hop_len : i * hop_len + frame_len]
        rms = np.sqrt(np.mean(f**2))
        energies.append(rms)
        t = (start_sample + i * hop_len + frame_len / 2) / sr
        times.append(t)

    energies = np.array(energies)
    times = np.array(times)

    floor = np.percentile(energies, 15)
    peak = np.max(energies)
    if peak <= floor * 1.2:
        diff = np.diff(energies)
        return times[np.argmax(diff)]

    threshold = floor + 0.15 * (peak - floor)
    above_th = np.where(energies >= threshold)[0]
    if len(above_th) > 0:
        onset_idx = above_th[0]
        while onset_idx > 0 and energies[onset_idx - 1] < energies[onset_idx] and energies[onset_idx] > floor:
            onset_idx -= 1
        return times[onset_idx]

    return times[np.argmin(np.abs(times - center_s))]

def valley_rise_detector(audio, sr, t0_s, max_lookback_s=0.30):
    """
    Continuous-speech onset detector: searches backward from DTW t0
    to find the inter-word energy valley, then finds the onset of the
    subsequent energy rise.
    """
    start_s = max(0, t0_s - max_lookback_s)
    end_s = min(len(audio)/sr, t0_s + 0.10)
    seg = audio[int(start_s*sr):int(end_s*sr)]
    if len(seg) < int(0.02*sr):
        return t0_s
    frame_len = int(0.010 * sr)
    hop_len = int(0.002 * sr)
    nf = (len(seg) - frame_len) // hop_len
    if nf < 2:
        return t0_s
    times = np.array([start_s + (i*hop_len + frame_len/2)/sr for i in range(nf)])
    rms = np.array([np.sqrt(np.mean(seg[i*hop_len:i*hop_len+frame_len]**2)) for i in range(nf)])

    # Valley in [t0_s - max_lookback_s, t0_s + 0.02]
    pre_idx = np.where(times <= t0_s + 0.02)[0]
    if len(pre_idx) < 2:
        return t0_s
    min_idx = pre_idx[np.argmin(rms[pre_idx])]
    valley_t = times[min_idx]
    valley_rms = rms[min_idx]

    # Peak in [valley_t, t0_s + 0.10]
    post_idx = np.where(times >= valley_t)[0]
    if len(post_idx) < 2:
        return valley_t
    peak_idx = post_idx[np.argmax(rms[post_idx])]
    peak_rms = rms[peak_idx]

    if peak_rms <= valley_rms * 1.15:
        return valley_t

    target_rms = valley_rms + 0.20 * (peak_rms - valley_rms)
    for idx in range(min_idx, peak_idx + 1):
        if rms[idx] >= target_rms:
            return times[idx]
    return valley_t

def run_investigation():
    samples = ["gaming_slang", "podcast_mixed", "gaming_live"]
    all_data = {}

    for sid in samples:
        audio, sr = load_wav(os.path.join(AUDIO_DIR, f"{sid}.wav"))
        with open(os.path.join(DTW_DIR, f"{sid}_small_q5_dtw.json"), encoding="utf-8") as f:
            words_dtw = convert_whisper_json_to_words(json.load(f))
        with open(os.path.join(DTW_DIR, f"{sid}_small_q5_nodtw.json"), encoding="utf-8") as f:
            words_nodtw = convert_whisper_json_to_words(json.load(f))

        # Select 30 evenly spaced words
        n = len(words_dtw)
        indices = [int(i * n / 30) for i in range(30)]
        indices = sorted(list(set(indices)))[:30]

        word_results = []
        for rank, idx in enumerate(indices):
            w = words_dtw[idx]
            t0 = w["t0"] / 1000.0

            # Capped S3b
            t_capped = s3b_detector_capped(audio, sr, t0, 0.25)
            err_capped = (t0 - t_capped) * 1000.0

            # Uncapped (250ms radius)
            t_uncapped = s3b_detector_uncapped(audio, sr, t0, 0.25)
            err_uncapped = (t0 - t_uncapped) * 1000.0

            # Valley-rise detector
            t_valley = valley_rise_detector(audio, sr, t0, 0.30)
            err_valley = (t0 - t_valley) * 1000.0

            # Word matching in NoDTW: find closest word by text and time
            nodtw_match = None
            for nw in words_nodtw:
                if abs(nw["t0"]/1000.0 - t0) < 2.0 and nw["text"].lower() == w["text"].lower():
                    nodtw_match = nw
                    break
            if nodtw_match is None:
                # find closest by time
                closest = min(words_nodtw, key=lambda nw: abs(nw["t0"]/1000.0 - t0))
                if abs(closest["t0"]/1000.0 - t0) < 1.0:
                    nodtw_match = closest

            nodtw_t0 = nodtw_match["t0"]/1000.0 if nodtw_match else None
            nodtw_err = (nodtw_t0 - t_valley)*1000.0 if nodtw_t0 is not None else None

            # S3b buggy index-based nodtw
            buggy_nodtw_t0 = words_nodtw[idx]["t0"]/1000.0 if idx < len(words_nodtw) else None
            buggy_nodtw_err = (buggy_nodtw_t0 - t_capped)*1000.0 if buggy_nodtw_t0 is not None else None

            word_results.append({
                "rank": rank + 1,
                "word": w["text"],
                "dtw_t0_ms": w["t0"],
                "s3b_capped_onset_ms": round(t_capped * 1000, 1),
                "s3b_capped_err_ms": round(err_capped, 1),
                "uncapped_onset_ms": round(t_uncapped * 1000, 1),
                "uncapped_err_ms": round(err_uncapped, 1),
                "valley_onset_ms": round(t_valley * 1000, 1),
                "valley_err_ms": round(err_valley, 1),
                "nodtw_word": nodtw_match["text"] if nodtw_match else None,
                "nodtw_t0_ms": round(nodtw_t0 * 1000, 1) if nodtw_t0 else None,
                "nodtw_err_vs_valley_ms": round(nodtw_err, 1) if nodtw_err else None,
                "buggy_s3b_nodtw_t0_ms": round(buggy_nodtw_t0 * 1000, 1) if buggy_nodtw_t0 else None,
                "buggy_s3b_nodtw_err_ms": round(buggy_nodtw_err, 1) if buggy_nodtw_err else None,
            })

        all_data[sid] = word_results

    # Write full diagnostic JSON
    out_file = os.path.join(BASE_DIR, "results", "s3c", "dtw_investigation_detailed.json")
    with open(out_file, "w", encoding="utf-8") as f:
        json.dump(all_data, f, indent=2, ensure_ascii=False)
    print(f"Saved detailed investigation to {out_file}")

if __name__ == "__main__":
    run_investigation()
