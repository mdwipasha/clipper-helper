#!/usr/bin/env node
/**
 * tools/src/manifest-signer.js
 *
 * Generates Ed25519 dev keys, builds component manifests with SHA-256 and size computation,
 * and produces detached signatures for manifest.json.sig (ARCHITECTURE §9, SECURITY §8).
 */

import * as crypto from 'node:crypto';
import * as fs from 'node:fs';
import * as path from 'node:path';

/**
 * Generates an Ed25519 keypair and writes to outputDir.
 * Private key is never committed (.gitignore + CI guard).
 */
export function generateDevKeyPair(kid, outputDir = '.dev-keys') {
  if (!fs.existsSync(outputDir)) {
    fs.mkdirSync(outputDir, { recursive: true });
  }

  const { privateKey, publicKey } = crypto.generateKeyPairSync('ed25519');

  const privDer = privateKey.export({ type: 'pkcs8', format: 'der' });
  const pubDer = publicKey.export({ type: 'spki', format: 'der' });

  // In Ed25519 SPKI DER (44 bytes total), the raw 32-byte public key is the last 32 bytes
  const rawPubKey32 = pubDer.subarray(pubDer.length - 32);

  const privPath = path.join(outputDir, `${kid}.priv.hex`);
  const pubPath = path.join(outputDir, `${kid}.pub.hex`);

  fs.writeFileSync(privPath, privDer.toString('hex'), { mode: 0o600 });
  fs.writeFileSync(pubPath, rawPubKey32.toString('hex'));

  return {
    kid,
    privPath,
    pubPath,
    pubKeyHex: rawPubKey32.toString('hex'),
  };
}

/**
 * Computes SHA-256 lowercase hex checksum of a buffer or file.
 */
export function computeSha256(filePathOrBuffer) {
  const hash = crypto.createHash('sha256');
  if (typeof filePathOrBuffer === 'string') {
    const data = fs.readFileSync(filePathOrBuffer);
    hash.update(data);
  } else {
    hash.update(filePathOrBuffer);
  }
  return hash.digest('hex');
}

/**
 * Signs exact manifest bytes using an Ed25519 private key.
 * Returns detached signature object { kid, signature: hex }.
 */
export function signManifestBytes(manifestBytes, privKeyDerHex, kid) {
  const privKeyDer = Buffer.from(privKeyDerHex.trim(), 'hex');
  const privateKey = crypto.createPrivateKey({
    key: privKeyDer,
    format: 'der',
    type: 'pkcs8',
  });

  const sig = crypto.sign(null, manifestBytes, privateKey);
  return {
    kid,
    signature: sig.toString('hex'),
  };
}

/**
 * Builds a manifest object from component entries and metadata.
 */
export function buildManifest({ sequence = 1, channel = 'stable', components = [] }) {
  return {
    schema: 1,
    sequence,
    generated_at: new Date().toISOString(),
    channel,
    components,
  };
}

// CLI handler if invoked directly
if (
  process.argv[1] &&
  path.resolve(process.argv[1]) ===
    path.resolve(new URL(import.meta.url).pathname.replace(/^\/([A-Z]:)/, '$1'))
) {
  const args = process.argv.slice(2);
  const command = args[0];

  if (command === 'generate-key') {
    const kid = args[1] || `dev-key-${Date.now()}`;
    const outDir = args[2] || '.dev-keys';
    const res = generateDevKeyPair(kid, outDir);
    console.log(`Generated Ed25519 keypair:`);
    console.log(`  KID:        ${res.kid}`);
    console.log(`  Public Key: ${res.pubKeyHex}`);
    console.log(`  Private:    ${res.privPath}`);
    console.log(`  Public:     ${res.pubPath}`);
  } else if (command === 'sign') {
    const manifestPath = args[1];
    const keyPath = args[2];
    const kid = args[3];

    if (!manifestPath || !keyPath || !kid) {
      console.error('Usage: manifest-signer sign <manifest.json> <priv_key_file> <kid>');
      process.exit(1);
    }

    const manifestBytes = fs.readFileSync(manifestPath);
    const privHex = fs.readFileSync(keyPath, 'utf8');
    const sig = signManifestBytes(manifestBytes, privHex, kid);
    const sigPath = `${manifestPath}.sig`;
    fs.writeFileSync(sigPath, JSON.stringify(sig, null, 2));
    console.log(`Signed ${manifestPath} -> ${sigPath}`);
  } else {
    console.log('Usage:');
    console.log('  manifest-signer generate-key [kid] [outDir]');
    console.log('  manifest-signer sign <manifest.json> <priv_key_file> <kid>');
  }
}
