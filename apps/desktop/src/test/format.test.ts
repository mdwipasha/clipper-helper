import { describe, expect, it } from 'vitest';
import { formatComponentVersion } from '../utils/format';

describe('formatComponentVersion', () => {
  it.each([
    ['n9.0.2-58-g8c7ef49002-20261119', 'n9.0.2-58-g8c7ef49002-20261119'],
    ['v0.18.0', 'v0.18.0'],
    ['1.9.5', 'v1.9.5'],
    ['2026.10.26', 'v2026.10.26'],
    ['silero-v6.2.0-2aa269b7', 'silero-v6.2.0-2aa269b7'],
    ['whisper-small-q5_1-ae85e4a9', 'whisper-small-q5_1-ae85e4a9'],
    ['', ''],
    [null, ''],
  ])('formats %s -> %s', (input, expected) => {
    expect(formatComponentVersion(input)).toBe(expected);
  });
});
