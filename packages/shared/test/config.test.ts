import { describe, expect, it } from 'vitest';
import { AppConfigSchema, SEED_APP_CONFIG } from '../src/config.js';

describe('AppConfigSchema', () => {
  it('validates seed defaults per DATA_MODEL §1', () => {
    expect(AppConfigSchema.parse(SEED_APP_CONFIG)).toEqual(SEED_APP_CONFIG);
  });

  it('rejects negative limits or invalid numbers', () => {
    const invalid = {
      ...SEED_APP_CONFIG,
      limits: {
        ...SEED_APP_CONFIG.limits,
        max_clip_seconds: -10,
      },
    };
    expect(() => AppConfigSchema.parse(invalid)).toThrow();
  });
});
