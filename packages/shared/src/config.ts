import { z } from 'zod';

export const MaintenanceConfigSchema = z.object({
  enabled: z.boolean(),
  message: z.string().nullable(),
});

export type MaintenanceConfig = z.infer<typeof MaintenanceConfigSchema>;

export const LimitsConfigSchema = z.object({
  max_range_seconds: z.number().int().positive(),
  max_clip_seconds: z.number().int().positive(),
  reexport_tolerance_ms: z.number().int().nonnegative(),
  reexport_daily_cap: z.number().int().positive(),
  reexport_total_cap: z.number().int().positive(),
  free_device_daily_cap: z.number().int().positive(),
  max_segments: z.number().int().positive(),
  device_swap_cooldown_days: z.number().int().nonnegative(),
});

export type LimitsConfig = z.infer<typeof LimitsConfigSchema>;

export const TicketConfigSchema = z.object({
  ttl_seconds: z.number().int().positive(),
  active_kid: z.string().nullable(),
  next_kid: z.string().nullable(),
});

export type TicketConfig = z.infer<typeof TicketConfigSchema>;

export const FlagsConfigSchema = z.object({
  caption_lead_offset_ms: z.number().int(),
});

export type FlagsConfig = z.infer<typeof FlagsConfigSchema>;

export const AppConfigSchema = z.object({
  min_app_version: z.string(),
  maintenance: MaintenanceConfigSchema,
  limits: LimitsConfigSchema,
  ticket: TicketConfigSchema,
  flags: FlagsConfigSchema,
});

export type AppConfig = z.infer<typeof AppConfigSchema>;

/**
 * Seed defaults per DATA_MODEL §1.
 */
export const SEED_APP_CONFIG: AppConfig = {
  min_app_version: '0.0.0',
  maintenance: {
    enabled: false,
    message: null,
  },
  limits: {
    max_range_seconds: 300,
    max_clip_seconds: 300,
    reexport_tolerance_ms: 15000,
    reexport_daily_cap: 30,
    reexport_total_cap: 200,
    free_device_daily_cap: 5,
    max_segments: 20,
    device_swap_cooldown_days: 7,
  },
  ticket: {
    ttl_seconds: 900,
    active_kid: null,
    next_kid: null,
  },
  flags: {
    caption_lead_offset_ms: 0,
  },
};
