#!/usr/bin/env node
/**
 * tools/src/check-manifest-sequence.js
 *
 * Compares CLIPPER_MIN_MANIFEST_SEQUENCE against the latest signed manifest sequence
 * and exits non-zero if CLIPPER_MIN_MANIFEST_SEQUENCE is unset, invalid, < 1, or lower
 * than the signed manifest sequence (ARCHITECTURE §13, SECURITY §10).
 */

import * as fs from 'node:fs';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const DEFAULT_MANIFEST_PATH = path.resolve(__dirname, '../dev-fixtures/manifest.json');

export function checkManifestSequence({
  minSequenceEnv = process.env.CLIPPER_MIN_MANIFEST_SEQUENCE,
  manifestPath = DEFAULT_MANIFEST_PATH,
} = {}) {
  if (
    minSequenceEnv === undefined ||
    minSequenceEnv === null ||
    String(minSequenceEnv).trim() === ''
  ) {
    return {
      ok: false,
      error: 'CLIPPER_MIN_MANIFEST_SEQUENCE is unset or empty.',
    };
  }

  const parsedMin = Number(String(minSequenceEnv).trim());
  if (!Number.isInteger(parsedMin) || parsedMin < 1) {
    return {
      ok: false,
      error: `CLIPPER_MIN_MANIFEST_SEQUENCE must be an integer >= 1, got '${minSequenceEnv}'.`,
    };
  }

  if (!fs.existsSync(manifestPath)) {
    return {
      ok: false,
      error: `Manifest file not found at '${manifestPath}'.`,
    };
  }

  const sigPath = `${manifestPath}.sig`;
  if (!fs.existsSync(sigPath)) {
    return {
      ok: false,
      error: `Detached signature file missing at '${sigPath}'. Refusing unsigned manifest.`,
    };
  }

  let manifestObj;
  try {
    manifestObj = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
  } catch (err) {
    return {
      ok: false,
      error: `Failed to parse manifest JSON '${manifestPath}': ${err.message}`,
    };
  }

  let sigObj;
  try {
    sigObj = JSON.parse(fs.readFileSync(sigPath, 'utf8'));
  } catch (err) {
    return {
      ok: false,
      error: `Failed to parse signature JSON '${sigPath}': ${err.message}`,
    };
  }

  if (!sigObj || typeof sigObj.kid !== 'string' || typeof sigObj.signature !== 'string') {
    return {
      ok: false,
      error: `Malformed detached signature in '${sigPath}'.`,
    };
  }

  const manifestSeq = manifestObj.sequence;
  if (!Number.isInteger(manifestSeq) || manifestSeq < 1) {
    return {
      ok: false,
      error: `Manifest sequence in '${manifestPath}' is invalid: ${manifestSeq}.`,
    };
  }

  if (parsedMin < manifestSeq) {
    return {
      ok: false,
      minSequence: parsedMin,
      manifestSequence: manifestSeq,
      error: `CLIPPER_MIN_MANIFEST_SEQUENCE (${parsedMin}) is lower than latest signed manifest sequence (${manifestSeq}). Raise CLIPPER_MIN_MANIFEST_SEQUENCE to at least ${manifestSeq}.`,
    };
  }

  return {
    ok: true,
    minSequence: parsedMin,
    manifestSequence: manifestSeq,
  };
}

if (process.argv[1] && path.resolve(process.argv[1]) === path.resolve(__filename)) {
  let manifestPath = DEFAULT_MANIFEST_PATH;
  const args = process.argv.slice(2);
  for (let i = 0; i < args.length; i++) {
    if (args[i] === '--manifest' && args[i + 1]) {
      manifestPath = path.resolve(args[i + 1]);
      i++;
    }
  }

  const result = checkManifestSequence({ manifestPath });
  if (!result.ok) {
    console.error(`[check-manifest-sequence] FAIL: ${result.error}`);
    process.exit(1);
  }
  console.log(
    `[check-manifest-sequence] OK: CLIPPER_MIN_MANIFEST_SEQUENCE (${result.minSequence}) >= manifest sequence (${result.manifestSequence})`
  );
}
