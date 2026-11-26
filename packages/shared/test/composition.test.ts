import { describe, expect, it } from 'vitest';
import {
  CaptionWordSchema,
  Composition,
  CompositionSchema,
  TimingSourceSchema,
} from '../src/composition.js';

describe('TimingSourceSchema', () => {
  it('accepts dtw, interpolated, and segment', () => {
    expect(TimingSourceSchema.parse('dtw')).toBe('dtw');
    expect(TimingSourceSchema.parse('interpolated')).toBe('interpolated');
    expect(TimingSourceSchema.parse('segment')).toBe('segment');
  });

  it('rejects invalid timing source', () => {
    expect(() => TimingSourceSchema.parse('whisper')).toThrow();
  });
});

describe('CaptionWordSchema', () => {
  it('validates word with dtw timing', () => {
    const word = {
      id: 'w-1',
      t0: 1000,
      t1: 1450,
      text: 'Halo',
      timingSource: 'dtw',
    };
    expect(CaptionWordSchema.parse(word)).toEqual(word);
  });
});

describe('CompositionSchema', () => {
  const validComposition: Composition = {
    schemaVersion: 1,
    id: 'a1b2c3d4-e5f6-7a8b-9c0d-1e2f3a4b5c6d',
    canvas: {
      width: 1080,
      height: 1920,
      fps: 30,
    },
    sources: [
      {
        id: 'src-1',
        provider: 'youtube',
        videoId: 'dQw4w9WgXcQ',
        path: 'cache/download_123.mp4',
        absoluteStartMs: 60000,
        durationMs: 30000,
        width: 1920,
        height: 1080,
        fps: 30,
      },
    ],
    timeline: [
      {
        id: 'seg-1',
        sourceId: 'src-1',
        inMs: 5000,
        outMs: 25000,
      },
    ],
    layout: {
      mode: 'fill',
      crop: { x: 0.2, y: 0, w: 0.6, h: 1.0 },
    },
    captions: {
      enabled: true,
      status: 'ready',
      language: 'id',
      words: [
        {
          id: 'w-1',
          t0: 0,
          t1: 450,
          text: 'Halo',
          timingSource: 'dtw',
        },
        {
          id: 'w-2',
          t0: 500,
          t1: 950,
          text: 'semua',
          timingSource: 'dtw',
        },
      ],
      edits: [],
      style: {
        fontFamily: 'Montserrat',
        fontSizePct: 5.0,
        weight: 700,
        color: '#FFFFFF',
        outlineColor: '#000000',
        outlinePx: 2,
        shadow: false,
        positionAnchor: 'bottom',
        offsetYPct: 15,
        maxWordsPerLine: 5,
        maxLines: 2,
        animation: 'word_highlight',
        highlightColor: '#FFD700',
        uppercase: true,
      },
    },
    overlays: [],
    audio: {
      gainDb: 0,
      normalize: {
        enabled: true,
        targetLufs: -14.0,
      },
    },
    template: null,
    export: {
      preset: 'standard',
      audioKbps: 192,
    },
  };

  it('validates a correct composition', () => {
    expect(CompositionSchema.parse(validComposition)).toEqual(validComposition);
  });

  it('rejects unsafe source paths with path traversal or absolute drive letters', () => {
    const invalidPath = {
      ...validComposition,
      sources: [
        {
          ...validComposition.sources[0]!,
          path: 'C:\\Windows\\System32\\calc.exe',
        },
      ],
    };
    expect(() => CompositionSchema.parse(invalidPath)).toThrow();

    const traversalPath = {
      ...validComposition,
      sources: [
        {
          ...validComposition.sources[0]!,
          path: 'cache/../../etc/passwd',
        },
      ],
    };
    expect(() => CompositionSchema.parse(traversalPath)).toThrow();
  });

  it('rejects timeline with more than 20 segments', () => {
    const tooManySegments = Array.from({ length: 21 }, (_, i) => ({
      id: `seg-${i}`,
      sourceId: 'src-1',
      inMs: i * 1000,
      outMs: (i + 1) * 1000,
    }));
    const invalid = {
      ...validComposition,
      timeline: tooManySegments,
    };
    expect(() => CompositionSchema.parse(invalid)).toThrow();
  });
});
