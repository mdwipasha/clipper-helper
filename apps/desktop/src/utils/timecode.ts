export const MAX_TIMECODE_SECONDS = 86_400;

export interface RangeValidationInput {
  startSeconds: number | null;
  endSeconds: number | null;
  videoDurationSeconds: number;
  maxRangeSeconds: number;
}

export type RangeValidationResult =
  | {
      valid: true;
      startSeconds: number;
      endSeconds: number;
      durationSeconds: number;
    }
  | {
      valid: false;
      code: 'E_RANGE_INVALID' | 'E_RANGE_TOO_LONG';
      reasonKey: string;
    };

/**
 * Parses a YouTube range timecode (`1:02:03`, `62:03`, `3723`, optional `.s`) into seconds.
 * Rejects negative values, malformed strings, and values > 86,400 s.
 */
export function parseTimecode(raw: string): number | null {
  const trimmed = raw.trim();
  if (!trimmed) return null;

  // Accept only digits, colons, and at most one decimal dot in the final segment.
  if (!/^\d+(?::\d+){0,2}(?:\.\d+)?$/.test(trimmed)) {
    return null;
  }

  const parts = trimmed.split(':');
  let total = 0;

  if (parts.length === 1) {
    total = Number(parts[0]);
  } else if (parts.length === 2) {
    const mins = Number(parts[0]);
    const secs = Number(parts[1]);
    if (!Number.isInteger(mins) || mins < 0 || !Number.isFinite(secs) || secs < 0 || secs >= 60) {
      return null;
    }
    total = mins * 60 + secs;
  } else if (parts.length === 3) {
    const hours = Number(parts[0]);
    const mins = Number(parts[1]);
    const secs = Number(parts[2]);
    if (
      !Number.isInteger(hours) ||
      hours < 0 ||
      !Number.isInteger(mins) ||
      mins < 0 ||
      mins >= 60 ||
      !Number.isFinite(secs) ||
      secs < 0 ||
      secs >= 60
    ) {
      return null;
    }
    total = hours * 3600 + mins * 60 + secs;
  } else {
    return null;
  }

  if (!Number.isFinite(total) || total < 0 || total > MAX_TIMECODE_SECONDS) {
    return null;
  }

  return Number(total.toFixed(3));
}

/**
 * Formats seconds cleanly as `HH:MM:SS` (with optional single decimal `.s` if fractional).
 */
export function formatTimecodeInput(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) {
    return '00:00:00';
  }
  const rounded = Math.round(seconds * 10) / 10;
  const whole = Math.floor(rounded);
  const tenth = Math.round((rounded - whole) * 10);

  const h = Math.floor(whole / 3600);
  const m = Math.floor((whole % 3600) / 60);
  const s = whole % 60;
  const pad = (n: number) => String(n).padStart(2, '0');

  const base = `${pad(h)}:${pad(m)}:${pad(s)}`;
  return tenth > 0 ? `${base}.${tenth}` : base;
}

/**
 * Validates `0 <= start < end <= videoDuration`, min `1 s`, and max `maxRangeSeconds` (default 300 s).
 */
export function validateClipRange(input: RangeValidationInput): RangeValidationResult {
  const { startSeconds, endSeconds, videoDurationSeconds, maxRangeSeconds } = input;
  if (
    startSeconds === null ||
    endSeconds === null ||
    !Number.isFinite(startSeconds) ||
    !Number.isFinite(endSeconds)
  ) {
    return {
      valid: false,
      code: 'E_RANGE_INVALID',
      reasonKey: 'range.errInvalidFormat',
    };
  }

  if (startSeconds < 0 || endSeconds <= startSeconds) {
    return {
      valid: false,
      code: 'E_RANGE_INVALID',
      reasonKey: 'range.errStartBeforeEnd',
    };
  }

  if (videoDurationSeconds > 0 && endSeconds > videoDurationSeconds) {
    return {
      valid: false,
      code: 'E_RANGE_INVALID',
      reasonKey: 'range.errExceedsDuration',
    };
  }

  const durationSeconds = Number((endSeconds - startSeconds).toFixed(3));
  if (durationSeconds < 1) {
    return {
      valid: false,
      code: 'E_RANGE_INVALID',
      reasonKey: 'range.errMinDuration',
    };
  }

  if (durationSeconds > maxRangeSeconds) {
    return {
      valid: false,
      code: 'E_RANGE_TOO_LONG',
      reasonKey: 'range.errMaxDuration',
    };
  }

  return {
    valid: true,
    startSeconds,
    endSeconds,
    durationSeconds,
  };
}

/**
 * Estimates initial download duration in seconds from S1 conservative priors (`range / 1.66 + 10 s`).
 */
export function estimateDownloadSeconds(
  rangeSeconds: number,
  rtFactor = 1.66,
  startupSeconds = 10
): number {
  if (
    !Number.isFinite(rangeSeconds) ||
    rangeSeconds <= 0 ||
    !Number.isFinite(rtFactor) ||
    rtFactor <= 0
  ) {
    return Math.max(0, startupSeconds);
  }
  const startup = Number.isFinite(startupSeconds) && startupSeconds >= 0 ? startupSeconds : 10;
  return Number((rangeSeconds / rtFactor + startup).toFixed(2));
}

export function formatEstimateMinutes(estimateSeconds: number): number {
  if (!Number.isFinite(estimateSeconds) || estimateSeconds <= 0) {
    return 1;
  }
  return Math.max(1, Math.round(estimateSeconds / 60));
}

/**
 * Strictly constructs a canonical external YouTube watch URL (`https://www.youtube.com/watch?v=<id>[&t=<sec>]`)
 * matching the narrowed Tauri opener URL capability (`https://www.youtube.com/watch?v=*`).
 */
export function buildAllowedYouTubeWatchUrl(videoId: string, startSeconds?: number): string {
  const cleanId = videoId.trim();
  if (!/^[A-Za-z0-9_-]{11}$/.test(cleanId)) {
    throw new Error(`Invalid YouTube video ID: ${videoId}`);
  }
  const base = `https://www.youtube.com/watch?v=${cleanId}`;
  if (typeof startSeconds === 'number' && Number.isFinite(startSeconds) && startSeconds > 0) {
    const sec = Math.min(MAX_TIMECODE_SECONDS, Math.floor(startSeconds));
    if (sec > 0) {
      return `${base}&t=${sec}`;
    }
  }
  return base;
}
