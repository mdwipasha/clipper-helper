import { describe, expect, it } from 'vitest';
import { CampaignTemplate, CampaignTemplateSchema, CaptionStyleSchema } from '../src/template.js';

describe('CaptionStyleSchema', () => {
  it('accepts valid caption animations including phrase', () => {
    const validAnimations = ['none', 'phrase', 'word_highlight', 'pop'] as const;
    for (const anim of validAnimations) {
      const style = {
        fontFamily: 'Montserrat',
        fontSizePct: 5.5,
        weight: 700,
        color: '#FFFFFF',
        outlineColor: '#000000',
        outlinePx: 2,
        shadow: true,
        positionAnchor: 'bottom',
        offsetYPct: 15,
        maxWordsPerLine: 5,
        maxLines: 2,
        animation: anim,
        highlightColor: '#FFD700',
        uppercase: true,
      };
      expect(CaptionStyleSchema.parse(style).animation).toBe(anim);
    }
  });

  it('rejects unsupported animation type', () => {
    const invalidStyle = {
      fontFamily: 'Montserrat',
      fontSizePct: 5.5,
      weight: 700,
      color: '#FFFFFF',
      outlineColor: '#000000',
      outlinePx: 2,
      shadow: true,
      positionAnchor: 'bottom',
      offsetYPct: 15,
      maxWordsPerLine: 5,
      maxLines: 2,
      animation: 'karaoke_bounce',
      highlightColor: '#FFD700',
      uppercase: true,
    };
    expect(() => CaptionStyleSchema.parse(invalidStyle)).toThrow();
  });
});

describe('CampaignTemplateSchema', () => {
  const validTemplate: CampaignTemplate = {
    schemaVersion: 1,
    id: '123e4567-e89b-12d3-a456-426614174000',
    name: 'TikTok Shorts Standard',
    description: 'Default vertical campaign template',
    platforms: ['tiktok', 'youtube_shorts'],
    format: {
      width: 1080,
      height: 1920,
      fps: 30,
      minDurationSec: 15,
      maxDurationSec: 60,
    },
    layout: {
      default: 'fill',
      safeZone: {
        top: 0.1,
        bottom: 0.15,
        left: 0.05,
        right: 0.05,
      },
    },
    captions: {
      enabled: true,
      language: 'id',
      fontFamily: 'Poppins',
      fontSizePct: 6.0,
      weight: 800,
      color: '#FFFFFF',
      outlineColor: '#000000',
      outlinePx: 3,
      shadow: true,
      positionAnchor: 'bottom',
      offsetYPct: 20,
      maxWordsPerLine: 4,
      maxLines: 2,
      animation: 'phrase',
      highlightColor: '#FFFF00',
      uppercase: true,
    },
    branding: {},
    audio: {
      normalize: {
        enabled: true,
        targetLufs: -14.0,
      },
      music: 'forbidden',
    },
    postText: {
      captionTemplate: 'Check this out! #gaming',
      hashtags: ['gaming', 'streamer'],
      mentions: [],
    },
    rules: [
      {
        id: 'r1',
        type: 'duration_range',
        minSec: 15,
        maxSec: 60,
        severity: 'error',
      },
      {
        id: 'r2',
        type: 'fps',
        value: 30,
        severity: 'error',
      },
    ],
  };

  it('validates a correct template', () => {
    expect(CampaignTemplateSchema.parse(validTemplate)).toEqual(validTemplate);
  });

  it('rejects invalid schemaVersion', () => {
    const invalid = { ...validTemplate, schemaVersion: 2 };
    expect(() => CampaignTemplateSchema.parse(invalid)).toThrow();
  });

  it('rejects invalid resolution in format', () => {
    const invalid = {
      ...validTemplate,
      format: {
        ...validTemplate.format,
        width: 720,
      },
    };
    expect(() => CampaignTemplateSchema.parse(invalid)).toThrow();
  });

  it('rejects rules exceeding cap (> 20 rules)', () => {
    const tooManyRules = Array.from({ length: 21 }, (_, i) => ({
      id: `r${i}`,
      type: 'captions_present' as const,
      severity: 'error' as const,
    }));
    const invalid = {
      ...validTemplate,
      rules: tooManyRules,
    };
    expect(() => CampaignTemplateSchema.parse(invalid)).toThrow();
  });
});
