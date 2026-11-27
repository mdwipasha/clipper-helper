import type { Composition, Rect } from '@clipper-helper/shared';
import {
  centerCropRect,
  clampCropRect,
  defaultCropRect,
  panCropRect,
  zoomCropRect,
} from './cropMath';
import { clampTrimRange, DEFAULT_MIN_CLIP_MS, RationalFps, stepByFrames } from './frameMath';

export const MAX_UNDO_STACK = 200;

export type PlaybackRate = 0.5 | 1 | 2;

export type SaveStatus = 'saved' | 'saving' | 'dirty' | 'error';

export interface EditorUiState {
  playheadMs: number;
  isPlaying: boolean;
  loopSelection: boolean;
  muted: boolean;
  playbackRate: PlaybackRate;
}

export interface EditorHistoryEntry {
  kind:
    | 'setTrim'
    | 'nudgeIn'
    | 'nudgeOut'
    | 'setCrop'
    | 'pan'
    | 'zoom'
    | 'resetCrop'
    | 'centerCrop';
  coalesceKey?: string | undefined;
  before: {
    inMs: number;
    outMs: number;
    crop: Rect;
  };
  after: {
    inMs: number;
    outMs: number;
    crop: Rect;
  };
}

export interface EditorConfigBounds {
  fps: RationalFps;
  sourceWidth: number;
  sourceHeight: number;
  sourceDurationMs: number;
  minClipMs: number;
  maxClipMs: number;
}

export interface EditorState {
  composition: Composition;
  bounds: EditorConfigBounds;
  ui: EditorUiState;
  undoStack: EditorHistoryEntry[];
  redoStack: EditorHistoryEntry[];
  savedSignature: string;
  dirty: boolean;
}

export type EditorAction =
  | {
      type: 'setTrim';
      inMs: number;
      outMs: number;
      anchor?: 'in' | 'out' | 'both';
      coalesceKey?: string;
    }
  | { type: 'nudgeIn'; deltaFrames?: number; deltaMs?: number; coalesceKey?: string }
  | { type: 'nudgeOut'; deltaFrames?: number; deltaMs?: number; coalesceKey?: string }
  | { type: 'setCrop'; crop: Rect; coalesceKey?: string }
  | { type: 'pan'; dx: number; dy: number; coalesceKey?: string }
  | { type: 'zoom'; factor: number; coalesceKey?: string }
  | { type: 'resetCrop' }
  | { type: 'centerCrop' };

export function compositionEditSignature(comp: Composition): string {
  const seg = comp.timeline[0];
  const crop = comp.layout.mode === 'fill' ? comp.layout.crop : null;
  return JSON.stringify({ inMs: seg?.inMs ?? 0, outMs: seg?.outMs ?? 0, crop });
}

export function getTrimBounds(comp: Composition): { inMs: number; outMs: number } {
  const seg = comp.timeline[0];
  return {
    inMs: seg ? Math.round(seg.inMs) : 0,
    outMs: seg ? Math.round(seg.outMs) : 0,
  };
}

export function getFillCrop(comp: Composition, sw: number, sh: number): Rect {
  if (comp.layout.mode === 'fill') {
    return clampCropRect(comp.layout.crop, sw, sh);
  }
  return defaultCropRect(sw, sh);
}

export function createEditorState(
  initialComposition: Composition,
  bounds: Partial<EditorConfigBounds> & { fps: RationalFps; maxClipMs: number }
): EditorState {
  const src = initialComposition.sources[0];
  const sw = bounds.sourceWidth ?? src?.width ?? 1920;
  const sh = bounds.sourceHeight ?? src?.height ?? 1080;
  const dur = bounds.sourceDurationMs ?? src?.durationMs ?? 60_000;
  const minClipMs = bounds.minClipMs ?? DEFAULT_MIN_CLIP_MS;

  const seg = initialComposition.timeline[0];
  const clamped = clampTrimRange(
    seg?.inMs ?? 0,
    seg?.outMs ?? dur,
    dur,
    bounds.fps,
    minClipMs,
    'both'
  );
  const crop = getFillCrop(initialComposition, sw, sh);

  const normalizedComp: Composition = {
    ...initialComposition,
    timeline: [
      {
        id: seg?.id ?? 'seg-1',
        sourceId: seg?.sourceId ?? src?.id ?? 'src-1',
        inMs: clamped.inMs,
        outMs: clamped.outMs,
      },
      ...initialComposition.timeline.slice(1),
    ],
    layout: { mode: 'fill', crop },
  };

  return {
    composition: normalizedComp,
    bounds: {
      fps: bounds.fps,
      sourceWidth: sw,
      sourceHeight: sh,
      sourceDurationMs: dur,
      minClipMs,
      maxClipMs: bounds.maxClipMs,
    },
    ui: {
      playheadMs: clamped.inMs,
      isPlaying: false,
      loopSelection: false,
      muted: false,
      playbackRate: 1,
    },
    undoStack: [],
    redoStack: [],
    savedSignature: compositionEditSignature(normalizedComp),
    dirty: false,
  };
}

function applyTrimAndCrop(comp: Composition, inMs: number, outMs: number, crop: Rect): Composition {
  const firstSeg = comp.timeline[0];
  return {
    ...comp,
    timeline: [
      {
        id: firstSeg?.id ?? 'seg-1',
        sourceId: firstSeg?.sourceId ?? comp.sources[0]?.id ?? 'src-1',
        inMs,
        outMs,
      },
      ...comp.timeline.slice(1),
    ],
    layout: { mode: 'fill', crop },
  };
}

