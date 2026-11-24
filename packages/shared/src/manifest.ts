import { z } from 'zod';

/**
 * File descriptor inside an archive component.
 */
export const ManifestFileEntrySchema = z.object({
  path: z.string().min(1),
  sha256: z.string().regex(/^[a-f0-9]{64}$/i, 'Must be valid 64-character lowercase hex SHA-256'),
  size_bytes: z.number().int().nonnegative(),
});

export type ManifestFileEntry = z.infer<typeof ManifestFileEntrySchema>;

/**
 * Component entry inside the component manifest.
 */
export const ComponentManifestEntrySchema = z.object({
  component: z.string().min(1),
  version: z.string().min(1),
  platform: z.string().min(1),
  url: z.string().url(),
  size_bytes: z.number().int().nonnegative(),
  sha256: z.string().regex(/^[a-f0-9]{64}$/i, 'Must be valid 64-character lowercase hex SHA-256'),
  archive: z.enum(['none', 'zip']),
  files: z.array(ManifestFileEntrySchema).default([]),
  min_app_version: z.string().min(1),
  max_app_version: z.string().min(1).nullable().optional(),
  rollout_percent: z.number().int().min(0).max(100).default(100),
});

export type ComponentManifestEntry = z.infer<typeof ComponentManifestEntrySchema>;

/**
 * Signed component manifest root schema.
 */
export const ComponentManifestSchema = z.object({
  schema: z.literal(1),
  sequence: z.number().int().positive(),
  generated_at: z.string().datetime(),
  channel: z.enum(['stable', 'beta', 'dev']),
  components: z.array(ComponentManifestEntrySchema),
});

export type ComponentManifest = z.infer<typeof ComponentManifestSchema>;

/**
 * Detached Ed25519 signature schema for manifest.json.sig.
 */
export const ManifestSignatureSchema = z.object({
  kid: z.string().min(1),
  signature: z.string().min(1),
});

export type ManifestSignature = z.infer<typeof ManifestSignatureSchema>;
