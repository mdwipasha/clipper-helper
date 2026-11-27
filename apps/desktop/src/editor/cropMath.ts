import type { Rect } from '@clipper-helper/shared';

export const OUTPUT_PORTRAIT_ASPECT = 9 / 16;
export const MIN_CROP_ZOOM = 1.0;
export const MAX_CROP_ZOOM = 3.0;

function roundCoord(v: number): number {
  return Math.round(v * 1_000_000) / 1_000_000;
}

function clamp(v: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, v));
}

/**
 * Computes the largest 9:16 normalized crop `(baseW, baseH)` that fits inside `(sourceWidth x sourceHeight)` at zoom 1.0x.
 *
 * Parity requirement (`ARCHITECTURE §7`):
 * FFmpeg render executes `crop=iw*w:ih*h:iw*x:ih*y,scale=1080:1920`, so `(w * sourceWidth) / (h * sourceHeight) === 9 / 16`.
 */
export function computeBasePortraitCropSize(
  sourceWidth: number,
  sourceHeight: number
): { w: number; h: number } {
  const sw = Math.max(1, sourceWidth);
  const sh = Math.max(1, sourceHeight);
  const srcAspect = sw / sh;

  if (Math.abs(srcAspect - OUTPUT_PORTRAIT_ASPECT) < 1e-4) {
    return { w: 1, h: 1 };
  }
  if (srcAspect > OUTPUT_PORTRAIT_ASPECT) {
    // Wider than 9:16 (e.g. 16:9 1920x1080 -> w = 1080*9/16 / 1920 = 0.316406)
    const w = clamp(OUTPUT_PORTRAIT_ASPECT / srcAspect, 0.05, 1.0);
    return { w: roundCoord(w), h: 1 };
  }
  // Taller than 9:16
  const h = clamp(srcAspect / OUTPUT_PORTRAIT_ASPECT, 0.05, 1.0);
  return { w: 1, h: roundCoord(h) };
}

export function getZoomFromCropRect(crop: Rect, sourceWidth: number, sourceHeight: number): number {
  const base = computeBasePortraitCropSize(sourceWidth, sourceHeight);
  if (!Number.isFinite(crop.w) || crop.w <= 0) {
    return MIN_CROP_ZOOM;
  }
  const zoom = base.w / crop.w;
  return roundCoord(clamp(zoom, MIN_CROP_ZOOM, MAX_CROP_ZOOM));
}

export function clampCropRect(rect: Rect, sourceWidth: number, sourceHeight: number): Rect {
  const base = computeBasePortraitCropSize(sourceWidth, sourceHeight);
  const minW = roundCoord(base.w / MAX_CROP_ZOOM);
  const rawW = Number.isFinite(rect.w) && rect.w > 0 ? rect.w : base.w;
  const w = roundCoord(clamp(rawW, minW, base.w));
  const h =
    Math.abs(w - base.w) < 1e-6 ? base.h : roundCoord(clamp((w / base.w) * base.h, 0.01, base.h));
  const rawX = Number.isFinite(rect.x) ? rect.x : (1 - w) * 0.5;
  const rawY = Number.isFinite(rect.y) ? rect.y : (1 - h) * 0.5;

  const x = roundCoord(clamp(rawX, 0, Math.max(0, 1 - w)));
  const y = roundCoord(clamp(rawY, 0, Math.max(0, 1 - h)));
  return { x, y, w, h };
}

export function defaultCropRect(sourceWidth: number, sourceHeight: number): Rect {
  const base = computeBasePortraitCropSize(sourceWidth, sourceHeight);
  return {
    x: roundCoord(Math.max(0, (1 - base.w) * 0.5)),
    y: roundCoord(Math.max(0, (1 - base.h) * 0.5)),
    w: base.w,
    h: base.h,
  };
}

export function centerCropRect(current: Rect, sourceWidth: number, sourceHeight: number): Rect {
  const clamped = clampCropRect(current, sourceWidth, sourceHeight);
  return {
    x: roundCoord(Math.max(0, (1 - clamped.w) * 0.5)),
    y: roundCoord(Math.max(0, (1 - clamped.h) * 0.5)),
    w: clamped.w,
    h: clamped.h,
  };
}

