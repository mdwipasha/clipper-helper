import { useEffect, useState } from 'react';
import { commands, DevSidecarRunResult, DevSystemInfo } from '../ipc/bindings';

export function DevPanel() {
  const [sysInfo, setSysInfo] = useState<DevSystemInfo | null>(null);
  const [sysError, setSysError] = useState<string | null>(null);

  const [mode, setMode] = useState<string>('progress');
  const [running, setRunning] = useState<boolean>(false);
  const [runResult, setRunResult] = useState<DevSidecarRunResult | null>(null);
  const [runError, setRunError] = useState<string | null>(null);

  useEffect(() => {
    commands
      .devGetSystemInfo()
      .then((res) => {
        if (res.status === 'ok') {
          setSysInfo(res.data);
        } else {
          setSysError(res.error);
        }
      })
      .catch((err: unknown) => {
        setSysError(String(err));
      });
  }, []);

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
      <div className="bg-slate-950/80 rounded p-3 border border-slate-800 space-y-1">
        <div className="text-slate-400 font-semibold mb-1">SQLite & Workspace Status</div>
        {sysInfo ? (
          <>
            <div className="flex justify-between">
              <span className="text-slate-500">Schema Version:</span>
              <span className="text-cyan-400 font-bold">v{sysInfo.schema_version}</span>
            </div>
            <div className="flex justify-between">
              <span className="text-slate-500">Workspace Root:</span>
              <span
                className="text-slate-300 truncate max-w-[200px]"
                title={sysInfo.workspace_root}
              >
                {sysInfo.workspace_root}
              </span>
            </div>
            <div className="flex justify-between">
              <span className="text-slate-500">Architecture:</span>
              <span className="text-indigo-300">
                {sysInfo.platform}-{sysInfo.arch}
              </span>
            </div>
          </>
        ) : sysError ? (
          <div className="text-red-400">Error: {sysError}</div>
        ) : (
          <div className="text-slate-500 animate-pulse">Loading system info...</div>
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
