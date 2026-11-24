import { t } from '@clipper-helper/i18n';
import { useEffect, useState } from 'react';
import { DevPanel } from './components/DevPanel';
import { AppInfo, commands, WorkspaceStatus } from './ipc/bindings';

export function App() {
  const [appInfo, setAppInfo] = useState<AppInfo | null>(null);
  const [wsStatus, setWsStatus] = useState<WorkspaceStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    commands
      .getAppInfo()
      .then((info) => setAppInfo(info))
      .catch((err: unknown) => {
        console.error('Failed to get app info:', err);
        setError(String(err));
      });

    commands
      .getWorkspaceStatus()
      .then((status) => setWsStatus(status))
      .catch((err: unknown) => {
        console.error('Failed to get workspace status:', err);
      });
  }, []);

  return (
    <main className="min-h-screen bg-slate-900 text-slate-100 flex flex-col items-center justify-center p-8">
      <div className="max-w-md w-full bg-slate-800/80 border border-slate-700/60 rounded-xl p-6 shadow-2xl backdrop-blur-sm">
        <div className="flex items-center space-x-3 mb-4">
          <div className="w-10 h-10 rounded-lg bg-indigo-600 flex items-center justify-center font-bold text-lg shadow-inner">
            CH
          </div>
          <div>
            <h1 className="text-xl font-bold tracking-tight text-white">{t('app.name')}</h1>
            <p className="text-xs text-slate-400">{t('app.welcome')}</p>
          </div>
        </div>

        <p className="text-sm text-slate-300 mb-6 leading-relaxed">{t('app.tagline')}</p>

        {/* Actionable Error Banner if Workspace Initialization Failed */}
        {wsStatus && !wsStatus.is_initialized && (
          <div
            data-testid="workspace-error-banner"
            className="mb-4 bg-rose-950/60 border border-rose-800/80 rounded-lg p-4 font-mono text-xs space-y-2 text-rose-200"
          >
            <div className="font-bold text-rose-400 flex items-center justify-between">
              <span>Gagal Menginisialisasi Ruang Kerja</span>
              <span className="text-[10px] bg-rose-900/60 px-1.5 py-0.5 rounded border border-rose-700/60 font-semibold">
                {wsStatus.error_code ?? 'E_APP_INIT_FAILED'}
              </span>
            </div>
            <p className="text-[11px] text-rose-300/90 leading-relaxed">
              {wsStatus.error_message ??
                'Ruang kerja lokal (%LOCALAPPDATA%\\ClipperHelper) tidak dapat dibuat atau diakses.'}
            </p>
            <p className="text-[10px] text-slate-400">
              Pastikan disk Anda memiliki ruang kosong yang cukup dan aplikasi memiliki izin tulis
              ke %LOCALAPPDATA%.
            </p>
            <button
              onClick={() => {
                commands.getWorkspaceStatus().then(setWsStatus);
              }}
              className="mt-1 bg-rose-800 hover:bg-rose-700 text-white font-medium px-3 py-1 rounded text-xs transition cursor-pointer"
            >
              Coba Lagi
            </button>
          </div>
        )}

        <div className="bg-slate-950/60 rounded-lg p-4 border border-slate-800 font-mono text-xs space-y-2">
          <div className="text-slate-400 uppercase tracking-wider text-[10px] font-semibold border-b border-slate-800 pb-1">
            Rust IPC Status (get_app_info)
          </div>
          {appInfo ? (
            <div className="space-y-1">
              <div className="flex justify-between">
                <span className="text-slate-500">App Name:</span>
                <span className="text-emerald-400 font-medium">{appInfo.name}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-slate-500">Version:</span>
                <span className="text-indigo-400 font-medium">{appInfo.version}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-slate-500">Platform:</span>
                <span className="text-amber-400 font-medium">{appInfo.platform}</span>
              </div>
            </div>
          ) : error ? (
            <div className="text-red-400">Error: {error}</div>
          ) : (
            <div className="text-slate-500 animate-pulse">Menghubungkan ke core Rust...</div>
          )}
        </div>

        {/* Debug Dev Panel only in development mode */}
        {import.meta.env.DEV && <DevPanel />}

        <div className="mt-6 text-center text-xs text-slate-500">
          M1.1a Foundations · Milestone 1
        </div>
      </div>
    </main>
  );
}

export default App;