export function panCropRect(
  current: Rect,
  dx: number,
  dy: number,
  sourceWidth: number,
  sourceHeight: number
): Rect {
  return clampCropRect(
    {
      x: current.x + (Number.isFinite(dx) ? dx : 0),
      y: current.y + (Number.isFinite(dy) ? dy : 0),
      w: current.w,
      h: current.h,
    },
    sourceWidth,
    sourceHeight
  );
}

export function zoomCropRect(
  current: Rect,
  targetZoom: number,
  sourceWidth: number,
  sourceHeight: number
): Rect {
  const clampedCur = clampCropRect(current, sourceWidth, sourceHeight);
  const cx = clampedCur.x + clampedCur.w * 0.5;
  const cy = clampedCur.y + clampedCur.h * 0.5;

  const base = computeBasePortraitCropSize(sourceWidth, sourceHeight);
  const z = clamp(
    Number.isFinite(targetZoom) ? targetZoom : MIN_CROP_ZOOM,
    MIN_CROP_ZOOM,
    MAX_CROP_ZOOM
  );
  const nextW = base.w / z;
  const nextH = base.h / z;

  return clampCropRect(
    {
      x: cx - nextW * 0.5,
      y: cy - nextH * 0.5,
      w: nextW,
      h: nextH,
    },
    sourceWidth,
    sourceHeight
  );
}

export interface CssCropTransform {
  /** Pixel width of the uncropped video element matching the viewport scale */
  videoWidthPx: number;
  /** Pixel height of the uncropped video element matching the viewport scale */
  videoHeightPx: number;
  /** CSS translateX in pixels applied to the top-left of the video element */
  translateXPx: number;
  /** CSS translateY in pixels applied to the top-left of the video element */
  translateYPx: number;
  /** Responsive CSS percentages relative to the 9:16 preview viewport container */
  widthPct: number;
  heightPct: number;
  /** CSS translate percentage relative to the `<video>` element's own box */
  translateVideoXPct: number;
  translateVideoYPct: number;
  cssTransform: string;
}

/**
 * Pure crop-to-CSS transform mapper shared by the 9:16 preview `<video>` container.
 *
 * Parity guarantee (`ARCHITECTURE §7`):
 * Positioning a `<video>` of size `(frameW / rect.w, frameH / rect.h)` at
 * `translate(-rect.x * 100%, -rect.y * 100%)` inside an `overflow: hidden` `(frameW, frameH)`
 * 9:16 box displays the EXACT same pixel sub-region `[x*sw, y*sh, w*sw, h*sh]` that Rust's
 * FFmpeg `crop=iw*w:ih*h:iw*x:ih*y,scale=1080:1920` filter renders on export.
 */
export function cropRectToCssTransform(
  crop: Rect,
  frameWidthPx: number,
  frameHeightPx: number,
  sourceWidth: number,
  sourceHeight: number
): CssCropTransform {
  const c = clampCropRect(crop, sourceWidth, sourceHeight);
  const fw = Math.max(1, frameWidthPx);
  const fh = Math.max(1, frameHeightPx);

  const videoWidthPx = fw / c.w;
  const videoHeightPx = fh / c.h;
  const translateXPx = -c.x * videoWidthPx;
  const translateYPx = -c.y * videoHeightPx;

  const widthPct = (1 / c.w) * 100;
  const heightPct = (1 / c.h) * 100;
  const translateVideoXPct = -c.x * 100;
  const translateVideoYPct = -c.y * 100;

  return {
    videoWidthPx,
    videoHeightPx,
    translateXPx,
    translateYPx,
    widthPct,
    heightPct,
    translateVideoXPct,
    translateVideoYPct,
    cssTransform: `translate(${translateVideoXPct.toFixed(4)}%, ${translateVideoYPct.toFixed(4)}%)`,
  };
}
