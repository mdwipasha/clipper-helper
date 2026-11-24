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

    // No unsafe-eval or unsafe-inline in script-src / default-src
    expect(csp).not.toContain("'unsafe-eval'");
    const scriptSrcMatch = csp.match(/script-src\s+([^;]+)/);
    if (scriptSrcMatch) {
      expect(scriptSrcMatch[1]).not.toContain("'unsafe-inline'");
      expect(scriptSrcMatch[1]).not.toContain("'unsafe-eval'");
    }

    // connect-src: only 'self' and the two documented placeholders (*.supabase.co and *.r2.cloudflarestorage.com)
    const connectSrcMatch = csp.match(/connect-src\s+([^;]+)/);
    expect(connectSrcMatch, 'connect-src directive must be present').toBeTruthy();
    const connectSources = connectSrcMatch![1]!.trim().split(/\s+/);
    for (const source of connectSources) {
      if (source.includes('*')) {
        expect(['https://*.supabase.co', 'https://*.r2.cloudflarestorage.com']).toContain(source);
      }
    }

    // frame-src must strictly be limited to https://clipper-bridge.vercel.app
    const frameSrcMatch = csp.match(/frame-src\s+([^;]+)/);
    expect(frameSrcMatch, 'frame-src directive must be present').toBeTruthy();
    const frameSrcSources = frameSrcMatch![1]!.trim().split(/\s+/);
    expect(frameSrcSources).toEqual(['https://clipper-bridge.vercel.app']);
    expect(csp).not.toMatch(/frame-src[^;]*\*/);
  });

  it('asserts devtools are disabled in release configuration', () => {
    const content = fs.readFileSync(tauriConfPath, 'utf8');
    const conf = JSON.parse(content);

    for (const win of conf.app?.windows ?? []) {
      expect(win.devtools, 'devtools must not be explicitly enabled on windows').not.toBe(true);
    }
    expect(conf.build?.devtools, 'build.devtools must not be enabled').not.toBe(true);
  });

  it('asserts asset protocol scope is restricted strictly to the workspace directory', () => {
    const content = fs.readFileSync(tauriConfPath, 'utf8');
    const conf = JSON.parse(content);

    const assetProtocol = conf.app?.security?.assetProtocol;
    expect(assetProtocol, 'assetProtocol configuration must exist').toBeDefined();
    expect(assetProtocol.enable).toBe(true);
    expect(Array.isArray(assetProtocol.scope)).toBe(true);

    for (const entry of assetProtocol.scope) {
      expect(entry).toMatch(/^(\$LOCALAPPDATA|%LOCALAPPDATA%)[/\\]ClipperHelper(-Dev)?[/\\]/);
      expect(entry).not.toContain('..');
    }
  });

  it('enforces least-privilege capabilities and rejects broad fs/shell permissions', () => {
    expect(fs.existsSync(capabilityPath)).toBe(true);
    const content = fs.readFileSync(capabilityPath, 'utf8');
    const cap = JSON.parse(content);

    expect(cap.permissions, 'Permissions array must exist').toBeDefined();
    expect(Array.isArray(cap.permissions)).toBe(true);

    // Disallow dangerous broad permissions (fs:*, shell:*)
    for (const perm of cap.permissions) {
      const permName = typeof perm === 'string' ? perm : perm.identifier;
      expect(permName).not.toMatch(/^(fs|shell):/);
      expect(permName).not.toMatch(/fs:all|shell:all|allow-all/);
      // Disallow dev commands in production default capabilities
      expect(permName).not.toMatch(/dev_/);
    }
  });

  it('asserts dev commands and dev panel are strictly gated to debug assertions and DEV environment', () => {
    // Check lib.rs has dev_commands gated with #[cfg(debug_assertions)]
    const libRsPath = path.resolve(__dirname, '../apps/desktop/src-tauri/src/lib.rs');
    expect(fs.existsSync(libRsPath)).toBe(true);
    const libContent = fs.readFileSync(libRsPath, 'utf8');

    // dev_commands module must be cfg(debug_assertions)
    expect(libContent).toMatch(/#\[cfg\(debug_assertions\)\]\s+pub mod dev_commands;/);

    // dev commands must only be registered in debug builder
    expect(libContent).toMatch(
      /#\[cfg\(not\(debug_assertions\)\)\]\s+pub fn create_specta_builder\(\)[\s\S]*?commands::get_app_info[\s\S]*?commands::get_workspace_status\s*,?\s*\]\)/
    );
    expect(libContent).not.toMatch(/#\[cfg\(not\(debug_assertions\)\)\][\s\S]*?dev_/);

    // Check App.tsx gates DevPanel with import.meta.env.DEV
    const appTsxPath = path.resolve(__dirname, '../apps/desktop/src/App.tsx');
    expect(fs.existsSync(appTsxPath)).toBe(true);
    const appContent = fs.readFileSync(appTsxPath, 'utf8');
    expect(appContent).toContain('import.meta.env.DEV');
    expect(appContent).toMatch(/import\.meta\.env\.DEV\s*&&\s*<DevPanel\s*\/>/);
  });

  it('asserts dev commands are absent from release binary if built', () => {
    const releaseExePath = path.resolve(
      __dirname,
      '../apps/desktop/src-tauri/target/release/clipper-helper-desktop.exe'
    );
    if (fs.existsSync(releaseExePath)) {
      const buffer = fs.readFileSync(releaseExePath);
      const str = buffer.toString('latin1');
      expect(str.includes('dev_run_fake_sidecar')).toBe(false);
      expect(str.includes('dev_get_system_info')).toBe(false);
      expect(str.includes('dev_reset_workspace')).toBe(false);
    }
  });
});
