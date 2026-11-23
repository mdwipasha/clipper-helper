import { t } from '@clipper-helper/i18n';
import { useEffect, useState } from 'react';
import { AppInfo, commands } from './ipc/bindings';

export function App() {
  const [appInfo, setAppInfo] = useState<AppInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    commands
      .getAppInfo()
      .then((info) => setAppInfo(info))
      .catch((err: unknown) => {
        console.error('Failed to get app info:', err);
        setError(String(err));
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

        <div className="mt-6 text-center text-xs text-slate-500">
          M1.0 Desktop Shell · Milestone 1
        </div>
      </div>
    </main>
  );
}

export default App;
