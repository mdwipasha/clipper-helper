import { describe, expect, it } from 'vitest';
import { DEFAULT_LOCALE, t } from '../src/index.js';

describe('i18n', () => {
  it('defaults to Indonesian (id)', () => {
    expect(DEFAULT_LOCALE).toBe('id');
    expect(t('app.welcome')).toBe('Selamat datang di ClipperHelper');
  });

  it('interpolates parameters correctly', () => {
    expect(t('app.version', { version: '0.1.0' })).toBe('Versi 0.1.0');
    expect(t('app.version', { version: '0.1.0' }, 'en')).toBe('Version 0.1.0');
  });

  it('falls back to English when Indonesian key is missing', () => {
    expect(t('app.welcome', undefined, 'en')).toBe('Welcome to ClipperHelper');
  });

  it('returns key if not found in any locale', () => {
    expect(t('non.existent.key')).toBe('non.existent.key');
  });
});
