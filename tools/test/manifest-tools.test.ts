import * as fs from 'node:fs';
import * as path from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { startDevServer } from '../src/dev-server.js';
import {
  buildManifest,
  computeSha256,
  generateDevKeyPair,
  signManifestBytes,
} from '../src/manifest-signer.js';

describe('Manifest Signer & Dev Server Tools', () => {
  const tmpDir = path.resolve(__dirname, '../../target/test-tools-tmp');
  let devServer: Awaited<ReturnType<typeof startDevServer>>;

  beforeAll(async () => {
    fs.mkdirSync(tmpDir, { recursive: true });
    // Write sample artifact
    fs.writeFileSync(
      path.join(tmpDir, 'test-component.bin'),
      Buffer.from('hello-world-payload-bytes')
    );
    devServer = await startDevServer({ port: 0, rootDir: tmpDir });
  });

  afterAll(async () => {
    await devServer.close();
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it('generates dev Ed25519 keypair and exports raw 32-byte public key', () => {
    const keysDir = path.join(tmpDir, 'keys');
    const { kid, pubKeyHex, privPath, pubPath } = generateDevKeyPair('test-kid-1', keysDir);

    expect(kid).toBe('test-kid-1');
    expect(pubKeyHex).toMatch(/^[a-f0-9]{64}$/);
    expect(fs.existsSync(privPath)).toBe(true);
    expect(fs.existsSync(pubPath)).toBe(true);
  });

  it('builds, hashes, and signs manifest bytes with Ed25519 signature', () => {
    const keysDir = path.join(tmpDir, 'keys');
    const { kid, privPath } = generateDevKeyPair('test-kid-2', keysDir);

    const artifactPath = path.join(tmpDir, 'test-component.bin');
    const sha = computeSha256(artifactPath);
    const size = fs.statSync(artifactPath).size;

    const manifestObj = buildManifest({
      sequence: 1,
      channel: 'dev',
      components: [
        {
          component: 'test-comp',
          version: '1.0.0',
          platform: 'windows-x86_64',
          url: `${devServer.url}/test-component.bin`,
          size_bytes: size,
          sha256: sha,
          archive: 'none',
          files: [],
          min_app_version: '0.1.0',
          rollout_percent: 100,
        },
      ],
    });

    const manifestBytes = Buffer.from(JSON.stringify(manifestObj, null, 2), 'utf8');
    const privHex = fs.readFileSync(privPath, 'utf8');
    const sig = signManifestBytes(manifestBytes, privHex, kid);

    expect(sig.kid).toBe('test-kid-2');
    expect(sig.signature).toMatch(/^[a-f0-9]{128}$/);
  });

  it('serves static files with ETag and Accept-Ranges', async () => {
    const res = await fetch(`${devServer.url}/test-component.bin`);
    expect(res.status).toBe(200);
    expect(res.headers.get('accept-ranges')).toBe('bytes');
    expect(res.headers.get('etag')).toBeTruthy();
    const text = await res.text();
    expect(text).toBe('hello-world-payload-bytes');
  });

  it('serves Range requests with 206 Partial Content', async () => {
    const res = await fetch(`${devServer.url}/test-component.bin`, {
      headers: { Range: 'bytes=0-4' },
    });
    expect(res.status).toBe(206);
    expect(res.headers.get('content-range')).toMatch(/^bytes 0-4\//);
    const text = await res.text();
    expect(text).toBe('hello');
  });

  it('handles wrong_content test mode by tampering bytes', async () => {
    const res = await fetch(`${devServer.url}/test-component.bin?test_mode=wrong_content`);
    const text = await res.text();
    expect(text).not.toBe('hello-world-payload-bytes');
  });

  it('handles redirect test mode', async () => {
    const res = await fetch(
      `${devServer.url}/test-component.bin?test_mode=redirect&target=https://cdn.example.com/test`,
      { redirect: 'manual' }
    );
    expect(res.status).toBe(302);
    expect(res.headers.get('location')).toBe('https://cdn.example.com/test');
  });
});
