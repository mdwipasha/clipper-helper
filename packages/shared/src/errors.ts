import { z } from 'zod';

/**
 * Standard error codes per PROGRESS_AND_ETA §8.
 */
export const ErrorCodeSchema = z.enum([
  'E_NET_OFFLINE',
  'E_NET_TIMEOUT',
  'E_YT_URL_INVALID',
  'E_YT_UNAVAILABLE',
  'E_YT_LIVE_NOT_ENDED',
  'E_YT_BOT_CHECK',
  'E_YT_RATE_LIMITED',
  'E_YT_EXTRACT_FAILED',
  'E_RANGE_INVALID',
  'E_RANGE_TOO_LONG',
  'E_DISK_LOW',
  'E_DISK_FULL',
  'E_MANIFEST_INVALID',
  'E_MANIFEST_STALE',
  'E_COMPONENT_MISSING',
  'E_MODEL_DOWNLOAD_FAILED',
  'E_TRANSCRIBE_FAILED',
  'E_TRANSCRIBE_OOM',
  'E_ENCODER_UNAVAILABLE',
  'E_RENDER_FAILED',
  'E_VERIFY_FAILED',
  'E_AUTH_EXPIRED',
  'E_NO_CREDITS',
  'E_DEVICE_LIMIT',
  'E_DEVICE_INVALID',
  'E_DEVICE_LIMIT_FREE',
  'E_REEXPORT_CAP',
  'E_ACCOUNT_SUSPENDED',
  'E_APP_OUTDATED',
  'E_SERVER_UNAVAILABLE',
  'E_TICKET_INVALID',
  'E_PAYMENT_PENDING',
  'E_PAYMENT_FAILED',
  'E_PAYMENT_EXPIRED',
  'E_TEMPLATE_INVALID',
  'E_PREVIEW_CONFIG',
  'E_UNKNOWN',
]);

export type ErrorCode = z.infer<typeof ErrorCodeSchema>;

/**
 * Specific reasons for E_YT_UNAVAILABLE per PRD FR-SRC-2 and PROGRESS_AND_ETA §8.
 */
export const YtUnavailableReasonSchema = z.enum([
  'members_only',
  'age_restricted',
  'region_blocked',
  'private',
  'deleted',
  'unknown',
]);

export type YtUnavailableReason = z.infer<typeof YtUnavailableReasonSchema>;

/**
 * Detailed structured error payload used in job events and UI notifications.
 */
export const AppErrorSchema = z.object({
  code: ErrorCodeSchema,
  messageKey: z.string(),
  retryable: z.boolean(),
  actionKeys: z.array(z.string()),
  detail: z.record(z.union([z.string(), z.number(), z.boolean()])).optional(),
  ytUnavailableReason: YtUnavailableReasonSchema.optional(),
});

export type AppError = z.infer<typeof AppErrorSchema>;
