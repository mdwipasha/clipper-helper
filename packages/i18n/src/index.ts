import enTranslations from './locales/en.json' with { type: 'json' };
import idTranslations from './locales/id.json' with { type: 'json' };

export type SupportedLocale = 'id' | 'en';
export const DEFAULT_LOCALE: SupportedLocale = 'id';

type TranslationsRecord = Record<string, unknown>;

const locales: Record<SupportedLocale, TranslationsRecord> = {
  id: idTranslations as unknown as TranslationsRecord,
  en: enTranslations as unknown as TranslationsRecord,
};

function resolveKey(obj: TranslationsRecord, path: string): string | undefined {
  const parts = path.split('.');
  let current: unknown = obj;
  for (const part of parts) {
    if (typeof current !== 'object' || current === null) {
      return undefined;
    }
    current = (current as Record<string, unknown>)[part];
  }
  return typeof current === 'string' ? current : undefined;
}

/**
 * Translates a key using id as default and en as fallback.
 * Interpolates variables formatted as {varName}.
 */
export function t(
  key: string,
  params?: Record<string, string | number>,
  locale: SupportedLocale = DEFAULT_LOCALE
): string {
  let value = resolveKey(locales[locale], key);
  if (!value && locale !== 'en') {
    value = resolveKey(locales.en, key);
  }
  if (!value) {
    return key;
  }
  if (params) {
    return value.replace(/\{(\w+)\}/g, (_, match: string) => {
      return params[match] !== undefined ? String(params[match]) : `{${match}}`;
    });
  }
  return value;
}

export { idTranslations, enTranslations };
