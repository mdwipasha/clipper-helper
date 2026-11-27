import { SupportedLocale, t } from '@clipper-helper/i18n';
import type { JobEventEta, JobEventRate } from '../ipc/bindings';

function formatDecimalLocale(value: number, decimals: number, locale: SupportedLocale): string {
  const fixed = value.toFixed(decimals);
  return locale === 'id' ? fixed.replace('.', ',') : fixed;
}

/**
 * Formats a point duration in seconds per `PROGRESS_AND_ETA §5` rounding rules:
 * - `< 10 s` -> "beberapa detik lagi"
 * - `10–60 s` -> round to nearest 5 s
 * - `1–10 min` -> round to nearest 15 s
 * - `> 10 min` -> round to nearest 1 min
 */
export function roundDisplaySeconds(seconds: number): number {
  const s = Math.max(0, seconds);
  if (s < 10) return Math.round(s);
  if (s <= 60) return Math.max(10, Math.round(s / 5) * 5);
  if (s <= 600) return Math.max(60, Math.round(s / 15) * 15);
  return Math.max(600, Math.round(s / 60) * 60);
}

export function formatShortDurationUnit(roundedSeconds: number, locale: SupportedLocale): string {
  if (roundedSeconds < 60) {
    return `${roundedSeconds} ${locale === 'id' ? 'dtk' : 's'}`;
  }
  const mins = Math.floor(roundedSeconds / 60);
  const rem = roundedSeconds % 60;
  if (rem === 0) {
    return `${mins} ${locale === 'id' ? 'mnt' : 'min'}`;
  }
  return locale === 'id' ? `${mins} mnt ${rem} dtk` : `${mins} min ${rem} s`;
}

export function formatEtaDisplay(
  eta: JobEventEta | null | undefined,
  locale: SupportedLocale,
  indeterminateStartup = false
): string {
  if (indeterminateStartup || !eta || !Number.isFinite(eta.seconds) || eta.seconds < 0) {
    return t('job.etaCalculating', undefined, locale);
  }

  if (eta.confidence === 'low' && eta.highSeconds > eta.lowSeconds + 5) {
    const lowR = roundDisplaySeconds(eta.lowSeconds);
    const highR = Math.max(lowR + 5, roundDisplaySeconds(eta.highSeconds));
    if (lowR >= 60 && highR >= 60) {
      const lowMin = Math.max(1, Math.round(lowR / 60));
      const highMin = Math.max(lowMin, Math.round(highR / 60));
      if (highMin > lowMin) {
        return t('job.etaRangeMinutes', { min: lowMin, max: highMin }, locale);
      }
    }
    if (highR > lowR) {
      return t(
        'job.etaRangeText',
        {
          low: formatShortDurationUnit(lowR, locale),
          high: formatShortDurationUnit(highR, locale),
        },
        locale
      );
    }
  }

  if (eta.seconds < 10) {
    return t('job.etaFewSeconds', undefined, locale);
  }

  const rounded = roundDisplaySeconds(eta.seconds);
  return t('job.etaApprox', { duration: formatShortDurationUnit(rounded, locale) }, locale);
}

export function formatRateDisplay(
  rate: JobEventRate | null | undefined,
  locale: SupportedLocale
): string | null {
  if (!rate || !Number.isFinite(rate.value) || rate.value <= 0) {
    return null;
  }
  if (rate.unit === 'media_seconds_per_sec' || rate.unit === 'realtime') {
    return t('job.rateRealtime', { speed: formatDecimalLocale(rate.value, 2, locale) }, locale);
  }
  return `${formatDecimalLocale(rate.value, 1, locale)} ${rate.unit}`;
}

function formatCompactDurationRange(
  lowRoundedSeconds: number,
  highRoundedSeconds: number,
  locale: SupportedLocale
): string {
  if (lowRoundedSeconds < 60 && highRoundedSeconds < 60) {
    return `${lowRoundedSeconds}–${highRoundedSeconds} ${locale === 'id' ? 'dtk' : 's'}`;
  }
  return `${formatShortDurationUnit(lowRoundedSeconds, locale)}–${formatShortDurationUnit(
    highRoundedSeconds,
    locale
  )}`;
}

/**
 * Formats pre-download range-picker estimate using `PROGRESS_AND_ETA §5` rounding rules:
 * e.g. `"sekitar 35 dtk (30–45 dtk)"`.
 */
export function formatDownloadEstimateSummary(
  estimateSeconds: number,
  locale: SupportedLocale
): string {
  const pointSec = Number.isFinite(estimateSeconds) && estimateSeconds > 0 ? estimateSeconds : 10;
  const lowRaw = Math.max(1, pointSec * 0.85);
  const highRaw = Math.max(lowRaw, pointSec * 1.3);

  const pointText =
    pointSec < 10
      ? t('job.etaFewSeconds', undefined, locale)
      : formatShortDurationUnit(roundDisplaySeconds(pointSec), locale);

  const lowRounded = roundDisplaySeconds(lowRaw);
  const highRounded = Math.max(lowRounded, roundDisplaySeconds(highRaw));
  const rangeText = formatCompactDurationRange(lowRounded, highRounded, locale);

  return t('range.downloadEstimateFormatted', { point: pointText, range: rangeText }, locale);
}
