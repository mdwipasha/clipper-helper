import { describe, expect, it } from 'vitest';
import { AppErrorSchema, ErrorCodeSchema, YtUnavailableReasonSchema } from '../src/errors.js';

describe('ErrorCodeSchema', () => {
  it('accepts all known E_* codes including bot check and preview config', () => {
    expect(ErrorCodeSchema.parse('E_YT_BOT_CHECK')).toBe('E_YT_BOT_CHECK');
    expect(ErrorCodeSchema.parse('E_PREVIEW_CONFIG')).toBe('E_PREVIEW_CONFIG');
    expect(ErrorCodeSchema.parse('E_YT_UNAVAILABLE')).toBe('E_YT_UNAVAILABLE');
    expect(ErrorCodeSchema.parse('E_NO_CREDITS')).toBe('E_NO_CREDITS');
  });

  it('rejects invalid error codes', () => {
    expect(() => ErrorCodeSchema.parse('E_UNKNOWN_CODE')).toThrow();
    expect(() => ErrorCodeSchema.parse('')).toThrow();
  });
});

describe('YtUnavailableReasonSchema', () => {
  it('accepts exact reasons per PRD FR-SRC-2', () => {
    const valid = [
      'members_only',
      'age_restricted',
      'region_blocked',
      'private',
      'deleted',
      'unknown',
    ];
    for (const r of valid) {
      expect(YtUnavailableReasonSchema.parse(r)).toBe(r);
    }
  });

  it('rejects invalid reason', () => {
    expect(() => YtUnavailableReasonSchema.parse('copyright')).toThrow();
  });
});

describe('AppErrorSchema', () => {
  it('validates a structured error with valid code and actions', () => {
    const error = {
      code: 'E_YT_UNAVAILABLE',
      messageKey: 'err.E_YT_UNAVAILABLE.title',
      retryable: false,
      actionKeys: ['action.try_other'],
      ytUnavailableReason: 'members_only',
    };
    expect(AppErrorSchema.parse(error)).toEqual(error);
  });

  it('rejects error without messageKey or invalid retryable flag', () => {
    expect(() =>
      AppErrorSchema.parse({
        code: 'E_UNKNOWN',
        retryable: 'yes',
      })
    ).toThrow();
  });
});
