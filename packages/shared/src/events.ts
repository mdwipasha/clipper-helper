import { z } from 'zod';
import { ErrorCodeSchema } from './errors.js';

export const JobKindSchema = z.enum([
  'generate',
  'captions',
  'export',
  'model_download',
  'component_update',
  'app_update',
  'benchmark',
]);

export type JobKind = z.infer<typeof JobKindSchema>;

export const JobStateSchema = z.enum([
  'queued',
  'running',
  'paused',
  'succeeded',
  'failed',
  'cancelled',
]);

export type JobState = z.infer<typeof JobStateSchema>;

export const ProgressUnitSchema = z.enum([
  'bytes',
  'media_seconds',
  'frames',
  'audio_seconds',
  'steps',
]);

export type ProgressUnit = z.infer<typeof ProgressUnitSchema>;

export const JobEventSchema = z.object({
  jobId: z.string(),
  kind: JobKindSchema,
  state: JobStateSchema,
  stage: z.object({
    id: z.string(),
    index: z.number().int().nonnegative(),
    count: z.number().int().positive(),
    labelKey: z.string(),
    labelArgs: z.record(z.union([z.string(), z.number()])).optional(),
  }),
  stageProgress: z.object({
    done: z.number().nonnegative(),
    total: z.number().nonnegative().nullable(),
    unit: ProgressUnitSchema,
  }),
  overall: z.object({
    percent: z.number().min(0).max(100),
  }),
  rate: z
    .object({
      value: z.number().nonnegative(),
      unit: z.string(),
      smoothed: z.literal(true),
    })
    .optional(),
  eta: z
    .object({
      seconds: z.number().nonnegative(),
      lowSeconds: z.number().nonnegative(),
      highSeconds: z.number().nonnegative(),
      confidence: z.enum(['low', 'medium', 'high']),
    })
    .nullable()
    .optional(),
  detail: z.record(z.union([z.string(), z.number()])).optional(),
  error: z
    .object({
      code: ErrorCodeSchema,
      messageKey: z.string(),
      retryable: z.boolean(),
      actionKeys: z.array(z.string()),
    })
    .optional(),
  ts: z.number().nonnegative(),
});

export type JobEvent = z.infer<typeof JobEventSchema>;
