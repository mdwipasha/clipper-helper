import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import {
  CapabilityReportView,
  commands,
  ComponentDiagnosticsSnapshot,
  CustomFfmpegStatus,
  DevFixtureServerStatus,
  DevInstallOutcome,
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

function formatVersion(version: string | null | undefined): string {
  if (!version) return '';
  return /^\d/.test(version) ? `v${version}` : version;
}

function isModelComponent(name: string): boolean {
  return name.startsWith('ggml-');
}

function CapabilityDetails({ report }: { report: CapabilityReportView }) {
  const statusColor =
    report.status === 'pass'
      ? 'bg-emerald-950/60 border-emerald-800 text-emerald-300'
      : report.status === 'warn'
        ? 'bg-amber-950/60 border-amber-700 text-amber-300'
        : 'bg-rose-950/60 border-rose-800 text-rose-300';

  return (
    <div className="mt-1.5 bg-black/50 border border-slate-800 rounded p-2 text-[10px] space-y-1">
      <div className="flex items-center justify-between">
        <span className="text-slate-400">
          Capability Check ({report.mode})
          {report.reported_version ? ` · reported ${formatVersion(report.reported_version)}` : ''}
        </span>
        <span className={`px-1.5 py-0.5 rounded border font-bold uppercase ${statusColor}`}>
          {report.status}
        </span>
      </div>

      {report.ffmpeg && (
        <div className="text-slate-300 space-y-0.5 border-t border-slate-800/80 pt-1">
          <div className="flex flex-wrap gap-x-3 gap-y-0.5">
            <span>
              License: <strong className="text-cyan-300">{report.ffmpeg.effective_license}</strong>
            </span>
            <span>ffprobe: {report.ffmpeg.ffprobe_version}</span>
            <span>
              Encoders in build:{' '}
              {report.ffmpeg.hardware_encoders_available.length > 0
                ? report.ffmpeg.hardware_encoders_available.join(', ')
                : 'none'}
            </span>
          </div>
          {report.ffmpeg.forbidden_libs_present.length > 0 && (
            <div className="text-rose-400">
              Forbidden libs: {report.ffmpeg.forbidden_libs_present.join(', ')}
            </div>
          )}
          {report.ffmpeg.gpl_filters_present.length > 0 && (
            <div className="text-rose-400">
              GPL denylist filters ({report.ffmpeg.gpl_filters_present.length}):{' '}
              {report.ffmpeg.gpl_filters_present.slice(0, 8).join(', ')}
              {report.ffmpeg.gpl_filters_present.length > 8 ? '…' : ''}
            </div>
          )}
        </div>
      )}

      {report.reasons.length > 0 && (
        <div className="text-rose-300 space-y-0.5">
          {report.reasons.map((r, i) => (
            <div key={i}>• {r}</div>
          ))}
        </div>
      )}

      {report.warnings.length > 0 && (
        <div className="text-amber-300 space-y-0.5">
          {report.warnings.map((w, i) => (
            <div key={i}>⚠ {w}</div>
          ))}
        </div>
      )}
    </div>
  );
}

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
  const [manifestUrl, setManifestUrl] = useState<string>(
    'http://127.0.0.1:8080/real/manifest.json'
  );
  const [sigUrl, setSigUrl] = useState<string>('http://127.0.0.1:8080/real/manifest.json.sig');
  const [fetchingManifest, setFetchingManifest] = useState<boolean>(false);
  const [manifestInfo, setManifestInfo] = useState<DevManifestInfo | null>(null);
  const [manifestError, setManifestError] = useState<DevManifestError | null>(null);
  const [sequenceFloorMsg, setSequenceFloorMsg] = useState<string | null>(null);

  const [downloadProgressMap, setDownloadProgressMap] = useState<
    Record<string, DevDownloadProgressEvent>
  >({});
  const [installingMap, setInstallingMap] = useState<Record<string, boolean>>({});
  const [installingAll, setInstallingAll] = useState<boolean>(false);
  const [installOutcomes, setInstallOutcomes] = useState<Record<string, DevInstallOutcome>>({});
  const [installErrors, setInstallErrors] = useState<Record<string, string>>({});

  // Component Diagnostics & State
  const [diagnostics, setDiagnostics] = useState<ComponentDiagnosticsSnapshot | null>(null);
  const [actionMessages, setActionMessages] = useState<Record<string, string>>({});
  const [expandedCaps, setExpandedCaps] = useState<Record<string, boolean>>({});

  // Custom FFmpeg Override State
  const [customPathInput, setCustomPathInput] = useState<string>('');
  const [customBusy, setCustomBusy] = useState<boolean>(false);
  const [customError, setCustomError] = useState<string | null>(null);

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

  const refreshDiagnostics = async () => {
    try {
      const res = await commands.devGetComponentDiagnostics();
      if (res.status === 'ok') {
        setDiagnostics(res.data);
        if (res.data.custom_ffmpeg.configured_path) {
          setCustomPathInput(res.data.custom_ffmpeg.configured_path);
        }
      }
    } catch (err: unknown) {
      console.warn('Failed to refresh component diagnostics:', err);
    }
  };

  const refreshServerStatus = async () => {
    try {
      const status = await commands.devGetFixtureServerStatus();
      setServerStatus(status);
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
    setSequenceFloorMsg(null);
    try {
      const res = await commands.devFetchManifest(targetM, targetS);
      if (res.status === 'ok') {
        setManifestInfo(res.data);
        await refreshDiagnostics();
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

  const handleResetSequenceFloor = async () => {
    setManifestError(null);
    try {
      const res = await commands.devResetManifestSequenceFloor();
      if (res.status === 'ok') {
        setManifestInfo(null);
        setSequenceFloorMsg(
          `Manifest sequence floor reset to #${res.data}; seq 1 -> 2 -> 3 presets can replay.`
        );
      } else {
        setSequenceFloorMsg(`Reset failed: ${res.error}`);
      }
    } catch (err: unknown) {
      setSequenceFloorMsg(String(err));
    }
  };

  const selectManifestPreset = async (relPath: string) => {
    const base = serverStatus.url || 'http://127.0.0.1:8080';
    const nextM = `${base}/${relPath}`;
    const nextS = `${base}/${relPath}.sig`;
    setManifestUrl(nextM);
    setSigUrl(nextS);
    await handleFetchManifest(nextM, nextS);
  };

  const handleInstallComponent = async (component: string, version: string) => {
    setInstallingMap((prev) => ({ ...prev, [component]: true }));
    setInstallErrors((prev) => {
      const copy = { ...prev };
      delete copy[component];
      return copy;
    });

    try {
      const res = await commands.devInstallComponent(component, version);
      if (res.status === 'ok') {
        setInstallOutcomes((prev) => ({ ...prev, [component]: res.data }));
        if (res.data.ok) {
          setManifestInfo((prev) =>
            prev
              ? {
                  ...prev,
                  components: prev.components.map((item) =>
                    item.component === component && item.version === version
                      ? { ...item, is_downloaded: true }
                      : item
                  ),
                }
              : prev
          );
        }
      } else {
        setInstallErrors((prev) => ({
          ...prev,
          [component]: `[${res.error.code}] ${res.error.message}`,
        }));
      }
    } catch (err: unknown) {
      setInstallErrors((prev) => ({ ...prev, [component]: String(err) }));
    } finally {
      setInstallingMap((prev) => ({ ...prev, [component]: false }));
      await refreshDiagnostics();
      loadSysInfo();
    }
  };

  const handleInstallAll = async () => {
    if (!manifestInfo) return;
    setInstallingAll(true);
    try {
      for (const comp of manifestInfo.components) {
        if (!comp.is_downloaded) {
          await handleInstallComponent(comp.component, comp.version);
        }
      }
    } finally {
      setInstallingAll(false);
    }
  };

  const handleRollback = async (component: string, targetVersion: string | null) => {
    try {
      const res = await commands.devRollbackComponent(component, targetVersion);
      if (res.status === 'ok') {
        setActionMessages((prev) => ({
          ...prev,
          [component]: `Switched active version to ${formatVersion(res.data)}`,
        }));
      } else {
        setActionMessages((prev) => ({ ...prev, [component]: `Rollback failed: ${res.error}` }));
      }
    } catch (err: unknown) {
      setActionMessages((prev) => ({ ...prev, [component]: String(err) }));
    } finally {
      await refreshDiagnostics();
      if (manifestInfo) {
        await handleFetchManifest();
      }
    }
  };

  const handleGc = async (component: string) => {
    try {
      const res = await commands.devGcComponent(component);
      if (res.status === 'ok') {
        const g = res.data;
        setActionMessages((prev) => ({
          ...prev,
          [component]: `GC: kept [${g.kept_versions.join(', ')}], removed ${g.removed_versions.length}, deferred ${g.deferred_versions.length}, skipped-in-use ${g.skipped_in_use.length}`,
        }));
      } else {
        setActionMessages((prev) => ({ ...prev, [component]: `GC error: ${res.error}` }));
      }
    } catch (err: unknown) {
      setActionMessages((prev) => ({ ...prev, [component]: String(err) }));
    } finally {
      await refreshDiagnostics();
    }
  };

  const handleVerify = async (component: string) => {
    try {
      const res = await commands.devVerifyComponent(component);
      if (res.status === 'ok') {
        const r = res.data;
        setActionMessages((prev) => ({
          ...prev,
          [component]: r.intact
            ? `Verify OK: ${r.message}`
            : `[${r.error_code ?? 'E_VERIFY_FAILED'}] ${r.message}`,
        }));
      } else {
        setActionMessages((prev) => ({ ...prev, [component]: `Verify failed: ${res.error}` }));
      }
    } catch (err: unknown) {
      setActionMessages((prev) => ({ ...prev, [component]: String(err) }));
    } finally {
      await refreshDiagnostics();
    }
  };

  const handleRetryQuarantined = async (component: string, version: string) => {
    setInstallingMap((prev) => ({ ...prev, [component]: true }));
    try {
      const res = await commands.devRetryQuarantinedComponent(component, version);
      if (res.status === 'ok') {
        const r = res.data;
        setActionMessages((prev) => ({
          ...prev,
          [component]: r.intact
            ? `Retry passed (${formatVersion(version)}): ${r.message}`
            : `[${r.error_code ?? 'E_VERIFY_FAILED'}] ${r.message}`,
        }));
        if (r.intact && manifestInfo) {
          await handleFetchManifest();
        }
      } else {
        setActionMessages((prev) => ({
          ...prev,
          [component]: `[${res.error.code}] ${res.error.message}`,
        }));
      }
    } catch (err: unknown) {
      setActionMessages((prev) => ({ ...prev, [component]: String(err) }));
    } finally {
      setInstallingMap((prev) => ({ ...prev, [component]: false }));
      await refreshDiagnostics();
    }
  };

  const handleRepair = async (component: string, version: string | null) => {
    setInstallingMap((prev) => ({ ...prev, [component]: true }));
    try {
      const res = await commands.devRepairComponent(component, version);
      if (res.status === 'ok') {
        setActionMessages((prev) => ({
          ...prev,
          [component]: `Repair complete: ${formatVersion(res.data.version)} — ${res.data.message}`,
        }));
        if (manifestInfo) {
          await handleFetchManifest();
        }
      } else {
        setActionMessages((prev) => ({
          ...prev,
          [component]: `[${res.error.code}] ${res.error.message}`,
        }));
      }
    } catch (err: unknown) {
      setActionMessages((prev) => ({ ...prev, [component]: String(err) }));
    } finally {
      setInstallingMap((prev) => ({ ...prev, [component]: false }));
      await refreshDiagnostics();
    }
  };

  const handleSetCustomFfmpeg = async () => {
    setCustomBusy(true);
    setCustomError(null);
    try {
      const res = await commands.devSetCustomFfmpeg(customPathInput);
      if (res.status === 'ok') {
        updateCustomState(res.data);
      } else {
        setCustomError(`[${res.error.code}] ${res.error.message}`);
      }
    } catch (err: unknown) {
      setCustomError(String(err));
    } finally {
      setCustomBusy(false);
    }
  };

  const handleClearCustomFfmpeg = async () => {
    setCustomBusy(true);
    setCustomError(null);
    try {
      const res = await commands.devClearCustomFfmpeg();
      if (res.status === 'ok') {
        setCustomPathInput('');
        updateCustomState(res.data);
      } else {
        setCustomError(res.error);
      }
    } catch (err: unknown) {
      setCustomError(String(err));
    } finally {
      setCustomBusy(false);
    }
  };

  const updateCustomState = (nextCustom: CustomFfmpegStatus) => {
    setDiagnostics((prev) => (prev ? { ...prev, custom_ffmpeg: nextCustom } : prev));
    void refreshDiagnostics();
  };

  useEffect(() => {
    loadSysInfo();
    void refreshDiagnostics();

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

    (async () => {
      let currentStatus = await refreshServerStatus();
      if (autoStartServer && currentStatus?.state !== 'running') {
        setServerStarting(true);
        try {
          const startRes = await commands.devStartFixtureServer();
          if (startRes.status === 'ok') {
            currentStatus = startRes.data;
            setServerStatus(startRes.data);
          }
        } catch (e) {
          console.warn('Auto-start fixture server failed:', e);
        } finally {
          setServerStarting(false);
        }
      }

      if (currentStatus?.state === 'running') {
        const base = currentStatus.url || 'http://127.0.0.1:8080';
        const realM = `${base}/real/manifest.json`;
        const realS = `${base}/real/manifest.json.sig`;
        // Try real pinned manifest first; fall back to dummy manifest if fetch-pinned-components hasn't run.
        const res = await commands.devFetchManifest(realM, realS);
        if (res.status === 'ok') {
          setManifestUrl(realM);
          setSigUrl(realS);
          setManifestInfo(res.data);
          await refreshDiagnostics();
        } else {
          const dummyM = `${base}/manifest.json`;
          const dummyS = `${base}/manifest.json.sig`;
          setManifestUrl(dummyM);
          setSigUrl(dummyS);
          await handleFetchManifest(dummyM, dummyS);
        }
      }
    })();

    const unlistenPromise = listen<DevDownloadProgressEvent>('download-progress', (event) => {
      setDownloadProgressMap((prev) => ({
        ...prev,
        [event.payload.component]: event.payload,
      }));
    });

    return () => {
      unlistenPromise.then((fn) => fn());
    };
  }, []);

  const handleFetchTamperedManifest = async () => {
    const base = serverStatus.url || 'http://127.0.0.1:8080';
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
        setInstallOutcomes({});
        setInstallErrors({});
        setActionMessages({});
        setCustomPathInput('');
        setManifestInfo(null);
        loadSysInfo();
        await refreshDiagnostics();
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
          [DEV] Debug Panel · M1.1c
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
                className="text-slate-300 truncate max-w-[360px]"
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
                type="button"
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

      {/* Manifest & Installer Section */}
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
                type="button"
                onClick={handleStopFixtureServer}
                className="text-[10px] bg-slate-800 hover:bg-slate-700 text-slate-300 border border-slate-600 px-2 py-0.5 rounded cursor-pointer"
              >
                Stop Server
              </button>
            ) : (
              <button
                type="button"
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

        {/* Quick Manifest Presets */}
        <div className="flex flex-wrap gap-1.5 items-center text-[10px]">
          <span className="text-slate-500">Presets:</span>
          <button
            type="button"
            onClick={() => selectManifestPreset('real/manifest.json')}
            className="bg-slate-900 hover:bg-slate-800 border border-slate-700 text-cyan-300 px-2 py-0.5 rounded cursor-pointer"
          >
            Real Pinned (seq 1)
          </button>
          <button
            type="button"
            onClick={() => selectManifestPreset('real/manifest-gpl-ffmpeg.json')}
            className="bg-rose-950/40 hover:bg-rose-900/50 border border-rose-800/60 text-rose-300 px-2 py-0.5 rounded cursor-pointer"
          >
            GPL FFmpeg S1 (seq 2)
          </button>
          <button
            type="button"
            onClick={() => selectManifestPreset('real/manifest-v2.json')}
            className="bg-indigo-950/40 hover:bg-indigo-900/50 border border-indigo-800/60 text-indigo-300 px-2 py-0.5 rounded cursor-pointer"
          >
            v2 Upgrade (seq 3)
          </button>
          <button
            type="button"
            onClick={() => selectManifestPreset('manifest.json')}
            className="bg-slate-900 hover:bg-slate-800 border border-slate-700 text-slate-400 px-2 py-0.5 rounded cursor-pointer"
          >
            Dummy Tiny
          </button>
        </div>

        {/* URL Inputs */}
        <div className="space-y-1.5">
          <div className="flex items-center space-x-2">
            <span className="text-slate-500 w-16 text-[11px]">Manifest:</span>
            <input
              type="text"
              aria-label="Manifest URL"
              value={manifestUrl}
              onChange={(e) => setManifestUrl(e.target.value)}
              className="flex-1 bg-slate-900 border border-slate-700 rounded px-2 py-0.5 text-slate-200 text-[11px] focus:outline-none focus:border-indigo-500"
            />
          </div>
          <div className="flex items-center space-x-2">
            <span className="text-slate-500 w-16 text-[11px]">Signature:</span>
            <input
              type="text"
              aria-label="Signature URL"
              value={sigUrl}
              onChange={(e) => setSigUrl(e.target.value)}
              className="flex-1 bg-slate-900 border border-slate-700 rounded px-2 py-0.5 text-slate-200 text-[11px] focus:outline-none focus:border-indigo-500"
            />
          </div>
        </div>

        {/* Fetch Controls */}
        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            onClick={() => handleFetchManifest()}
            disabled={fetchingManifest}
            className="flex-1 bg-indigo-600 hover:bg-indigo-500 disabled:opacity-50 text-white font-medium py-1 px-3 rounded transition text-xs cursor-pointer"
          >
            {fetchingManifest ? 'Fetching & Verifying...' : 'Fetch Manifest'}
          </button>
          <button
            type="button"
            onClick={handleFetchTamperedManifest}
            disabled={fetchingManifest}
            className="bg-rose-950/50 hover:bg-rose-900/60 text-rose-300 border border-rose-800/60 font-medium py-1 px-2.5 rounded transition text-xs cursor-pointer"
            title="Fetches a manifest with corrupted bytes / invalid signature to verify cryptographic rejection"
          >
            Test Tampered Manifest
          </button>
          <button
            type="button"
            onClick={handleResetSequenceFloor}
            disabled={fetchingManifest}
            className="bg-amber-950/50 hover:bg-amber-900/60 text-amber-300 border border-amber-800/60 font-medium py-1 px-2.5 rounded transition text-xs cursor-pointer"
            title="Debug-only: resets local manifest sequence floor so seq 1 -> 2 -> 3 presets can replay without wiping the workspace"
          >
            Reset manifest sequence floor
          </button>
        </div>

        {sequenceFloorMsg && (
          <div className="bg-amber-950/20 border border-amber-800/50 text-amber-200 p-2 rounded text-[10px]">
            {sequenceFloorMsg}
          </div>
        )}

        {/* Manifest Error Banner */}
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
                <span className="bg-rose-900/60 border border-rose-700 text-rose-200 text-[10px] px-1.5 py-0.5 rounded uppercase font-bold">
                  {manifestError.code}
                </span>
                <span className="text-rose-300 font-bold">Manifest Fetch / Verify Error</span>
              </div>
              <button
                type="button"
                onClick={() => handleFetchManifest()}
                disabled={fetchingManifest}
                className="bg-amber-800/50 hover:bg-amber-700/60 text-amber-200 border border-amber-700/60 px-2 py-0.5 rounded text-[10px] cursor-pointer"
              >
                Retry
              </button>
            </div>
            <div className="text-slate-300 break-all">{manifestError.message}</div>
          </div>
        )}

        {/* Verified Manifest Components */}
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

            <div className="flex items-center justify-between">
              <div className="text-slate-400 font-semibold text-[11px]">
                Manifest Components ({manifestInfo.components.length})
              </div>
              <button
                type="button"
                onClick={handleInstallAll}
                disabled={installingAll}
                className="bg-emerald-700 hover:bg-emerald-600 disabled:opacity-50 text-white text-[10px] font-bold px-2.5 py-1 rounded cursor-pointer transition"
              >
                {installingAll ? 'Installing All...' : 'Install All'}
              </button>
            </div>

            <div className="space-y-2">
              {manifestInfo.components.map((comp) => {
                const isInstalling = !!installingMap[comp.component];
                const progress = downloadProgressMap[comp.component];
                const outcome = installOutcomes[comp.component];
                const error = installErrors[comp.component];
                const isModel = isModelComponent(comp.component);

                return (
                  <div
                    key={`${comp.component}-${comp.version}`}
                    className="bg-slate-900/90 border border-slate-800 rounded p-2.5 space-y-2"
                  >
                    <div className="flex items-center justify-between">
                      <div className="space-y-0.5">
                        <div className="text-slate-200 font-bold flex items-center space-x-2">
                          <span>{comp.component}</span>
                          <span className="text-[10px] text-slate-400 font-normal">
                            {formatVersion(comp.version)}
                          </span>
                          <span className="text-[10px] text-slate-500 font-normal">
                            [{isModel ? 'model · hash check' : comp.archive}
                            {comp.files_count > 0 ? ` · ${comp.files_count} files` : ''}]
                          </span>
                        </div>
                        <div className="text-[10px] text-slate-500">
                          Size: {formatBytes(comp.size_bytes)} | SHA-256:{' '}
                          <span title={comp.sha256}>{comp.sha256.substring(0, 12)}…</span>
                        </div>
                      </div>

                      <div>
                        {comp.is_downloaded ? (
                          <span className="bg-emerald-950/60 border border-emerald-800/80 text-emerald-400 text-[10px] font-bold px-2 py-1 rounded">
                            {isModel ? 'Hash Verified & Current' : 'Installed & Current'}
                          </span>
                        ) : (
                          <button
                            type="button"
                            onClick={() => handleInstallComponent(comp.component, comp.version)}
                            disabled={isInstalling || installingAll}
                            className="bg-indigo-600 hover:bg-indigo-500 disabled:opacity-50 text-white text-[11px] font-medium px-3 py-1 rounded cursor-pointer transition"
                          >
                            {isInstalling ? 'Installing...' : 'Install'}
                          </button>
                        )}
                      </div>
                    </div>

                    {isInstalling && progress && (
                      <div className="space-y-1 bg-black/40 p-2 rounded border border-slate-800 text-[10px]">
                        <div className="flex justify-between text-slate-300">
                          <span>
                            [{progress.stage}] {formatBytes(progress.completed_bytes)} /{' '}
                            {formatBytes(progress.total_bytes)} ({progress.percent.toFixed(1)}%)
                          </span>
                          <span>
                            {progress.rate_bps > 0
                              ? `${formatBytes(progress.rate_bps)}/s`
                              : 'preparing…'}{' '}
                            | ETA:{' '}
                            {progress.eta_seconds !== null && progress.eta_seconds !== undefined
                              ? `${progress.eta_seconds.toFixed(1)}s`
                              : '…'}
                          </span>
                        </div>
                        <div className="w-full bg-slate-800 rounded-full h-1.5 overflow-hidden">
                          <div
                            className="bg-indigo-500 h-1.5 rounded-full transition-all duration-150"
                            style={{ width: `${Math.min(100, Math.max(0, progress.percent))}%` }}
                          />
                        </div>
                      </div>
                    )}

                    {outcome && outcome.component === comp.component && (
                      <div
                        className={
                          outcome.ok
                            ? 'bg-emerald-950/20 border border-emerald-800/50 p-2 rounded text-[10px] text-emerald-300'
                            : 'bg-rose-950/30 border border-rose-800/70 p-2 rounded text-[10px] text-rose-300 space-y-1'
                        }
                      >
                        <div className="font-bold flex items-center justify-between">
                          <span>{outcome.message}</span>
                          {outcome.quarantined && (
                            <span className="bg-rose-900/70 border border-rose-700 text-rose-100 px-1.5 py-0.5 rounded uppercase">
                              QUARANTINED
                            </span>
                          )}
                        </div>
                        {outcome.rolled_back_to && (
                          <div className="text-amber-300">
                            Rolled back automatically to {formatVersion(outcome.rolled_back_to)}.
                          </div>
                        )}
                        {outcome.capability_report && (
                          <CapabilityDetails report={outcome.capability_report} />
                        )}
                      </div>
                    )}

                    {error && (
                      <div className="text-rose-400 bg-rose-950/20 border border-rose-900/40 p-1.5 rounded text-[10px]">
                        Install error: {error}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          </div>
        )}
      </div>

      {/* Per-Component Runtime State, Rollback, GC, Verify/Repair & Capabilities */}
      <div
        data-testid="installed-state-section"
        className="bg-slate-950/80 rounded p-3 border border-slate-800 space-y-3"
      >
        <div className="flex items-center justify-between border-b border-slate-800/80 pb-2">
          <span className="text-slate-400 font-semibold">
            Installed Components State & Capability Reports
          </span>
          <div className="flex items-center space-x-2 text-[10px]">
            {diagnostics && diagnostics.pending_deferred_deletes > 0 && (
              <span className="bg-amber-950/60 border border-amber-700 text-amber-300 px-1.5 py-0.5 rounded">
                Deferred deletes: {diagnostics.pending_deferred_deletes}
              </span>
            )}
            <button
              type="button"
              onClick={refreshDiagnostics}
              className="bg-slate-800 hover:bg-slate-700 text-slate-300 border border-slate-600 px-2 py-0.5 rounded cursor-pointer"
            >
              Refresh State
            </button>
          </div>
        </div>

        {!diagnostics || diagnostics.components.length === 0 ? (
          <div className="text-slate-500 text-[11px] italic">
            No components installed in SQLite yet. Fetch a manifest and install components above.
          </div>
        ) : (
          <div className="space-y-2.5">
            {diagnostics.components.map((state) => (
              <div
                key={state.component}
                className="bg-slate-900/90 border border-slate-800 rounded p-2.5 space-y-2"
              >
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div>
                    <div className="flex items-center space-x-2">
                      <span className="text-slate-100 font-bold">{state.component}</span>
                      <span className="text-[10px] text-slate-400 border border-slate-700 px-1.5 py-0.5 rounded">
                        {state.kind}
                      </span>
                      <span
                        className={`text-[10px] px-1.5 py-0.5 rounded border font-bold ${
                          state.active_source === 'managed'
                            ? 'bg-emerald-950/60 border-emerald-800 text-emerald-300'
                            : state.active_source === 'custom'
                              ? 'bg-amber-950/60 border-amber-700 text-amber-300'
                              : state.active_source === 'fallback'
                                ? 'bg-amber-950/60 border-amber-700 text-amber-200'
                                : 'bg-rose-950/60 border-rose-800 text-rose-300'
                        }`}
                      >
                        {state.active_source.toUpperCase()}
                      </span>
                    </div>
                    <div className="text-[10px] text-slate-500 mt-0.5">
                      current:{' '}
                      <strong className="text-cyan-300">{state.current_version ?? 'none'}</strong> |
                      previous:{' '}
                      <strong className="text-indigo-300">
                        {state.previous_version ?? 'none'}
                      </strong>
                      {state.active_path ? ` | ${state.active_path}` : ''}
                    </div>
                  </div>

                  {/* Lifecycle Buttons */}
                  <div className="flex flex-wrap gap-1.5">
                    {state.kind === 'bin' && (
                      <>
                        <button
                          type="button"
                          onClick={() => handleRollback(state.component, null)}
                          disabled={!state.previous_version}
                          className="bg-amber-900/40 hover:bg-amber-800/60 disabled:opacity-40 text-amber-200 border border-amber-700/60 text-[10px] px-2 py-0.5 rounded cursor-pointer"
                        >
                          Rollback
                        </button>
                        <button
                          type="button"
                          onClick={() => handleGc(state.component)}
                          className="bg-slate-800 hover:bg-slate-700 text-slate-200 border border-slate-600 text-[10px] px-2 py-0.5 rounded cursor-pointer"
                        >
                          GC
                        </button>
                      </>
                    )}
                    <button
                      type="button"
                      onClick={() => handleVerify(state.component)}
                      className="bg-cyan-900/40 hover:bg-cyan-800/60 text-cyan-200 border border-cyan-700/60 text-[10px] px-2 py-0.5 rounded cursor-pointer"
                    >
                      Verify
                    </button>
                    <button
                      type="button"
                      onClick={() => handleRepair(state.component, state.current_version)}
                      disabled={!!installingMap[state.component]}
                      className="bg-indigo-700 hover:bg-indigo-600 disabled:opacity-50 text-white text-[10px] px-2 py-0.5 rounded cursor-pointer"
                    >
                      Repair
                    </button>
                  </div>
                </div>

                {state.missing_advice && (
                  <div className="bg-rose-950/40 border border-rose-800 text-rose-200 p-2 rounded text-[10px] space-y-1">
                    <div className="font-bold">[{state.missing_advice.code}] Missing / Broken</div>
                    <div>{state.missing_advice.message}</div>
                  </div>
                )}

                {actionMessages[state.component] && (
                  <div className="bg-slate-950 border border-slate-700 text-cyan-200 p-1.5 rounded text-[10px]">
                    {actionMessages[state.component]}
                  </div>
                )}

                {/* Per-version Table */}
                <div className="space-y-1.5 border-t border-slate-800/80 pt-1.5">
                  {state.versions.map((ver) => {
                    const key = `${state.component}:${ver.version}`;
                    const showCap = expandedCaps[key] ?? ver.is_current;
                    return (
                      <div
                        key={ver.version}
                        className="bg-black/30 border border-slate-800/80 rounded p-2 space-y-1"
                      >
                        <div className="flex flex-wrap items-center justify-between gap-2 text-[10px]">
                          <div className="flex items-center space-x-2">
                            <span className="text-slate-200 font-semibold">
                              {formatVersion(ver.version)}
                            </span>
                            <span className="text-slate-500">seq #{ver.manifest_sequence}</span>
                            {ver.is_current && (
                              <span className="bg-emerald-950/80 border border-emerald-700 text-emerald-300 px-1.5 py-0.2 rounded font-bold">
                                CURRENT
                              </span>
                            )}
                            {ver.is_previous && (
                              <span className="bg-indigo-950/80 border border-indigo-700 text-indigo-300 px-1.5 py-0.2 rounded font-bold">
                                PREVIOUS
                              </span>
                            )}
                            {state.kind === 'model' && ver.status === 'installed' && (
                              <span className="bg-cyan-950/70 border border-cyan-800 text-cyan-300 px-1.5 py-0.2 rounded font-semibold">
                                HASH VERIFIED
                              </span>
                            )}
                            {ver.status === 'quarantined' ? (
                              <span className="bg-rose-950/80 border border-rose-700 text-rose-300 px-1.5 py-0.2 rounded font-bold">
                                QUARANTINED
                              </span>
                            ) : (
                              !ver.is_current &&
                              !ver.is_previous && (
                                <span className="bg-slate-800 border border-slate-700 text-slate-300 px-1.5 py-0.2 rounded">
                                  INSTALLED
                                </span>
                              )
                            )}
                          </div>

                          <div className="flex items-center space-x-1.5">
                            {ver.status === 'installed' &&
                              !ver.is_current &&
                              state.kind === 'bin' && (
                                <button
                                  type="button"
                                  onClick={() => handleRollback(state.component, ver.version)}
                                  className="bg-emerald-900/40 hover:bg-emerald-800/60 text-emerald-200 border border-emerald-700/60 px-2 py-0.5 rounded cursor-pointer"
                                >
                                  Activate
                                </button>
                              )}
                            {ver.status === 'quarantined' && state.kind === 'bin' && (
                              <>
                                <button
                                  type="button"
                                  onClick={() =>
                                    handleRetryQuarantined(state.component, ver.version)
                                  }
                                  disabled={!!installingMap[state.component]}
                                  className="bg-amber-900/50 hover:bg-amber-800/70 disabled:opacity-50 text-amber-200 border border-amber-700/70 px-2 py-0.5 rounded cursor-pointer"
                                  title="Re-runs strict capability verification on this quarantined version"
                                >
                                  Retry
                                </button>
                                <button
                                  type="button"
                                  onClick={() => handleRepair(state.component, ver.version)}
                                  disabled={!!installingMap[state.component]}
                                  className="bg-indigo-800/60 hover:bg-indigo-700/70 disabled:opacity-50 text-indigo-100 border border-indigo-600/70 px-2 py-0.5 rounded cursor-pointer"
                                  title="Re-downloads and re-verifies this quarantined version from active manifest"
                                >
                                  Repair
                                </button>
                              </>
                            )}
                            {ver.capability_report && (
                              <button
                                type="button"
                                onClick={() =>
                                  setExpandedCaps((prev) => ({ ...prev, [key]: !showCap }))
                                }
                                className="bg-slate-800 hover:bg-slate-700 text-slate-300 border border-slate-600 px-2 py-0.5 rounded cursor-pointer"
                              >
                                {showCap ? 'Hide Report' : 'Capability Report'}
                              </button>
                            )}
                          </div>
                        </div>

                        {ver.quarantine_reason && (
                          <div className="text-[10px] text-rose-300">
                            Quarantine reason: {ver.quarantine_reason}
                          </div>
                        )}

                        {ver.capability_report && showCap && (
                          <CapabilityDetails report={ver.capability_report} />
                        )}
                      </div>
                    );
                  })}
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      {/* Custom FFmpeg Override (LGPLv3 §6) */}
      <div
        data-testid="custom-ffmpeg-section"
        className="bg-slate-950/80 rounded p-3 border border-slate-800 space-y-2.5"
      >
        <div className="flex items-center justify-between">
          <span className="text-slate-400 font-semibold">
            Custom FFmpeg (LGPLv3 User Replacement)
          </span>
          {diagnostics?.custom_ffmpeg.warning_flag && (
            <span className="bg-amber-950/70 border border-amber-700 text-amber-300 text-[10px] px-2 py-0.5 rounded font-bold">
              CUSTOM ACTIVE · WARNING
            </span>
          )}
        </div>

        <div className="flex space-x-2">
          <input
            type="text"
            aria-label="Custom FFmpeg path"
            placeholder="C:\path\to\ffmpeg\bin\ffmpeg.exe (fixed drive, with sibling ffprobe.exe)"
            value={customPathInput}
            onChange={(e) => setCustomPathInput(e.target.value)}
            disabled={customBusy}
            className="flex-1 bg-slate-900 border border-slate-700 rounded px-2 py-1 text-slate-200 text-[11px] focus:outline-none focus:border-amber-500"
          />
          <button
            type="button"
            onClick={handleSetCustomFfmpeg}
            disabled={customBusy}
            className="bg-amber-700 hover:bg-amber-600 disabled:opacity-50 text-white text-[11px] font-medium px-3 py-1 rounded cursor-pointer"
          >
            {customBusy ? 'Checking...' : 'Apply'}
          </button>
          <button
            type="button"
            onClick={handleClearCustomFfmpeg}
            disabled={customBusy || !diagnostics?.custom_ffmpeg.enabled}
            className="bg-slate-800 hover:bg-slate-700 disabled:opacity-40 text-slate-300 border border-slate-600 text-[11px] px-2.5 py-1 rounded cursor-pointer"
          >
            Clear
          </button>
        </div>

        {customError && (
          <div className="text-rose-300 bg-rose-950/30 border border-rose-800/60 p-2 rounded text-[10px]">
            {customError}
          </div>
        )}

        {diagnostics?.custom_ffmpeg.enabled && (
          <div
            data-testid="custom-ffmpeg-warning-banner"
            className="bg-amber-950/30 border border-amber-700/70 p-2.5 rounded text-[10px] text-amber-200 space-y-1"
          >
            <div className="font-bold flex items-center justify-between">
              <span>Persistent Custom FFmpeg Warning (applies to FFmpeg/ffprobe only)</span>
              {diagnostics.custom_ffmpeg.fallback_active && (
                <span className="bg-rose-900/70 border border-rose-700 text-rose-100 px-1.5 py-0.5 rounded uppercase">
                  MISSING ON DISK · FALLBACK TO MANAGED
                </span>
              )}
            </div>
            <div>Path: {diagnostics.custom_ffmpeg.redacted_path}</div>
            {diagnostics.custom_ffmpeg.warnings.map((w, idx) => (
              <div key={idx}>⚠ {w}</div>
            ))}
            {diagnostics.custom_ffmpeg.capability_report && (
              <CapabilityDetails report={diagnostics.custom_ffmpeg.capability_report} />
            )}
          </div>
        )}
      </div>

      {/* Fake Sidecar Runner */}
      <div className="bg-slate-950/80 rounded p-3 border border-slate-800 space-y-3">
        <div className="text-slate-400 font-semibold">Sidecar Supervisor Test</div>

        <div className="flex space-x-2">
          <select
            aria-label="Fake sidecar mode"
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
            type="button"
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
