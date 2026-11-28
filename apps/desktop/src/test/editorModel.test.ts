import type { Composition } from '@clipper-helper/shared';
import { describe, expect, it } from 'vitest';
import {
  centerCropRect,
  clampCropRect,
  computeBasePortraitCropSize,
  cropRectToCssTransform,
  defaultCropRect,
  getZoomFromCropRect,
  panCropRect,
  zoomCropRect,
} from '../editor/cropMath';
import {
  createEditorState,
  EditorAction,
  executeEditorCommand,
  getTrimBounds,
  MAX_UNDO_STACK,
  redoEditorCommand,
  undoEditorCommand,
} from '../editor/editorState';
import {
  clampTrimRange,
  formatFrameTimecodeMs,
  frameDurationMs,
  frameToMs,
  frameToSeekSec,
  msToNearestFrame,
  snapMsToFrame,
  stepByFrames,
  stepByFramesTarget,
} from '../editor/frameMath';

function makeComposition(overrides?: Partial<Composition>): Composition {
  return {
    schemaVersion: 1,
    id: '11111111-1111-4111-8111-111111111111',
    canvas: { width: 1080, height: 1920, fps: 60 },
    sources: [
      {
        id: 'src-1',
        provider: 'youtube',
        videoId: 'aqz-KE-bpKQ',
        path: 'cache/src.mp4',
        absoluteStartMs: 10_000,
        durationMs: 70_000,
        width: 1920,
        height: 1080,
        fps: 60,
      },
    ],
    timeline: [{ id: 'seg-1', sourceId: 'src-1', inMs: 5_000, outMs: 65_000 }],
    layout: {
      mode: 'fill',
      crop: defaultCropRect(1920, 1080),
    },
    captions: {
      enabled: false,
      status: 'none',
      language: 'id',
      words: [],
      edits: [],
      style: {
        fontFamily: 'Montserrat',
        fontSizePct: 5,
        weight: 700,
        color: '#FFFFFF',
        outlineColor: '#000000',
        outlinePx: 2,
        shadow: true,
        positionAnchor: 'bottom',
        offsetYPct: 14,
        maxWordsPerLine: 5,
        maxLines: 2,
        animation: 'phrase',
        highlightColor: '#FACC15',
        uppercase: false,
      },
    },
    overlays: [],
    audio: { gainDb: 0, normalize: { enabled: false, targetLufs: -14 } },
    template: null,
    export: { preset: 'standard', audioKbps: 192 },
    ...overrides,
  };
}

describe('frameMath (rational fps, snapping, stepping, clamping)', () => {
  it('differentiates 59.94 (60000/1001) vs 60 (60/1) over 60 frames cleanly', () => {
    const fps60 = { num: 60, den: 1 };
    const fps5994 = { num: 60_000, den: 1001 };

    expect(frameDurationMs(fps60)).toBeCloseTo(16.6667, 3);
    expect(frameDurationMs(fps5994)).toBeCloseTo(16.6833, 3);
    expect(frameToMs(60, fps60)).toBe(1000);
    expect(frameToMs(60, fps5994)).toBe(1001);

    let t = 0;
    const steps: number[] = [];
    for (let i = 0; i < 60; i += 1) {
      t = stepByFrames(t, 1, fps60, 70_000);
      steps.push(t);
    }
    expect(steps[0]).toBe(17);
    expect(steps[59]).toBe(1000);
    expect(msToNearestFrame(1000, fps60)).toBe(60);
    expect(snapMsToFrame(1005, fps60, 70_000)).toBe(1000);
    expect(formatFrameTimecodeMs(61_017)).toBe('01:01.017');
  });

  it('clamps trim range to [0, durationMs] and enforces >= minClipMs (1s)', () => {
    const fps30 = { num: 30, den: 1 };
    expect(clampTrimRange(-500, 75_000, 70_000, fps30, 1000, 'both')).toEqual({
      inMs: 0,
      outMs: 70_000,
    });
    expect(clampTrimRange(9_800, 10_000, 70_000, fps30, 1000, 'in')).toEqual({
      inMs: 9_000,
      outMs: 10_000,
    });
    expect(clampTrimRange(10_000, 10_200, 70_000, fps30, 1000, 'out')).toEqual({
      inMs: 10_000,
      outMs: 11_000,
    });
  });

  it('computes WebView2-safe seek targets inside frame intervals to avoid 60fps PTS truncation repeats', () => {
    const fps60 = { num: 60, den: 1 };
    const fps5994 = { num: 60_000, den: 1001 };

    // Frame 2 PTS = 2/60 = 0.033333...s (integer 33ms would fall BEFORE PTS!)
    // frameToSeekSec adds +0.25 frame margin -> strictly inside [2/60, 3/60)
    const sec2 = frameToSeekSec(2, fps60, 10_000);
    expect(sec2).toBeGreaterThan(2 / 60);
    expect(sec2).toBeLessThan(3 / 60);

    // Stepping 12 consecutive frames from 0 never hits a previous frame PTS boundary
    let curSec = 0;
    for (let f = 1; f <= 12; f += 1) {
      const next = stepByFramesTarget(curSec, 1, fps60, 10_000);
      expect(next.targetIndex).toBe(f);
      expect(next.targetSec).toBeGreaterThan(f / 60);
      expect(next.targetSec).toBeLessThan((f + 0.5) / 60);
      curSec = f / 60;
    }

    // Same invariant holds for 59.94 fps
    const ntscStep = stepByFramesTarget(1001 / 60_000, 1, fps5994, 10_000);
    expect(ntscStep.targetIndex).toBe(2);
    expect(ntscStep.targetSec).toBeGreaterThan((2 * 1001) / 60_000);
    expect(ntscStep.targetSec).toBeLessThan((3 * 1001) / 60_000);
  });
});