export function executeEditorCommand(state: EditorState, action: EditorAction): EditorState {
  const { fps, sourceWidth, sourceHeight, sourceDurationMs, minClipMs } = state.bounds;
  const beforeTrim = getTrimBounds(state.composition);
  const beforeCrop = getFillCrop(state.composition, sourceWidth, sourceHeight);

  let nextTrim = beforeTrim;
  let nextCrop = beforeCrop;
  let coalesceKey: string | undefined;

  switch (action.type) {
    case 'setTrim': {
      nextTrim = clampTrimRange(
        action.inMs,
        action.outMs,
        sourceDurationMs,
        fps,
        minClipMs,
        action.anchor ?? 'both'
      );
      coalesceKey = action.coalesceKey;
      break;
    }
    case 'nudgeIn': {
      const targetIn =
        typeof action.deltaFrames === 'number'
          ? stepByFrames(beforeTrim.inMs, action.deltaFrames, fps, sourceDurationMs)
          : beforeTrim.inMs + (action.deltaMs ?? 0);
      nextTrim = clampTrimRange(targetIn, beforeTrim.outMs, sourceDurationMs, fps, minClipMs, 'in');
      coalesceKey = action.coalesceKey;
      break;
    }
    case 'nudgeOut': {
      const targetOut =
        typeof action.deltaFrames === 'number'
          ? stepByFrames(beforeTrim.outMs, action.deltaFrames, fps, sourceDurationMs)
          : beforeTrim.outMs + (action.deltaMs ?? 0);
      nextTrim = clampTrimRange(
        beforeTrim.inMs,
        targetOut,
        sourceDurationMs,
        fps,
        minClipMs,
        'out'
      );
      coalesceKey = action.coalesceKey;
      break;
    }
    case 'setCrop': {
      nextCrop = clampCropRect(action.crop, sourceWidth, sourceHeight);
      coalesceKey = action.coalesceKey;
      break;
    }
    case 'pan': {
      nextCrop = panCropRect(beforeCrop, action.dx, action.dy, sourceWidth, sourceHeight);
      coalesceKey = action.coalesceKey;
      break;
    }
    case 'zoom': {
      nextCrop = zoomCropRect(beforeCrop, action.factor, sourceWidth, sourceHeight);
      coalesceKey = action.coalesceKey;
      break;
    }
    case 'resetCrop': {
      nextCrop = defaultCropRect(sourceWidth, sourceHeight);
      break;
    }
    case 'centerCrop': {
      nextCrop = centerCropRect(beforeCrop, sourceWidth, sourceHeight);
      break;
    }
  }

  const unchanged =
    nextTrim.inMs === beforeTrim.inMs &&
    nextTrim.outMs === beforeTrim.outMs &&
    nextCrop.x === beforeCrop.x &&
    nextCrop.y === beforeCrop.y &&
    nextCrop.w === beforeCrop.w &&
    nextCrop.h === beforeCrop.h;
  if (unchanged) {
    return state;
  }

  const nextComp = applyTrimAndCrop(state.composition, nextTrim.inMs, nextTrim.outMs, nextCrop);
  const top = state.undoStack[state.undoStack.length - 1];
  const shouldCoalesce =
    Boolean(coalesceKey) && top && top.kind === action.type && top.coalesceKey === coalesceKey;

  let nextUndo: EditorHistoryEntry[];
  if (shouldCoalesce && top) {
    const merged: EditorHistoryEntry = {
      ...top,
      after: { inMs: nextTrim.inMs, outMs: nextTrim.outMs, crop: nextCrop },
    };
    nextUndo = [...state.undoStack.slice(0, -1), merged];
  } else {
    const entry: EditorHistoryEntry = {
      kind: action.type,
      coalesceKey,
      before: { inMs: beforeTrim.inMs, outMs: beforeTrim.outMs, crop: beforeCrop },
      after: { inMs: nextTrim.inMs, outMs: nextTrim.outMs, crop: nextCrop },
    };
    nextUndo = [...state.undoStack, entry].slice(-MAX_UNDO_STACK);
  }

  return {
    ...state,
    composition: nextComp,
    undoStack: nextUndo,
    redoStack: [],
    dirty: compositionEditSignature(nextComp) !== state.savedSignature,
  };
}

export function undoEditorCommand(state: EditorState): EditorState {
  const entry = state.undoStack[state.undoStack.length - 1];
  if (!entry) return state;

  const nextComp = applyTrimAndCrop(
    state.composition,
    entry.before.inMs,
    entry.before.outMs,
    entry.before.crop
  );
  return {
    ...state,
    composition: nextComp,
    undoStack: state.undoStack.slice(0, -1),
    redoStack: [...state.redoStack, entry].slice(-MAX_UNDO_STACK),
    dirty: compositionEditSignature(nextComp) !== state.savedSignature,
  };
}

export function redoEditorCommand(state: EditorState): EditorState {
  const entry = state.redoStack[state.redoStack.length - 1];
  if (!entry) return state;

  const nextComp = applyTrimAndCrop(
    state.composition,
    entry.after.inMs,
    entry.after.outMs,
    entry.after.crop
  );
  return {
    ...state,
    composition: nextComp,
    undoStack: [...state.undoStack, entry].slice(-MAX_UNDO_STACK),
    redoStack: state.redoStack.slice(0, -1),
    dirty: compositionEditSignature(nextComp) !== state.savedSignature,
  };
}

export function markEditorSaved(
  state: EditorState,
  persistedComposition?: Composition
): EditorState {
  const comp = persistedComposition ?? state.composition;
  const sig = compositionEditSignature(comp);
  return {
    ...state,
    savedSignature: sig,
    dirty: compositionEditSignature(state.composition) !== sig,
  };
}
