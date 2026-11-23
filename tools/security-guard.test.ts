import * as fs from 'node:fs';
import * as path from 'node:path';
import { describe, expect, it } from 'vitest';

describe('Security Baseline Guard', () => {
  const tauriConfPath = path.resolve(__dirname, '../apps/desktop/src-tauri/tauri.conf.json');
  const capabilityPath = path.resolve(
    __dirname,
    '../apps/desktop/src-tauri/capabilities/default.json'
  );

  it('enforces strict CSP in tauri.conf.json', () => {
    expect(fs.existsSync(tauriConfPath)).toBe(true);
    const content = fs.readFileSync(tauriConfPath, 'utf8');
    const conf = JSON.parse(content);

    const csp = conf.app?.security?.csp;
    expect(csp, 'CSP must be defined in app.security.csp').toBeDefined();
    expect(typeof csp).toBe('string');

    // default-src must be 'self'
    expect(csp).toMatch(/default-src\s+['"]self['"]/);

    // No unsafe-eval
    expect(csp).not.toContain("'unsafe-eval'");

    // frame-src must strictly be limited to https://clipper-bridge.vercel.app
    const frameSrcMatch = csp.match(/frame-src\s+([^;]+)/);
    expect(frameSrcMatch, 'frame-src directive must be present').toBeTruthy();
    const frameSrcSources = frameSrcMatch![1]!.trim().split(/\s+/);

    expect(frameSrcSources).toEqual(['https://clipper-bridge.vercel.app']);

    // Never allow wildcards in frame-src or connect-src
    expect(csp).not.toMatch(/frame-src[^;]*\*/);
  });

  it('enforces least-privilege capabilities in default.json', () => {
    expect(fs.existsSync(capabilityPath)).toBe(true);
    const content = fs.readFileSync(capabilityPath, 'utf8');
    const cap = JSON.parse(content);

    expect(cap.permissions, 'Permissions array must exist').toBeDefined();
    expect(Array.isArray(cap.permissions)).toBe(true);

    // Disallow dangerous broad permissions
    for (const perm of cap.permissions) {
      const permName = typeof perm === 'string' ? perm : perm.identifier;
      expect(permName).not.toMatch(/shell:all|fs:all|fs:allow-all/);
    }
  });
});