describe('cropMath (9:16 locked aspect, zoom 1x-3x, pan clamping, CSS parity)', () => {
  it('computes 0.316406 base width on 1920x1080 and handles 4:3, 1:1, and ultra-tall sources', () => {
    const c169 = computeBasePortraitCropSize(1920, 1080);
    expect(c169.w).toBeCloseTo(0.316406, 5);
    expect(c169.h).toBe(1);

    const c43 = computeBasePortraitCropSize(1440, 1080);
    expect(c43.w).toBeCloseTo(9 / 16 / (4 / 3), 5);
    expect(c43.h).toBe(1);

    const cTall = computeBasePortraitCropSize(1080, 2400);
    expect(cTall.w).toBe(1);
    expect(cTall.h).toBeCloseTo(1080 / 2400 / (9 / 16), 5);
  });

  it('clamps zoom inside [1, 3], keeps crop within [0, 1], and preserves exact FFmpeg parity transform', () => {
    const base = defaultCropRect(1920, 1080);
    expect(getZoomFromCropRect(base, 1920, 1080)).toBeCloseTo(1, 3);

    const zoomed2x = zoomCropRect(base, 2, 1920, 1080);
    expect(zoomed2x.w).toBeCloseTo(base.w / 2, 5);
    expect(zoomed2x.h).toBeCloseTo(0.5, 5);
    expect(getZoomFromCropRect(zoomed2x, 1920, 1080)).toBeCloseTo(2, 2);

    // Zoom beyond cap clamps to 3x
    const zoomedOver = zoomCropRect(base, 9, 1920, 1080);
    expect(getZoomFromCropRect(zoomedOver, 1920, 1080)).toBeCloseTo(3, 2);

    // Pan past edges clamps to [0, 1 - w] x [0, 1 - h]
    const pannedEdge = panCropRect(zoomed2x, 10, 10, 1920, 1080);
    expect(pannedEdge.x + pannedEdge.w).toBeLessThanOrEqual(1.000001);
    expect(pannedEdge.y + pannedEdge.h).toBeLessThanOrEqual(1.000001);
    const pannedNeg = panCropRect(zoomed2x, -10, -10, 1920, 1080);
    expect(pannedNeg.x).toBe(0);
    expect(pannedNeg.y).toBe(0);

    // Center preserves zoom level
    const centered = centerCropRect(pannedEdge, 1920, 1080);
    expect(centered.x).toBeCloseTo((1 - centered.w) * 0.5, 5);
    expect(centered.y).toBeCloseTo((1 - centered.h) * 0.5, 5);

    // Parity mapping table against FFmpeg crop=iw*w:ih*h:iw*x:ih*y -> scale=1080:1920
    const css = cropRectToCssTransform(centered, 270, 480, 1920, 1080);
    expect(css.videoWidthPx).toBeCloseTo(270 / centered.w, 3);
    expect(css.videoHeightPx).toBeCloseTo(480 / centered.h, 3);
    // Check unstretched source ratio preserved inside CSS video box
    expect(css.videoWidthPx / css.videoHeightPx).toBeCloseTo(1920 / 1080, 3);
    // Check viewport top-left pixel aligns with crop (x, y)
    expect(css.translateXPx).toBeCloseTo(-centered.x * css.videoWidthPx, 3);
    expect(css.translateYPx).toBeCloseTo(-centered.y * css.videoHeightPx, 3);
    // NaN/negative inputs get safely sanitized
    const safeFromNan = clampCropRect({ x: NaN, y: -5, w: -1, h: 2 }, 1920, 1080);
    expect(safeFromNan.x).toBeGreaterThanOrEqual(0);
    expect(safeFromNan.w).toBeGreaterThan(0);
  });
});

