import { z } from 'zod';
import { AnchorSchema, CaptionStyleSchema, HexColorRegex, RectSchema } from './template.js';

export const WorkspacePathRegex = /^(cache|projects)\/(?!.*\.\.)[A-Za-z0-9._/-]+$/;

export const SourceProviderSchema = z.enum(['youtube', 'local_file']);
export type SourceProvider = z.infer<typeof SourceProviderSchema>;

export const TimingSourceSchema = z.enum(['dtw', 'interpolated', 'segment']);
export type TimingSource = z.infer<typeof TimingSourceSchema>;

export const CaptionWordSchema = z.object({
  id: z.string(),
  t0: z.number().nonnegative(),
  t1: z.number().nonnegative(),
  text: z.string(),
  timingSource: TimingSourceSchema,
});

export type CaptionWord = z.infer<typeof CaptionWordSchema>;

export const CaptionEditSchema = z.object({
  wordId: z.string(),
  text: z.string().optional(),
  t0: z.number().nonnegative().optional(),
  t1: z.number().nonnegative().optional(),
  deleted: z.boolean().optional(),
});

export type CaptionEdit = z.infer<typeof CaptionEditSchema>;

export const CompositionSourceSchema = z.object({
  id: z.string(),
  provider: SourceProviderSchema,
  videoId: z.string().min(1),
  path: z
    .string()
    .regex(
      WorkspacePathRegex,
      'Must be POSIX path relative to workspace root matching ^(cache|projects)/[A-Za-z0-9._/-]+$'
    ),
  absoluteStartMs: z.number().nonnegative(),
  durationMs: z.number().positive(),
  width: z.number().int().positive(),
  height: z.number().int().positive(),
  fps: z.number().positive(),
});

export type CompositionSource = z.infer<typeof CompositionSourceSchema>;

export const TimelineSegmentSchema = z.object({
  id: z.string(),
  sourceId: z.string(),
  inMs: z.number().nonnegative(),
  outMs: z.number().positive(),
});

export type TimelineSegment = z.infer<typeof TimelineSegmentSchema>;

export const CompositionLayoutSchema = z.discriminatedUnion('mode', [
  z.object({
    mode: z.literal('fill'),
    crop: RectSchema,
  }),
  z.object({
    mode: z.literal('blur_fit'),
    blur: z.number().positive(),
  }),
  z.object({
    mode: z.literal('split'),
    top: RectSchema,
    bottom: RectSchema,
    ratio: z.number().min(0.3).max(0.7),
  }),
]);

export type CompositionLayout = z.infer<typeof CompositionLayoutSchema>;

export const OverlaySchema = z.discriminatedUnion('type', [
  z.object({
    type: z.literal('logo'),
    assetId: z.string(),
    anchor: AnchorSchema,
    marginPct: z.number().min(0).max(100),
    widthPct: z.number().positive().max(100),
    opacity: z.number().min(0).max(1),
  }),
  z.object({
    type: z.literal('text'),
    text: z.string(),
    anchor: AnchorSchema,
    marginPct: z.number().min(0).max(100),
    sizePct: z.number().positive().max(100),
    color: z.string().regex(HexColorRegex, 'Invalid hex color'),
    startMs: z.number().nonnegative().optional(),
    endMs: z.number().positive().optional(),
  }),
]);

export type Overlay = z.infer<typeof OverlaySchema>;

export const CompositionSchema = z.object({
  schemaVersion: z.literal(1),
  id: z.string().uuid(),
  canvas: z.object({
    width: z.literal(1080),
    height: z.literal(1920),
    fps: z.union([z.literal(30), z.literal(60)]),
  }),
  sources: z.array(CompositionSourceSchema).min(1),
  timeline: z.array(TimelineSegmentSchema).min(1).max(20),
  layout: CompositionLayoutSchema,
  captions: z.object({
    enabled: z.boolean(),
    status: z.enum(['none', 'running', 'ready', 'stale']),
    language: z.enum(['id', 'en', 'auto']),
    words: z.array(CaptionWordSchema),
    edits: z.array(CaptionEditSchema),
    style: CaptionStyleSchema,
  }),
  overlays: z.array(OverlaySchema),
  audio: z.object({
    gainDb: z.number(),
    normalize: z.object({
      enabled: z.boolean(),
      targetLufs: z.number().min(-70).max(0),
    }),
  }),
  template: z
    .object({
      id: z.string().uuid(),
      hash: z.string(),
    })
    .nullable(),
  export: z.object({
    preset: z.enum(['standard', 'small_file']),
    audioKbps: z.number().int().positive(),
  }),
});

export type Composition = z.infer<typeof CompositionSchema>;

export const SourceTranscriptSchema = z.object({
  sourceId: z.string(),
  coverage: z.array(z.tuple([z.number().nonnegative(), z.number().nonnegative()])),
  words: z.array(CaptionWordSchema),
});

export type SourceTranscript = z.infer<typeof SourceTranscriptSchema>;
