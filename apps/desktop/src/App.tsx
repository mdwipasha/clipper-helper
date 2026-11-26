import { SupportedLocale, t } from '@clipper-helper/i18n';
import { useCallback, useEffect, useState } from 'react';
import { ComponentsView } from './components/ComponentsView';
import { DevPanel } from './components/DevPanel';
import { DiagnosticsView } from './components/DiagnosticsView';
import { ErrorBanner } from './components/ErrorBanner';
import { HomeView, NavTab } from './components/HomeView';
import { SettingsView } from './components/SettingsView';
import {
  installGlobalWebViewErrorHandlers,
  WebViewErrorBoundary,
} from './components/WebViewErrorBoundary';
import {
  AppInfo,
  commands,
  ComponentsOverview,
  CrashRecoveryStatus,
  SettingsOverview,
  WorkspaceStatus,
} from './ipc/bindings';
import { copyTextToClipboard, formatVersionBadge } from './utils/format';

export function App() {
  const [activeTab, setActiveTab] = useState<NavTab>('home');
  const [appInfo, setAppInfo] = useState<AppInfo | null>(null);
  const [wsStatus, setWsStatus] = useState<WorkspaceStatus | null>(null);
  const [overview, setOverview] = useState<ComponentsOverview | null>(null);
  const [settings, setSettings] = useState<SettingsOverview | null>(null);
  const [crashStatus, setCrashStatus] = useState<CrashRecoveryStatus | null>(null);
  const [ipcError, setIpcError] = useState<string | null>(null);

  const locale: SupportedLocale = settings?.language === 'en' ? 'en' : 'id';

  const refreshOverview = useCallback(async (overrideUrl?: string | null) => {
    const res = await commands.getComponentsOverview(overrideUrl ?? null);
    if (res.status === 'ok') {
      setOverview(res.data);
    }
  }, []);

  const loadInitialState = useCallback(async () => {
    try {
      const [info, ws, crash] = await Promise.all([
        commands.getAppInfo(),
        commands.getWorkspaceStatus(),
        commands.getCrashRecoveryStatus(),
      ]);
      setAppInfo(info);
      setWsStatus(ws);
      setCrashStatus(crash);

      if (ws.is_initialized) {
        const [ovRes, setRes] = await Promise.all([
          commands.getComponentsOverview(null),
          commands.getSettingsOverview(),
        ]);
        if (ovRes.status === 'ok') setOverview(ovRes.data);
        if (setRes.status === 'ok') setSettings(setRes.data);
        // Best-effort non-blocking manifest check on startup (falls back cleanly to neutral offline state)
        const manifestRes = await commands.fetchComponentsManifest(null);
        if (manifestRes.status === 'ok') {
          const refreshed = await commands.getComponentsOverview(null);
          if (refreshed.status === 'ok') setOverview(refreshed.data);
        }
      }
    } catch (err: unknown) {
      setIpcError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    const cleanup = installGlobalWebViewErrorHandlers();
    void loadInitialState();
    return cleanup;
  }, [loadInitialState]);

  const handleDismissCrash = async () => {
    const res = await commands.dismissCrashRecovery();
    if (res.status === 'ok') {
      setCrashStatus((prev) =>
        prev ? { ...prev, unclean_shutdown_detected: false, banner_dismissed: true } : prev
      );
    }
  };

  const handleQuickCopyCrashReport = async () => {
    const rep = await commands.generateDiagnosticsReport('', true);
    if (rep.status === 'ok') {
      await copyTextToClipboard(rep.data.preview_text).catch(() => undefined);
    }
  };

  const navItems: { id: NavTab; label: string }[] = [
    { id: 'home', label: t('nav.home', undefined, locale) },
    { id: 'components', label: t('nav.components', undefined, locale) },
    { id: 'diagnostics', label: t('nav.diagnostics', undefined, locale) },
    { id: 'settings', label: t('nav.settings', undefined, locale) },
  ];

  return (
    <div className="min-h-screen bg-slate-950 text-slate-100 flex flex-col">
      <header className="border-b border-slate-800 bg-slate-900/90 px-6 py-3">
        <div className="max-w-5xl mx-auto flex flex-wrap items-center justify-between gap-4">
          <div className="flex items-center gap-3">
            <div className="w-9 h-9 rounded-lg bg-indigo-600 flex items-center justify-center font-bold text-sm shadow-inner">
              CH
            </div>
            <div>
              <h1 className="text-base font-bold tracking-tight text-white leading-none">
                {t('app.name', undefined, locale)}
              </h1>
              <p className="text-[11px] text-slate-400 mt-0.5">
                {appInfo ? formatVersionBadge(appInfo.version) : '…'} ·{' '}
                {appInfo?.platform ?? 'windows'}
              </p>
            </div>
          </div>

          <nav aria-label="Main navigation" className="flex flex-wrap gap-1.5">
            {navItems.map((item) => {
              const active = activeTab === item.id;
              return (
                <button
                  key={item.id}
                  type="button"
                  onClick={() => setActiveTab(item.id)}
                  className={`px-3.5 py-1.5 rounded-lg text-xs font-medium transition ${
                    active
                      ? 'bg-indigo-600 text-white shadow-sm'
                      : 'text-slate-300 hover:bg-slate-800 hover:text-white'
                  }`}
                >
                  {item.label}
                </button>
              );
            })}
          </nav>
        </div>
      </header>

      <main className="flex-1 w-full max-w-5xl mx-auto p-6 space-y-4">
        {/* Top Crash Recovery Banner */}
        {crashStatus?.unclean_shutdown_detected && !crashStatus.banner_dismissed && (
          <div
            data-testid="crash-recovery-banner"
            role="alert"
            className="bg-amber-950/70 border border-amber-700/80 rounded-lg p-3.5 text-xs flex flex-wrap items-center justify-between gap-2 text-amber-100"
          >
            <span className="font-medium">{t('crash.bannerText', undefined, locale)}</span>
            <div className="flex flex-wrap items-center gap-2">
              <button
                type="button"
                onClick={() => void handleQuickCopyCrashReport()}
                className="bg-amber-600 hover:bg-amber-500 text-slate-950 font-semibold px-3 py-1 rounded transition"
              >
                {t('crash.copyReport', undefined, locale)}
              </button>
              <button
                type="button"
                onClick={() => setActiveTab('diagnostics')}
                className="underline text-amber-200 px-2 py-1"
              >
                {t('crash.openDiagnostics', undefined, locale)}
              </button>
              <button
                type="button"
                onClick={() => void handleDismissCrash()}
                className="border border-amber-700/70 text-amber-200 px-2.5 py-1 rounded transition"
              >
                {t('crash.dismiss', undefined, locale)}
              </button>
            </div>
          </div>
        )}

        {/* Shared ErrorBanner for Workspace Init or IPC failure */}
        {wsStatus && !wsStatus.is_initialized && (
          <ErrorBanner
            testId="workspace-error-banner"
            locale={locale}
            error={{
              code: wsStatus.error_code ?? 'E_APP_INIT_FAILED',
              message: wsStatus.error_message ?? t('err.E_APP_INIT_FAILED.body', undefined, locale),
              redactedDetail: wsStatus.error_message ?? undefined,
              actions: [
                {
                  label: t('components.retry', undefined, locale),
                  variant: 'primary',
                  onClick: () => void loadInitialState(),
                },
              ],
            }}
            onOpenDiagnostics={() => setActiveTab('diagnostics')}
          />
        )}

        {ipcError && (
          <ErrorBanner
            locale={locale}
            error={{
              code: 'E_UNKNOWN',
              message: ipcError,
              redactedDetail: ipcError,
              actions: [
                {
                  label: t('components.retry', undefined, locale),
                  onClick: () => {
                    setIpcError(null);
                    void loadInitialState();
                  },
                },
              ],
            }}
            onOpenDiagnostics={() => setActiveTab('diagnostics')}
          />
        )}

        <WebViewErrorBoundary locale={locale} onOpenDiagnostics={() => setActiveTab('diagnostics')}>
          {activeTab === 'home' && (
            <HomeView
              locale={locale}
              overview={overview}
              settings={settings}
              onNavigate={setActiveTab}
            />
          )}
          {activeTab === 'components' && (
            <ComponentsView
              locale={locale}
              overview={overview}
              onRefreshOverview={refreshOverview}
              onOpenDiagnostics={() => setActiveTab('diagnostics')}
            />
          )}
          {activeTab === 'diagnostics' && (
            <DiagnosticsView
              locale={locale}
              crashStatus={crashStatus}
              onDismissCrashBanner={handleDismissCrash}
              onNavigateToDiagnostics={() => setActiveTab('diagnostics')}
            />
          )}
          {activeTab === 'settings' && (
            <SettingsView
              locale={locale}
              settings={settings}
              onUpdated={setSettings}
              onOpenDiagnostics={() => setActiveTab('diagnostics')}
            />
          )}
        </WebViewErrorBoundary>

        {/* Debug Panel stays strictly debug-only behind import.meta.env.DEV */}
        {import.meta.env.DEV && (
          <details className="mt-6 bg-slate-900/60 border border-slate-800 rounded-xl p-4">
            <summary className="cursor-pointer text-xs font-mono text-slate-400">
              Developer Debug Panel (DEV only)
            </summary>
            <div className="mt-3">{import.meta.env.DEV && <DevPanel />}</div>
          </details>
        )}
      </main>
    </div>
  );
}
export default App;
