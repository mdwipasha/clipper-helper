import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import {
  commands,
  DevFixtureServerStatus,
  DevManifestError,
  DevManifestInfo,
  DevSidecarRunResult,
  DevSystemInfo,
} from '../ipc/bindings';

export type DevDownloadProgressEvent = {
  component: string;
  stage: string;
  unit: string;
  completed_bytes: number;
  total_bytes: number;
  percent: number;
  rate_bps: number;
  eta_seconds: number | null;
};

export function DevPanel() {
  const [sysInfo, setSysInfo] = useState<DevSystemInfo | null>(null);
  const [sysError, setSysError] = useState<string | null>(null);
  const [resetting, setResetting] = useState<boolean>(false);

  const [mode, setMode] = useState<string>('normal');
  const [running, setRunning] = useState<boolean>(false);
  const [runResult, setRunResult] = useState<DevSidecarRunResult | null>(null);
  const [runError, setRunError] = useState<string | null>(null);

  // Fixture Server State
  const [serverStatus, setServerStatus] = useState<DevFixtureServerStatus>({
    state: 'stopped',
    port: null,
    url: null,
    error: null,
  });
  const [serverStarting, setServerStarting] = useState<boolean>(false);
  const [autoStartServer, setAutoStartServer] = useState<boolean>(true);

  // Components & Manifest State
  const [manifestUrl, setManifestUrl] = useState<string>('http://127.0.0.1:8080/manifest.json');
  const [sigUrl, setSigUrl] = useState<string>('http://127.0.0.1:8080/manifest.json.sig');
  const [fetchingManifest, setFetchingManifest] = useState<boolean>(false);
  const [manifestInfo, setManifestInfo] = useState<DevManifestInfo | null>(null);
  const [manifestError, setManifestError] = useState<DevManifestError | null>(null);

  const [downloadProgressMap, setDownloadProgressMap] = useState<
    Record<string, DevDownloadProgressEvent>
  >({});
  const [downloadingMap, setDownloadingMap] = useState<Record<string, boolean>>({});
  const [downloadErrors, setDownloadErrors] = useState<Record<string, string>>({});
  const [downloadedSet, setDownloadedSet] = useState<Set<string>>(new Set());

  const loadSysInfo = () => {
    commands
      .devGetSystemInfo()
      .then((res) => {
        if (res.status === 'ok') {
          setSysInfo(res.data);
          setSysError(null);
        } else {
          setSysError(res.error);
        }
      })
      .catch((err: unknown) => {
        setSysError(String(err));
      });
  };

  const handleDownload = async (c: {
    component: string;
    version: string;
    url: string;
    size_bytes: number;
    sha256: string;
    archive: string;
  }) => {
    setDownloadingMap((prev) => ({ ...prev, [c.component]: true }));
    setDownloadErrors((prev) => {
      const copy = { ...prev };
      delete copy[c.component];
      return copy;
    });

    try {
      const res = await commands.devDownloadComponent(
        c.component,
        c.version,
        c.url,
        c.size_bytes,
        c.sha256,
        c.archive
      );
      if (res.status === 'ok') {
        setDownloadedSet((prev) => new Set(prev).add(c.component));
      } else {
        setDownloadErrors((prev) => ({ ...prev, [c.component]: res.error }));
      }
    } catch (err: unknown) {
      setDownloadErrors((prev) => ({ ...prev, [c.component]: String(err) }));
    } finally {
      setDownloadingMap((prev) => ({ ...prev, [c.component]: false }));
    }
  };

  const refreshServerStatus = async () => {
    try {
      const status = await commands.devGetFixtureServerStatus();
      setServerStatus(status);
      if (status.url) {
        setManifestUrl(`${status.url}/manifest.json`);
        setSigUrl(`${status.url}/manifest.json.sig`);
      }
      return status;
    } catch {
      return null;
    }
  };

  const handleStartFixtureServer = async () => {
    setServerStarting(true);
    setManifestError(null);
    try {
      const res = await commands.devStartFixtureServer();
      if (res.status === 'ok') {
        setServerStatus(res.data);
        if (res.data.url) {
          setManifestUrl(`${res.data.url}/manifest.json`);
          setSigUrl(`${res.data.url}/manifest.json.sig`);
        }
      } else {
        setServerStatus({
          state: 'error',
          port: null,
          url: null,
          error: res.error,
        });
      }
    } catch (err: unknown) {
      setServerStatus({
        state: 'error',
        port: null,
        url: null,
        error: String(err),
      });
    } finally {
      setServerStarting(false);
    }
  };

  const handleStopFixtureServer = async () => {
    try {
      const res = await commands.devStopFixtureServer();
      if (res.status === 'ok') {
        setServerStatus(res.data);
      }
    } catch (err: unknown) {
      console.error('Failed to stop fixture server:', err);
    }
  };

  const handleFetchManifest = async (mUrl?: string, sUrl?: string) => {
    const targetM = mUrl || manifestUrl;
    const targetS = sUrl || sigUrl;
    setFetchingManifest(true);
    setManifestError(null);
    try {
      const res = await commands.devFetchManifest(targetM, targetS);
      if (res.status === 'ok') {
        setManifestInfo(res.data);
        const downloaded = new Set<string>();
        for (const c of res.data.components) {
          if (c.is_downloaded) {
            downloaded.add(c.component);
          }
        }
        setDownloadedSet(downloaded);
      } else {
        setManifestError(res.error);
        setManifestInfo(null);
      }
    } catch (err: unknown) {
      setManifestError({
        code: 'E_NET_OFFLINE',
        message: String(err),
      });
      setManifestInfo(null);
    } finally {
      setFetchingManifest(false);
    }
  };

  useEffect(() => {
    loadSysInfo();
    // Run initial sidecar test to show exit code 0 on initial view
    commands
      .devRunFakeSidecar('normal')
      .then((res) => {
        if (res.status === 'ok') {
          setRunResult(res.data);
        }
      })
      .catch((err: unknown) => {
        setRunError(String(err));
      });

    // Initialize fixture server and fetch manifest
    (async () => {
      let currentStatus = await refreshServerStatus();
      if (autoStartServer && currentStatus?.state !== 'running') {
        setServerStarting(true);
        try {
          const startRes = await commands.devStartFixtureServer();
          if (startRes.status === 'ok') {
            currentStatus = startRes.data;
            setServerStatus(startRes.data);
            if (startRes.data.url) {
              setManifestUrl(`${startRes.data.url}/manifest.json`);
              setSigUrl(`${startRes.data.url}/manifest.json.sig`);
            }
          }
        } catch (e) {
          console.warn('Auto-start fixture server failed:', e);
        } finally {
          setServerStarting(false);
        }
      }

      // If server is running, auto-fetch
      if (currentStatus?.state === 'running') {
        const mUrl = `${currentStatus.url || 'http://127.0.0.1:8080'}/manifest.json`;
        const sUrl = `${currentStatus.url || 'http://127.0.0.1:8080'}/manifest.json.sig`;
        await handleFetchManifest(mUrl, sUrl);
      }
    })();

    // Listen for streaming download progress events from core
    const unlistenPromise = listen<DevDownloadProgressEvent>('download-progress', (event) => {
      setDownloadProgressMap((prev) => ({
        ...prev,
        [event.payload.component]: event.payload,
      }));
      if (event.payload.percent >= 100) {
        setDownloadedSet((prev) => new Set(prev).add(event.payload.component));
        setDownloadingMap((prev) => ({
          ...prev,
          [event.payload.component]: false,
        }));
      }
    });

    return () => {
      unlistenPromise.then((fn) => fn());
    };
  }, []);

  const handleFetchTamperedManifest = async () => {
    const base = 'http://127.0.0.1:8080';
    const tManifest = `${base}/manifest-tampered.json`;
    const tSig = `${base}/manifest-tampered.json.sig`;
    setManifestUrl(tManifest);
    setSigUrl(tSig);
    await handleFetchManifest(tManifest, tSig);
  };

  const formatBytes = (bytes: number) => {
    if (bytes >= 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
    if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${bytes} B`;
  };

  const handleResetWorkspace = async () => {
    setResetting(true);
    try {
      const res = await commands.devResetWorkspace();
      if (res.status === 'ok') {
        loadSysInfo();
      } else {
        setSysError(res.error);
      }
    } catch (err: unknown) {
      setSysError(String(err));
    } finally {
      setResetting(false);
    }
  };

  const handleRun = async () => {
    setRunning(true);
    setRunError(null);
    try {
      const res = await commands.devRunFakeSidecar(mode);
      if (res.status === 'ok') {
        setRunResult(res.data);
      } else {
        setRunError(res.error);
      }
    } catch (err: unknown) {
      setRunError(String(err));
    } finally {
      setRunning(false);
    }
  };

  return (
    <div
      data-testid="dev-panel"
      className="mt-6 border-t border-slate-700/60 pt-4 font-mono text-xs space-y-4"
    >
      <div className="flex items-center justify-between">
        <span className="text-amber-400 font-bold uppercase tracking-wider text-[11px]">
          [DEV] Debug Panel
        </span>
        <span className="text-[10px] bg-amber-500/10 text-amber-300 border border-amber-500/20 px-2 py-0.5 rounded">
          cfg(debug_assertions)
        </span>
      </div>

      {/* System & Store Info */}
      <div className="bg-slate-950/80 rounded p-3 border border-slate-800 space-y-2">
        <div className="text-slate-400 font-semibold mb-1">SQLite & Workspace Status</div>
        {sysInfo ? (
          <div className="space-y-1">
            <div className="flex justify-between">
              <span className="text-slate-500">Schema Version:</span>
              <span className="text-cyan-400 font-bold">v{sysInfo.schema_version}</span>
            </div>
            <div className="flex justify-between">
              <span className="text-slate-500">Workspace Root:</span>
              <span
                className="text-slate-300 truncate max-w-[200px]"
                title={sysInfo.redacted_workspace_root}
              >
                {sysInfo.redacted_workspace_root}
              </span>
            </div>
            <div className="flex justify-between">
              <span className="text-slate-500">Free Disk Space:</span>
              <span className="text-emerald-400 font-medium">
                {sysInfo.free_disk_space_formatted}
              </span>
            </div>
            <div className="flex justify-between">
              <span className="text-slate-500">Architecture:</span>
              <span className="text-indigo-300">
                {sysInfo.platform}-{sysInfo.arch}
              </span>
            </div>
            <div className="pt-2 border-t border-slate-800/80 flex justify-end">
              <button
                onClick={handleResetWorkspace}
                disabled={resetting}
                className="bg-rose-900/30 hover:bg-rose-900/50 text-rose-300 border border-rose-800/40 text-[10px] px-2.5 py-1 rounded transition cursor-pointer"
              >
                {resetting ? 'Resetting...' : 'Reset Dev Workspace'}
              </button>
            </div>
          </div>
        ) : sysError ? (
          <div className="text-red-400">Error: {sysError}</div>
        ) : (
          <div className="text-slate-500 animate-pulse">Loading system info...</div>
        )}
      </div>

      {/* Components & Signed Manifest Section */}
      <div
        data-testid="components-section"
        className="bg-slate-950/80 rounded p-3 border border-slate-800 space-y-3"
      >
        <div className="flex items-center justify-between border-b border-slate-800/80 pb-2">
          <div className="flex items-center space-x-2">
            <span className="text-slate-400 font-semibold">Fixture Server:</span>
            {serverStatus.state === 'running' ? (
              <span className="bg-emerald-950/60 border border-emerald-800 text-emerald-400 px-2 py-0.5 rounded text-[10px] font-bold">
                RUNNING {serverStatus.port ? `:${serverStatus.port}` : ''}
              </span>
            ) : serverStarting || serverStatus.state === 'starting' ? (
              <span className="bg-amber-950/60 border border-amber-800 text-amber-300 px-2 py-0.5 rounded text-[10px] animate-pulse">
                STARTING...
              </span>
            ) : serverStatus.state === 'error' ? (
              <span className="bg-rose-950/60 border border-rose-800 text-rose-300 px-2 py-0.5 rounded text-[10px]">
                ERROR
              </span>
            ) : (
              <span className="bg-slate-900 border border-slate-700 text-slate-400 px-2 py-0.5 rounded text-[10px]">
                STOPPED
              </span>
            )}
          </div>

          <div className="flex items-center space-x-2">
            <label className="flex items-center space-x-1 text-[10px] text-slate-400 cursor-pointer select-none">
              <input
                type="checkbox"
                checked={autoStartServer}
                onChange={(e) => setAutoStartServer(e.target.checked)}
                className="rounded bg-slate-900 border-slate-700 text-indigo-600 focus:ring-0 text-[10px]"
              />
              <span>Auto-start</span>
            </label>

            {serverStatus.state === 'running' ? (
              <button
                onClick={handleStopFixtureServer}
                className="text-[10px] bg-slate-800 hover:bg-slate-700 text-slate-300 border border-slate-600 px-2 py-0.5 rounded cursor-pointer"
              >
                Stop Server
              </button>
            ) : (
              <button
                onClick={handleStartFixtureServer}
                disabled={serverStarting}
                className="text-[10px] bg-indigo-900/40 hover:bg-indigo-800/60 text-indigo-200 border border-indigo-700/60 px-2 py-0.5 rounded cursor-pointer"
              >
                {serverStarting ? 'Starting...' : 'Start Local Fixture Server'}
              </button>
            )}
          </div>
        </div>

        {serverStatus.error && (
          <div className="text-rose-400 bg-rose-950/20 border border-rose-900/40 p-1.5 rounded text-[10px]">
            Server error: {serverStatus.error}
          </div>
        )}

        {/* URL Inputs */}
        <div className="space-y-1.5">
          <div className="flex items-center space-x-2">
            <span className="text-slate-500 w-16 text-[11px]">Manifest:</span>
            <input
              type="text"
              value={manifestUrl}
              onChange={(e) => setManifestUrl(e.target.value)}
              className="flex-1 bg-slate-900 border border-slate-700 rounded px-2 py-0.5 text-slate-200 text-[11px] focus:outline-none focus:border-indigo-500"
            />
          </div>
          <div className="flex items-center space-x-2">
            <span className="text-slate-500 w-16 text-[11px]">Signature:</span>
            <input
              type="text"
              value={sigUrl}
              onChange={(e) => setSigUrl(e.target.value)}
              className="flex-1 bg-slate-900 border border-slate-700 rounded px-2 py-0.5 text-slate-200 text-[11px] focus:outline-none focus:border-indigo-500"
            />
          </div>
        </div>

        {/* Fetch Controls */}
        <div className="flex space-x-2">
          <button
            onClick={() => handleFetchManifest()}
            disabled={fetchingManifest}
            className="flex-1 bg-indigo-600 hover:bg-indigo-500 disabled:opacity-50 text-white font-medium py-1 px-3 rounded transition text-xs cursor-pointer"
          >
            {fetchingManifest ? 'Fetching & Verifying...' : 'Fetch Manifest'}
          </button>
          <button
            onClick={handleFetchTamperedManifest}
            disabled={fetchingManifest}
            className="bg-rose-950/50 hover:bg-rose-900/60 text-rose-300 border border-rose-800/60 font-medium py-1 px-2.5 rounded transition text-xs cursor-pointer"
            title="Fetches a manifest with corrupted bytes / invalid signature to verify cryptographic rejection"
          >
            Test Tampered Manifest
          </button>
        </div>

        {/* Refused / Unreachable Banner */}
        {manifestError && (
          <div
            data-testid="manifest-error-banner"
            className={
              manifestError.code === 'E_MANIFEST_INVALID' ||
              manifestError.code === 'E_MANIFEST_STALE'
                ? 'bg-rose-950/30 border border-rose-800/60 p-2.5 rounded text-[11px] space-y-1.5'
                : 'bg-amber-950/20 border border-amber-800/50 p-2.5 rounded text-[11px] space-y-1.5'
            }
          >
            <div className="flex items-center justify-between">
              <div className="flex items-center space-x-2">
                {manifestError.code === 'E_MANIFEST_INVALID' ? (
                  <>
                    <span className="bg-rose-900/60 border border-rose-700 text-rose-200 text-[10px] px-1.5 py-0.5 rounded uppercase font-bold">
                      REFUSED
                    </span>
                    <span className="text-rose-400 font-bold">
                      Cryptographic Verification Failed
                    </span>
                  </>
                ) : manifestError.code === 'E_MANIFEST_STALE' ? (
                  <>
                    <span className="bg-rose-900/60 border border-rose-700 text-rose-200 text-[10px] px-1.5 py-0.5 rounded uppercase font-bold">
                      REFUSED
                    </span>
                    <span className="text-rose-400 font-bold">Sequence Rollback Rejected</span>
                  </>
                ) : (
                  <>
                    <span className="bg-amber-900/60 border border-amber-700 text-amber-200 text-[10px] px-1.5 py-0.5 rounded uppercase font-bold">
                      UNREACHABLE
                    </span>
                    <span className="text-amber-400 font-bold">
                      Server Offline / Unreachable [{manifestError.code}]
                    </span>
                  </>
                )}
              </div>

              {manifestError.code !== 'E_MANIFEST_INVALID' &&
                manifestError.code !== 'E_MANIFEST_STALE' && (
                  <button
                    onClick={() => handleFetchManifest()}
                    disabled={fetchingManifest}
                    className="bg-amber-800/50 hover:bg-amber-700/60 text-amber-200 border border-amber-700/60 px-2 py-0.5 rounded text-[10px] cursor-pointer"
                  >
                    {fetchingManifest ? 'Retrying...' : 'Retry'}
                  </button>
                )}
            </div>

            <div
              className={
                manifestError.code === 'E_MANIFEST_INVALID' ||
                manifestError.code === 'E_MANIFEST_STALE'
                  ? 'text-rose-300 break-all'
                  : 'text-amber-300/90 break-all'
              }
            >
              {manifestError.message}
            </div>
          </div>
        )}

        {/* Verified Manifest Details */}
        {manifestInfo && (
          <div
            data-testid="manifest-info-card"
            className="border-t border-slate-800 pt-2.5 space-y-2.5 text-[11px]"
          >
            <div className="flex items-center justify-between bg-slate-900/80 p-2 rounded border border-slate-800">
              <div className="flex items-center space-x-2">
                <span className="text-slate-500">Signature:</span>
                <span className="bg-emerald-950/60 border border-emerald-800/80 text-emerald-400 font-bold px-2 py-0.5 rounded text-[10px]">
                  VERIFIED (Ed25519)
                </span>
              </div>
              <div className="flex items-center space-x-3 text-slate-400">
                <span>
                  Sequence: <strong className="text-cyan-400">#{manifestInfo.sequence}</strong>
                </span>
                <span>
                  Channel: <strong className="text-indigo-300">{manifestInfo.channel}</strong>
                </span>
              </div>
            </div>

            {/* Components List */}
            <div className="space-y-2">
              <div className="text-slate-400 font-semibold text-[11px]">
                Components ({manifestInfo.components.length})
              </div>

              {manifestInfo.components.map((comp) => {
                const isDownloaded = downloadedSet.has(comp.component);
                const isDownloading = downloadingMap[comp.component];
                const progress = downloadProgressMap[comp.component];
                const error = downloadErrors[comp.component];

                return (
                  <div
                    key={comp.component}
                    className="bg-slate-900/90 border border-slate-800 rounded p-2.5 space-y-2"
                  >
                    <div className="flex items-center justify-between">
                      <div className="space-y-0.5">
                        <div className="text-slate-200 font-bold flex items-center space-x-2">
                          <span>{comp.component}</span>
                          <span className="text-[10px] text-slate-400 font-normal">
                            v{comp.version}
                          </span>
                        </div>
                        <div className="text-[10px] text-slate-500">
                          Size: {formatBytes(comp.size_bytes)} | SHA-256:{' '}
                          <span title={comp.sha256}>{comp.sha256.substring(0, 12)}…</span>
                        </div>
                      </div>

                      <div>
                        {isDownloaded ? (
                          <span className="bg-emerald-950/60 border border-emerald-800/80 text-emerald-400 text-[10px] font-bold px-2 py-1 rounded">
                            Verified & Ready
                          </span>
                        ) : (
                          <button
                            onClick={() => handleDownload(comp)}
                            disabled={isDownloading}
                            className="bg-indigo-600 hover:bg-indigo-500 disabled:opacity-50 text-white text-[11px] font-medium px-3 py-1 rounded cursor-pointer transition"
                          >
                            {isDownloading ? 'Downloading...' : 'Download'}
                          </button>
                        )}
                      </div>
                    </div>

                    {/* Progress Bar & ETA */}
                    {isDownloading && progress && (
                      <div className="space-y-1 bg-black/40 p-2 rounded border border-slate-800 text-[10px]">
                        <div className="flex justify-between text-slate-300">
                          <span>
                            {formatBytes(progress.completed_bytes)} /{' '}
                            {formatBytes(progress.total_bytes)} ({progress.percent.toFixed(1)}%)
                          </span>
                          <span>
                            {progress.rate_bps > 0
                              ? `${formatBytes(progress.rate_bps)}/s`
                              : 'calculating…'}{' '}
                            | ETA:{' '}
                            {progress.eta_seconds !== null && progress.eta_seconds !== undefined
                              ? `${progress.eta_seconds.toFixed(1)}s`
                              : '…'}
                          </span>
                        </div>
                        <div className="w-full bg-slate-800 rounded-full h-1.5 overflow-hidden">
                          <div
                            className="bg-indigo-500 h-1.5 rounded-full transition-all duration-200"
                            style={{ width: `${Math.min(100, Math.max(0, progress.percent))}%` }}
                          />
                        </div>
                      </div>
                    )}

                    {/* Error display */}
                    {error && (
                      <div className="text-rose-400 bg-rose-950/20 border border-rose-900/40 p-1.5 rounded text-[10px]">
                        Download error: {error}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          </div>
        )}
      </div>

      {/* Fake Sidecar Runner */}
      <div className="bg-slate-950/80 rounded p-3 border border-slate-800 space-y-3">
        <div className="text-slate-400 font-semibold">Sidecar Supervisor Test</div>

        <div className="flex space-x-2">
          <select
            value={mode}
            onChange={(e) => setMode(e.target.value)}
            disabled={running}
            className="flex-1 bg-slate-900 border border-slate-700 rounded px-2 py-1 text-slate-200 text-xs focus:outline-none focus:border-indigo-500"
          >
            <option value="normal">normal (exit 0)</option>
            <option value="progress">progress (\r progress stream)</option>
            <option value="sleep 500">sleep 500 (delay test)</option>
            <option value="exit-code 42">exit-code 42 (non-zero test)</option>
            <option value="invalid-utf8">invalid-utf8 (lossy test)</option>
            <option value="grandchild">grandchild (job object test)</option>
            <option value="flood">flood (output cap test)</option>
          </select>

          <button
            onClick={handleRun}
            disabled={running}
            className="bg-indigo-600 hover:bg-indigo-500 disabled:opacity-50 text-white font-medium px-3 py-1 rounded transition text-xs cursor-pointer"
          >
            {running ? 'Running...' : 'Run'}
          </button>
        </div>

        {runError && (
          <div className="text-red-400 bg-red-950/30 p-2 rounded border border-red-800/40">
            {runError}
          </div>
        )}

        {runResult && (
          <div className="space-y-1.5 border-t border-slate-800 pt-2">
            <div className="flex justify-between text-[11px]">
              <span>
                Exit Code:{' '}
                <span
                  className={
                    runResult.exit_code === 0
                      ? 'text-emerald-400 font-bold'
                      : 'text-rose-400 font-bold'
                  }
                >
                  {runResult.exit_code}
                </span>
              </span>
              <span className="text-slate-400">{runResult.elapsed_ms} ms</span>
            </div>

            <div className="bg-black/60 rounded p-2 max-h-32 overflow-y-auto space-y-0.5 font-mono text-[11px] text-slate-300">
              {runResult.lines.length > 0 ? (
                runResult.lines.map((l, idx) => (
                  <div key={idx} className="whitespace-pre-wrap break-all">
                    {l}
                  </div>
                ))
              ) : (
                <div className="text-slate-500 italic">(no stdout output)</div>
              )}
            </div>

            {runResult.stderr_tail && (
              <div className="text-amber-400/80 bg-amber-950/20 p-1.5 rounded text-[10px]">
                stderr: {runResult.stderr_tail}
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

export default DevPanel;
