import { execSync } from 'node:child_process';
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

    // media-src must allow self + asset protocol strictly without wildcards
    const mediaSrcMatch = csp.match(/media-src\s+([^;]+)/);
    expect(mediaSrcMatch, 'media-src directive must be present').toBeTruthy();
    const mediaSrcSources = mediaSrcMatch![1]!.trim().split(/\s+/);
    expect(mediaSrcSources).toContain("'self'");
    expect(mediaSrcSources).toContain('asset:');
    expect(csp).not.toMatch(/media-src[^;]*\*/);
  });

  it('asserts devtools are disabled in release configuration', () => {
    const content = fs.readFileSync(tauriConfPath, 'utf8');
    const conf = JSON.parse(content);

    for (const win of conf.app?.windows ?? []) {
      expect(win.devtools, 'devtools must not be explicitly enabled on windows').not.toBe(true);
    }
    expect(conf.build?.devtools, 'build.devtools must not be enabled').not.toBe(true);
  });

  it('asserts asset protocol scope is restricted strictly to cache and projects dirs', () => {
    const content = fs.readFileSync(tauriConfPath, 'utf8');
    const conf = JSON.parse(content);

    const assetProtocol = conf.app?.security?.assetProtocol;
    expect(assetProtocol, 'assetProtocol configuration must exist').toBeDefined();
    expect(assetProtocol.enable).toBe(true);
    expect(Array.isArray(assetProtocol.scope)).toBe(true);
    expect(assetProtocol.scope).toEqual([
      '$LOCALAPPDATA/ClipperHelper/cache/**',
      '$LOCALAPPDATA/ClipperHelper/projects/**',
      '$LOCALAPPDATA/ClipperHelper-Dev/cache/**',
      '$LOCALAPPDATA/ClipperHelper-Dev/projects/**',
    ]);

    for (const entry of assetProtocol.scope) {
      expect(entry).toMatch(
        /^(\$LOCALAPPDATA|%LOCALAPPDATA%)[/\\]ClipperHelper(-Dev)?[/\\](cache|projects)[/\\]\*\*$/
      );
      expect(entry).not.toContain('..');
    }
  });

  it('enforces exact narrow capability set for dialog (dir open, .txt save) and opener ($LOCALDATA logs only)', () => {
    expect(fs.existsSync(capabilityPath)).toBe(true);
    const content = fs.readFileSync(capabilityPath, 'utf8');
    const cap = JSON.parse(content);

    expect(cap.permissions, 'Permissions array must exist').toBeDefined();
    expect(Array.isArray(cap.permissions)).toBe(true);

    // Enforce the EXACT permission set — fail on any extra, broad, or unexpected capability.
    expect(cap.permissions).toEqual([
      'core:default',
      'core:event:default',
      {
        identifier: 'opener:allow-open-path',
        allow: [
          { path: '$LOCALDATA/ClipperHelper/logs' },
          { path: '$LOCALDATA/ClipperHelper-Dev/logs' },
        ],
      },
      'dialog:allow-open',
      'dialog:allow-save',
    ]);

    for (const perm of cap.permissions) {
      const permName = typeof perm === 'string' ? perm : perm.identifier;
      expect(permName).not.toMatch(/^(fs|shell):/);
      expect(permName).not.toMatch(/fs:all|shell:all|allow-all/);
      expect(permName).not.toBe('dialog:default');
      expect(permName).not.toBe('opener:default');
      expect(permName).not.toMatch(/opener:allow-(default-urls|open-url|reveal-item-in-dir)/);
      expect(permName).not.toMatch(/dev_/);
    }

    // Defense-in-depth: verify frontend dialog plugin calls are strictly directory-open and .txt-save only.
    const srcRoot = path.resolve(__dirname, '../apps/desktop/src');
    const uiFiles: string[] = [];
    const walk = (dir: string) => {
      for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) {
          if (entry.name !== 'test' && entry.name !== 'ipc') walk(full);
        } else if (/\.(ts|tsx)$/.test(entry.name)) {
          uiFiles.push(full);
        }
      }
    };
    walk(srcRoot);

    const dialogUsers = uiFiles.filter((f) =>
      fs.readFileSync(f, 'utf8').includes('@tauri-apps/plugin-dialog')
    );
    expect(dialogUsers.map((f) => path.basename(f)).sort()).toEqual([
      'ComponentsView.tsx',
      'DiagnosticsView.tsx',
      'SettingsView.tsx',
    ]);

    for (const fileName of ['components/ComponentsView.tsx', 'components/SettingsView.tsx']) {
      const src = fs.readFileSync(path.join(srcRoot, fileName), 'utf8');
      expect(src).toMatch(/open\(\{\s*directory:\s*true,\s*multiple:\s*false/);
      expect(src).not.toMatch(/\bsave\s*\(/);
    }

    const diagnosticsSrc = fs.readFileSync(
      path.join(srcRoot, 'components/DiagnosticsView.tsx'),
      'utf8'
    );
    expect(diagnosticsSrc).toMatch(/save\(\{[\s\S]*extensions:\s*\['txt'\]/);
    expect(diagnosticsSrc).not.toMatch(/\bopen\s*\(/);

    // Defense-in-depth: frontend must NEVER import @tauri-apps/plugin-opener (external URLs open strictly via Rust open_video_in_browser)
    const openerUsers = uiFiles.filter((f) =>
      fs.readFileSync(f, 'utf8').includes('@tauri-apps/plugin-opener')
    );
    expect(openerUsers).toEqual([]);
    const rangePickerSrc = fs.readFileSync(
      path.join(srcRoot, 'components/RangePickerView.tsx'),
      'utf8'
    );
    expect(rangePickerSrc).toContain('commands.openVideoInBrowser(');
  });

  it('asserts dev commands and dev panel are strictly gated to debug assertions and DEV environment', () => {
    // Check lib.rs has dev_commands gated with #[cfg(debug_assertions)]
    const libRsPath = path.resolve(__dirname, '../apps/desktop/src-tauri/src/lib.rs');
    expect(fs.existsSync(libRsPath)).toBe(true);
    const libContent = fs.readFileSync(libRsPath, 'utf8');

    // dev_commands module must be cfg(debug_assertions)
    expect(libContent).toMatch(/#\[cfg\(debug_assertions\)\]\s+pub mod dev_commands;/);

    // Production commands must appear in release builder without any dev_ commands
    expect(libContent).toMatch(
      /#\[cfg\(not\(debug_assertions\)\)\]\s+pub fn create_specta_builder\(\)[\s\S]*?commands::get_app_info[\s\S]*?commands::get_workspace_status[\s\S]*?commands::install_all_required_components[\s\S]*?commands::generate_diagnostics_report[\s\S]*?commands::open_logs_folder[\s\S]*?commands::open_video_in_browser[\s\S]*?\]\)/
    );
    expect(libContent).not.toMatch(/#\[cfg\(not\(debug_assertions\)\)\][\s\S]*?dev_/);

    // Check App.tsx gates DevPanel with import.meta.env.DEV
    const appTsxPath = path.resolve(__dirname, '../apps/desktop/src/App.tsx');
    expect(fs.existsSync(appTsxPath)).toBe(true);
    const appContent = fs.readFileSync(appTsxPath, 'utf8');
    expect(appContent).toContain('import.meta.env.DEV');
    expect(appContent).toMatch(/import\.meta\.env\.DEV\s*&&\s*<DevPanel\s*\/>/);

    // Check ComponentsView gates Debug Manifest URL behind import.meta.env.DEV
    const componentsTsxPath = path.resolve(
      __dirname,
      '../apps/desktop/src/components/ComponentsView.tsx'
    );
    const componentsContent = fs.readFileSync(componentsTsxPath, 'utf8');
    expect(componentsContent).toMatch(
      /import\.meta\.env\.DEV\s*&&\s*overview\?\.manifest_url_config\.can_override_in_ui/
    );

    // Inspect exact Rust release command list for zero debug-only commands
    const releaseBlockMatch = libContent.match(
      /#\[cfg\(not\(debug_assertions\)\)\]\s+pub fn create_specta_builder\(\)[\s\S]*?\.commands\(tauri_specta::collect_commands!\[([\s\S]*?)\]\)/
    );
    expect(releaseBlockMatch, 'Release collect_commands block must exist').toBeTruthy();
    const releaseCmds = releaseBlockMatch![1]!
      .split(',')
      .map((s) => s.trim())
      .filter(Boolean);
    expect(releaseCmds.length).toBeGreaterThan(10);
    for (const cmd of releaseCmds) {
      expect(cmd.startsWith('commands::')).toBe(true);
      expect(cmd).not.toMatch(/dev_|debug_/i);
    }

    // Check built production frontend bundle (apps/desktop/dist/assets) excludes debug controls
    const distAssetsDir = path.resolve(__dirname, '../apps/desktop/dist/assets');
    if (fs.existsSync(distAssetsDir)) {
      const jsFiles = fs.readdirSync(distAssetsDir).filter((f) => f.endsWith('.js'));
      expect(jsFiles.length).toBeGreaterThan(0);
      for (const jsFile of jsFiles) {
        const bundle = fs.readFileSync(path.join(distAssetsDir, jsFile), 'utf8');
        expect(bundle).not.toContain('Debug Manifest URL');
        expect(bundle).not.toContain('debug-manifest-url');
        expect(bundle).not.toContain('Force timecode-only');
        expect(bundle).not.toContain('dev_run_fake_sidecar');
        expect(bundle).not.toContain('[DEV] Debug Panel');
      }
    }
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
      expect(str.includes('dev_start_fixture_server')).toBe(false);
      expect(str.includes('dev_fetch_manifest')).toBe(false);
      expect(str.includes('dev_download_component')).toBe(false);
      expect(str.includes('dev_install_component')).toBe(false);
      expect(str.includes('dev_get_component_diagnostics')).toBe(false);
      expect(str.includes('dev_rollback_component')).toBe(false);
      expect(str.includes('dev_gc_component')).toBe(false);
      expect(str.includes('dev_verify_component')).toBe(false);
      expect(str.includes('dev_retry_quarantined_component')).toBe(false);
      expect(str.includes('dev_repair_component')).toBe(false);
      expect(str.includes('dev_reset_manifest_sequence_floor')).toBe(false);
      expect(str.includes('dev_set_custom_ffmpeg')).toBe(false);
      expect(str.includes('dev_clear_custom_ffmpeg')).toBe(false);
      expect(str.includes('find_node_executable')).toBe(false);
      expect(str.includes('CLIPPER_DEV_NODE_PATH')).toBe(false);
      expect(str.includes('fnm_multishells')).toBe(false);
      expect(str.includes('NVM_HOME')).toBe(false);
      expect(str.includes('where.exe')).toBe(false);
    }
  });

  it('asserts Node discovery (nvm/fnm/where.exe) exists strictly under cfg(debug_assertions)', () => {
    const srcDir = path.resolve(__dirname, '../apps/desktop/src-tauri/src');
    const rustFiles = fs.readdirSync(srcDir).filter((f) => f.endsWith('.rs'));

    const devCmdsContent = fs.readFileSync(path.join(srcDir, 'dev_commands.rs'), 'utf8');
    expect(devCmdsContent).toContain('#![cfg(debug_assertions)]');

    for (const file of rustFiles) {
      if (file === 'dev_commands.rs' || file === 'lib.rs') continue;
      const content = fs.readFileSync(path.join(srcDir, file), 'utf8');
      expect(content).not.toMatch(
        /find_node_executable|where\.exe|fnm_multishells|NVM_HOME|NVM_SYMLINK/
      );
    }
  });

  it('asserts no private keys or dev key files are tracked or committed in git', () => {
    const trackedFiles = execSync('git ls-files', { encoding: 'utf8' })
      .split('\n')
      .map((f: string) => f.trim())
      .filter(Boolean);

    for (const file of trackedFiles) {
      expect(file).not.toMatch(/\.dev-keys\//);
      expect(file).not.toMatch(/\.(priv|priv\.hex|pem|key)$/i);
      expect(file).not.toMatch(/id_ed25519/i);
    }
  });
});
