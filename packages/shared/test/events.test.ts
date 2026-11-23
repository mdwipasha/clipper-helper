import { describe, expect, it } from 'vitest';
import { JobEvent, JobEventSchema } from '../src/events.js';

describe('JobEventSchema', () => {
  const validEvent: JobEvent = {
    jobId: 'job-123',
    kind: 'generate',
    state: 'running',
    stage: {
      id: 'download',
      index: 1,
      count: 4,
      labelKey: 'stage.download',
    },
    stageProgress: {
      done: 45,
      total: 100,
      unit: 'media_seconds',
    },
    overall: {
      percent: 42.5,
    },
    rate: {
      value: 1.66,
      unit: 'x realtime',
      smoothed: true,
    },
    eta: {
      seconds: 35,
      lowSeconds: 30,
      highSeconds: 45,
      confidence: 'medium',
    },
    ts: 123456789,
  };

  it('validates a correct job event', () => {
    expect(JobEventSchema.parse(validEvent)).toEqual(validEvent);
  });

  it('rejects percent out of range 0..100', () => {
    const invalid = {
      ...validEvent,
      overall: {
        percent: 105,
      },
    };
    expect(() => JobEventSchema.parse(invalid)).toThrow();
  });

  it('rejects invalid job kind or state', () => {
    const invalid = {
      ...validEvent,
      kind: 'render_everything',
    };
    expect(() => JobEventSchema.parse(invalid)).toThrow();
  });
});
