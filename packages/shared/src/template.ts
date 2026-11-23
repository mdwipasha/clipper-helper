import { z } from 'zod';

export const HexColorRegex = /^#([0-9a-fA-F]{3}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/;

export const AnchorSchema = z.enum([
  'top_left',
  'top_right',
  'bottom_left',
  'bottom_right',
  'top_center',
  'bottom_center',
  'center',
]);

export type Anchor = z.infer<typeof AnchorSchema>;

export const RectSchema = z.object({
  x: z.number().min(0).max(1),
  y: z.number().min(0).max(1),
  w: z.number().min(0).max(1),
  h: z.number().min(0).max(1),
});

export type Rect = z.infer<typeof RectSchema>;

export const CaptionAnimationSchema = z.enum(['none', 'phrase', 'word_highlight', 'pop']);

export type CaptionAnimation = z.infer<typeof CaptionAnimationSchema>;

export const CaptionStyleSchema = z.object({
  fontFamily: z.enum(['Montserrat', 'Poppins', 'Inter', 'Anton']),
  fontSizePct: z.number().positive().max(100),
  weight: z.union([z.literal(400), z.literal(600), z.literal(700), z.literal(800)]),
  color: z.string().regex(HexColorRegex, 'Invalid hex color'),
  outlineColor: z.string().regex(HexColorRegex, 'Invalid hex color'),
  outlinePx: z.number().nonnegative(),
  shadow: z.boolean(),
  positionAnchor: z.enum(['top', 'center', 'bottom']),
  offsetYPct: z.number().min(-100).max(100),
  maxWordsPerLine: z.number().int().positive().max(50),
  maxLines: z.union([z.literal(1), z.literal(2), z.literal(3)]),
  animation: CaptionAnimationSchema,
  highlightColor: z.string().regex(HexColorRegex, 'Invalid hex color'),
  uppercase: z.boolean(),
});

export type CaptionStyle = z.infer<typeof CaptionStyleSchema>;

export const RuleSeveritySchema = z.enum(['error', 'warn']);

export const RuleSchema = z.discriminatedUnion('type', [
  z.object({
    id: z.string(),
    type: z.literal('duration_range'),
    minSec: z.number().nonnegative(),
    maxSec: z.number().positive(),
    severity: RuleSeveritySchema,
  }),
  z.object({
    id: z.string(),
    type: z.literal('resolution'),
    width: z.number().int().positive(),
    height: z.number().int().positive(),
    severity: RuleSeveritySchema,
  }),
  z.object({
    id: z.string(),
    type: z.literal('fps'),
    value: z.union([z.literal(30), z.literal(60)]),
    severity: RuleSeveritySchema,
  }),
  z.object({
    id: z.string(),
    type: z.literal('max_file_size'),
    mb: z.number().positive(),
    severity: RuleSeveritySchema,
  }),
  z.object({
    id: z.string(),
    type: z.literal('logo_present'),
    severity: RuleSeveritySchema,
  }),
  z.object({
    id: z.string(),
    type: z.literal('captions_present'),
    severity: RuleSeveritySchema,
  }),
  z.object({
    id: z.string(),
    type: z.literal('audio_present'),
    severity: RuleSeveritySchema,
  }),
  z.object({
    id: z.string(),
    type: z.literal('post_text_copied'),
    severity: z.literal('warn'),
  }),
  z.object({
    id: z.string(),
    type: z.literal('manual_confirm'),
    label: z.string().max(500),
    severity: z.literal('warn'),
  }),
]);

export type Rule = z.infer<typeof RuleSchema>;

export const CampaignTemplateSchema = z.object({
  schemaVersion: z.literal(1),
  id: z.string().uuid(),
  name: z.string().min(1).max(80),
  description: z.string().max(500).optional(),
  briefUrl: z.string().url().max(500).optional(),
  platforms: z.array(z.enum(['tiktok', 'instagram_reels', 'youtube_shorts'])).min(1),
  format: z.object({
    width: z.literal(1080),
    height: z.literal(1920),
    fps: z.union([z.literal(30), z.literal(60)]),
    minDurationSec: z.number().nonnegative(),
    maxDurationSec: z.number().positive(),
    maxFileSizeMB: z.number().positive().optional(),
  }),
  layout: z.object({
    default: z.enum(['fill', 'blur_fit', 'split']),
    safeZone: z.object({
      top: z.number().min(0).max(0.4),
      bottom: z.number().min(0).max(0.4),
      left: z.number().min(0).max(0.4),
      right: z.number().min(0).max(0.4),
    }),
    split: z
      .object({
        top: RectSchema,
        bottom: RectSchema,
        ratio: z.number().min(0.3).max(0.7),
      })
      .optional(),
  }),
  captions: CaptionStyleSchema.extend({
    enabled: z.boolean(),
    language: z.enum(['id', 'en', 'auto']),
  }),
  branding: z.object({
    logo: z
      .object({
        assetId: z.string().uuid(),
        anchor: AnchorSchema,
        marginPct: z.number().min(0).max(100),
        widthPct: z.number().positive().max(100),
        opacity: z.number().min(0).max(1),
        mandatory: z.boolean(),
      })
      .optional(),
    fixedText: z
      .array(
        z.object({
          text: z.string().max(500),
          anchor: AnchorSchema,
          sizePct: z.number().positive().max(100),
          color: z.string().regex(HexColorRegex, 'Invalid hex color'),
        })
      )
      .optional(),
  }),
  audio: z.object({
    normalize: z.object({
      enabled: z.boolean(),
      targetLufs: z.number().min(-70).max(0),
    }),
    music: z.enum(['allowed', 'forbidden']),
  }),
  postText: z.object({
    captionTemplate: z.string().max(2000),
    hashtags: z.array(z.string().max(100)),
    mentions: z.array(z.string().max(100)),
    disclosure: z.string().max(500).optional(),
  }),
  rules: z.array(RuleSchema).max(20),
});

export type CampaignTemplate = z.infer<typeof CampaignTemplateSchema>;