describe('editorState command pattern, coalescing, undo/redo limit & property invariants', () => {
  it('supports coalesced drags, undo/redo symmetry, and 200-entry cap', () => {
    let state = createEditorState(makeComposition(), {
      fps: { num: 60, den: 1 },
      maxClipMs: 300_000,
    });
    const initialCrop =
      state.composition.layout.mode === 'fill' ? state.composition.layout.crop : null;

    // 5 drag updates with same coalesceKey collapse to 1 undo entry
    for (let i = 0; i < 5; i += 1) {
      state = executeEditorCommand(state, {
        type: 'pan',
        dx: 0.02,
        dy: 0,
        coalesceKey: 'drag-1',
      });
    }
    expect(state.undoStack).toHaveLength(1);
    expect(state.dirty).toBe(true);

    // Single undo restores exact starting crop and clears dirty
    state = undoEditorCommand(state);
    expect(state.dirty).toBe(false);
    expect(state.composition.layout.mode === 'fill' ? state.composition.layout.crop : null).toEqual(
      initialCrop
    );

    // Redo re-applies
    state = redoEditorCommand(state);
    expect(state.dirty).toBe(true);

    // Enforce 200-step undo cap cleanly
    for (let i = 0; i < 250; i += 1) {
      state = executeEditorCommand(state, {
        type: 'pan',
        dx: i % 2 === 0 ? 0.005 : -0.003,
        dy: 0,
      });
    }
    expect(state.undoStack.length).toBeLessThanOrEqual(MAX_UNDO_STACK);
  });

  it('property-tests random command sequences preserve crop & trim bounds and undo(do) === identity', () => {
    let seed = 42_1337;
    const nextRand = () => {
      seed = (seed * 1664525 + 1013904223) >>> 0;
      return seed / 4294967296;
    };

    let state = createEditorState(makeComposition(), {
      fps: { num: 60_000, den: 1001 },
      maxClipMs: 300_000,
    });

    for (let step = 0; step < 120; step += 1) {
      const beforeComp = JSON.stringify(state.composition);
      const r = nextRand();
      let action: EditorAction;
      if (r < 0.2) {
        action = {
          type: 'setTrim',
          inMs: nextRand() * 70_000,
          outMs: nextRand() * 70_000,
          anchor: nextRand() > 0.5 ? 'in' : 'out',
        };
      } else if (r < 0.35) {
        action = { type: 'nudgeIn', deltaFrames: Math.round((nextRand() - 0.5) * 60) };
      } else if (r < 0.5) {
        action = { type: 'nudgeOut', deltaFrames: Math.round((nextRand() - 0.5) * 60) };
      } else if (r < 0.7) {
        action = {
          type: 'pan',
          dx: (nextRand() - 0.5) * 0.4,
          dy: (nextRand() - 0.5) * 0.4,
        };
      } else if (r < 0.85) {
        action = { type: 'zoom', factor: 1 + nextRand() * 2.5 };
      } else if (r < 0.93) {
        action = { type: 'centerCrop' };
      } else {
        action = { type: 'resetCrop' };
      }

      const afterDo = executeEditorCommand(state, action);
      // Invariants always hold after every command
      const trim = getTrimBounds(afterDo.composition);
      expect(trim.inMs).toBeGreaterThanOrEqual(0);
      expect(trim.outMs).toBeLessThanOrEqual(70_000);
      expect(trim.outMs - trim.inMs).toBeGreaterThanOrEqual(1000);

      expect(afterDo.composition.layout.mode).toBe('fill');
      if (afterDo.composition.layout.mode === 'fill') {
        const c = afterDo.composition.layout.crop;
        expect(c.x).toBeGreaterThanOrEqual(0);
        expect(c.y).toBeGreaterThanOrEqual(0);
        expect(c.w).toBeGreaterThan(0);
        expect(c.h).toBeGreaterThan(0);
        expect(c.x + c.w).toBeLessThanOrEqual(1.00001);
        expect(c.y + c.h).toBeLessThanOrEqual(1.00001);
      }

      if (afterDo !== state) {
        const reverted = undoEditorCommand(afterDo);
        expect(JSON.stringify(reverted.composition)).toBe(beforeComp);
      }
      state = afterDo;
    }
  });
});
