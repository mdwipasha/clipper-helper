export interface RationalFps {
  num: number;
  den: number;
}

export const DEFAULT_MIN_CLIP_MS = 1000;

export function normalizeRationalFps(num: number, den: number): RationalFps {
  if (!Number.isFinite(num) || !Number.isFinite(den) || num <= 0 || den <= 0) {
    return { num: 30, den: 1 };
  }
  return { num: Math.round(num), den: Math.round(den) };
}

export function rationalToFps(fps: RationalFps): number {
  const n = normalizeRationalFps(fps.num, fps.den);
  return n.num / n.den;
}

export function frameDurationMs(fps: RationalFps): number {
  const n = normalizeRationalFps(fps.num, fps.den);
  return (1000 * n.den) / n.num;
}

export function msToNearestFrame(ms: number, fps: RationalFps): number {
  if (!Number.isFinite(ms) || ms <= 0) return 0;
  const n = normalizeRationalFps(fps.num, fps.den);
  return Math.max(0, Math.round((ms * n.num) / (1000 * n.den)));
}

export function frameToMs(frameIndex: number, fps: RationalFps): number {
  if (!Number.isFinite(frameIndex) || frameIndex <= 0) return 0;
  const n = normalizeRationalFps(fps.num, fps.den);
  return Math.max(0, Math.round((frameIndex * 1000 * n.den) / n.num));
}

export function snapMsToFrame(ms: number, fps: RationalFps, durationMs: number): number {
  const safeDur = Math.max(0, Math.round(durationMs));
  if (!Number.isFinite(ms) || ms <= 0) return 0;
  if (ms >= safeDur) return safeDur;
  const frameStep = frameDurationMs(fps);
  if (safeDur - ms < frameStep * 0.5) {
    return safeDur;
  }
  const idx = msToNearestFrame(ms, fps);
  return Math.min(safeDur, frameToMs(idx, fps));
}

export function stepByFrames(
  currentMs: number,
  deltaFrames: number,
  fps: RationalFps,
  durationMs: number
): number {
  const safeDur = Math.max(0, Math.round(durationMs));
  const curIdx = msToNearestFrame(Math.max(0, Math.min(safeDur, currentMs)), fps);
  const nextIdx = Math.max(0, curIdx + Math.round(deltaFrames));
  const nextMs = frameToMs(nextIdx, fps);
  return Math.max(0, Math.min(safeDur, nextMs));
}

export function effectiveMinClipMs(durationMs: number, minClipMs = DEFAULT_MIN_CLIP_MS): number {
  return Math.min(Math.max(1, Math.round(durationMs)), Math.max(1, Math.round(minClipMs)));
}

/**
 * Clamps and frame-snaps an `(inMs, outMs)` pair inside `[0, durationMs]`,
 * ensuring `outMs - inMs >= effectiveMinClipMs`.
 */
export function clampTrimRange(
  inMs: number,
  outMs: number,
  durationMs: number,
  fps: RationalFps,
  minClipMs = DEFAULT_MIN_CLIP_MS,
  anchor: 'in' | 'out' | 'both' = 'both'
): { inMs: number; outMs: number } {
  const safeDur = Math.max(1, Math.round(durationMs));
  const minLen = effectiveMinClipMs(safeDur, minClipMs);

  let nextIn = snapMsToFrame(inMs, fps, safeDur);
  let nextOut = snapMsToFrame(outMs, fps, safeDur);

  if (anchor === 'in') {
    nextIn = Math.max(0, Math.min(nextOut - minLen, nextIn));
    nextIn = snapMsToFrame(nextIn, fps, safeDur);
    if (nextOut - nextIn < minLen) {
      nextIn = Math.max(0, nextOut - minLen);
    }
  } else if (anchor === 'out') {
    nextOut = Math.min(safeDur, Math.max(nextIn + minLen, nextOut));
    nextOut = snapMsToFrame(nextOut, fps, safeDur);
    if (nextOut - nextIn < minLen) {
      nextOut = Math.min(safeDur, nextIn + minLen);
    }
  } else {
    nextIn = Math.max(0, Math.min(safeDur - minLen, nextIn));
    nextOut = Math.min(safeDur, Math.max(nextIn + minLen, nextOut));
  }

  return { inMs: Math.round(nextIn), outMs: Math.round(nextOut) };
}

/**
 * Formats millisecond time as `MM:SS.mmm` (or `HH:MM:SS.mmm` when >= 1 hour)
 * so 1-frame nudges (e.g. `1/60 s` ≈ 16–17 ms) are clearly visible in the editor readout.
 */
export function formatFrameTimecodeMs(ms: number): string {
  const safe = Number.isFinite(ms) ? Math.max(0, Math.round(ms)) : 0;
  const totalSecs = Math.floor(safe / 1000);
  const remMs = safe % 1000;
  const h = Math.floor(totalSecs / 3600);
  const m = Math.floor((totalSecs % 3600) / 60);
  const s = totalSecs % 60;
  const pad2 = (n: number) => String(n).padStart(2, '0');
  const pad3 = (n: number) => String(n).padStart(3, '0');
  const base = h > 0 ? `${pad2(h)}:${pad2(m)}:${pad2(s)}` : `${pad2(m)}:${pad2(s)}`;
  return `${base}.${pad3(remMs)}`;
}
