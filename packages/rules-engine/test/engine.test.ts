import { describe, expect, it } from 'vitest';
import { evaluateRules } from '../src/index.js';

describe('rules-engine stub', () => {
  it('returns passed: true on stub call', () => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const result = evaluateRules({} as any, {} as any);
    expect(result.passed).toBe(true);
    expect(result.violations).toEqual([]);
  });
});
