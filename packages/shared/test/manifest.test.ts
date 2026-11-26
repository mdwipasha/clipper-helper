import { describe, expect, it } from 'vitest';
import { ComponentManifestSchema, ManifestSignatureSchema } from '../src/manifest.js';

describe('Component Manifest Schema', () => {
  const validManifest = {
    schema: 1,
    sequence: 42,
    generated_at: '2026-11-24T12:00:00.000Z',
    channel: 'stable' as const,
    components: [
      {
        component: 'yt-dlp',
        version: '2026.10.26',
        platform: 'windows-x86_64',
        url: 'https://cdn.clipperhelper.com/components/yt-dlp/yt-dlp.exe',
        size_bytes: 24746393,
        sha256: 'f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d',
        archive: 'none' as const,
        files: [],
        min_app_version: '0.1.0',
        max_app_version: null,
        rollout_percent: 100,
      },
    ],
  };

  it('validates a complete, conformant manifest', () => {
    const res = ComponentManifestSchema.safeParse(validManifest);
    expect(res.success).toBe(true);
  });

  it('rejects schema versions other than 1', () => {
    const res = ComponentManifestSchema.safeParse({ ...validManifest, schema: 2 });
    expect(res.success).toBe(false);
  });

  it('rejects invalid sha256 checksums', () => {
    const badSha = {
      ...validManifest,
      components: [
        {
          ...validManifest.components[0]!,
          sha256: 'not-a-valid-sha256',
        },
      ],
    };
    const res = ComponentManifestSchema.safeParse(badSha);
    expect(res.success).toBe(false);
  });

  it('rejects non-url component URLs', () => {
    const badUrl = {
      ...validManifest,
      components: [
        {
          ...validManifest.components[0]!,
          url: 'invalid-url-string',
        },
      ],
    };
    const res = ComponentManifestSchema.safeParse(badUrl);
    expect(res.success).toBe(false);
  });

  it('validates detached signature payload', () => {
    const validSig = {
      kid: 'clipper-manifest-2026-1',
      signature: 'a'.repeat(128),
    };
    const res = ManifestSignatureSchema.safeParse(validSig);
    expect(res.success).toBe(true);
  });
});
