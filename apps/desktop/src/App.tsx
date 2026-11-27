import { SupportedLocale, t } from '@clipper-helper/i18n';
import { useCallback, useEffect, useState } from 'react';
import { ComponentsView } from './components/ComponentsView';
import { DevPanel } from './components/DevPanel';
import { DiagnosticsView } from './components/DiagnosticsView';
import { ErrorBanner } from './components/ErrorBanner';
import { HomeView, NavTab } from './components/HomeView';
import { RangePickerView } from './components/RangePickerView';
import { SettingsView } from './components/SettingsView';
import {
  installGlobalWebViewErrorHandlers,
  WebViewErrorBoundary,
} from './components/WebViewErrorBoundary';
import {
  AppCommandError,
  AppInfo,
  commands,
  ComponentsOverview,
  CrashRecoveryStatus,
  events,
  JobEventPayload,
  JobRecord,
  PreviewConfigResponse,
  ProjectRecord,
  SettingsOverview,
  WorkspaceStatus,
} from './ipc/bindings';
import { formatEtaDisplay } from './utils/etaFormat';
import { copyTextToClipboard, formatComponentVersion } from './utils/format';

const DEFAULT_PREVIEW_CONFIG: PreviewConfigResponse = {
  bridge_url: 'https://clipper-bridge.vercel.app',
  bridge_protocol_version: 1,
  control_video_id: 'aqz-KE-bpKQ',
  ready_timeout_ms: 6000,
  max_range_seconds: 300,
  download_prior_rt_factor: 1.66,
  download_prior_startup_seconds: 10,
  can_force_timecode_only: false,
};

export function App() {
  const [activeTab, setActiveTab] = useState<NavTab>('home');
  const [selectedProject, setSelectedProject] = useState<ProjectRecord | null>(null);
  const [previewConfig, setPreviewConfig] = useState<PreviewConfigResponse>(DEFAULT_PREVIEW_CONFIG);
  const [appInfo, setAppInfo] = useState<AppInfo | null>(null);
  const [wsStatus, setWsStatus] = useState<WorkspaceStatus | null>(null);
  const [overview, setOverview] = useState<ComponentsOverview | null>(null);
  const [settings, setSettings] = useState<SettingsOverview | null>(null);
  const [crashStatus, setCrashStatus] = useState<CrashRecoveryStatus | null>(null);
  const [ipcError, setIpcError] = useState<string | null>(null);
  const [jobs, setJobs] = useState<JobRecord[]>([]);
  const [jobEvents, setJobEvents] = useState<Record<string, JobEventPayload>>({});
  const [jobActionError, setJobActionError] = useState<AppCommandError | null>(null);

  const locale: SupportedLocale = settings?.language === 'en' ? 'en' : 'id';

  const refreshOverview = useCallback(async (overrideUrl?: string | null) => {
    const res = await commands.getComponentsOverview(overrideUrl ?? null);
    if (res.status === 'ok') {
      setOverview(res.data);
    }
  }, []);

  const refreshJobs = useCallback(async () => {
    try {
      const res = await commands.listJobs(null, 20);
      if (res.status === 'ok') {
        setJobs(res.data);
      }
    } catch {
      // Non-fatal during startup or test harnesses lacking job commands
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
        const [ovRes, setRes, prevCfg] = await Promise.all([
          commands.getComponentsOverview(null),
          commands.getSettingsOverview(),
          commands.getPreviewConfig().catch(() => DEFAULT_PREVIEW_CONFIG),
        ]);
        if (ovRes.status === 'ok') setOverview(ovRes.data);
        if (setRes.status === 'ok') setSettings(setRes.data);
        if (prevCfg) setPreviewConfig(prevCfg);
        await refreshJobs();
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
  }, [refreshJobs]);

  useEffect(() => {
    const cleanup = installGlobalWebViewErrorHandlers();
    void loadInitialState();
    return cleanup;
  }, [loadInitialState]);

  useEffect(() => {
    let disposed = false;
    let unlistenFn: (() => void) | null = null;

    if (typeof events?.jobEventPayload?.listen === 'function') {
      events.jobEventPayload
        .listen((ev) => {
          if (disposed) return;
          const payload = ev.payload;
          setJobEvents((prev) => ({ ...prev, [payload.jobId]: payload }));
          const isTerminal =
            payload.state === 'succeeded' ||
            payload.state === 'failed' ||
            payload.state === 'cancelled' ||
            payload.state === 'interrupted';
          if (isTerminal) {
            void refreshJobs();
            if (payload.state === 'succeeded' && selectedProject?.id === payload.projectId) {
              void commands.getProject(payload.projectId).then((res) => {
                if (!disposed && res.status === 'ok' && res.data) {
                  setSelectedProject(res.data);
                }
              });
            }
          }
        })
        .then((fn) => {
          if (disposed) fn();
          else unlistenFn = fn;
        })
        .catch(() => undefined);
    }

    return () => {
      disposed = true;
      if (unlistenFn) unlistenFn();
    };
  }, [refreshJobs, selectedProject?.id]);

  const handleStartGenerate = useCallback(
    async (projectId: string): Promise<JobRecord | null> => {
      setJobActionError(null);
      const res = await commands.startGenerateJob(projectId);
      if (res.status === 'ok') {
        await refreshJobs();
        return res.data;
      }
      setJobActionError(res.error);
      return null;
    },
    [refreshJobs]
  );

  const handleCancelJob = useCallback(
    async (jobId: string): Promise<void> => {
      const res = await commands.cancelJob(jobId);
      if (res.status === 'error') {
        setJobActionError(res.error);
      }
      await refreshJobs();
    },
    [refreshJobs]
  );

  const handleRetryJob = useCallback(
    async (jobId: string): Promise<JobRecord | null> => {
      setJobActionError(null);
      const res = await commands.retryJob(jobId);
      if (res.status === 'ok') {
        await refreshJobs();
        return res.data;
      }
      setJobActionError(res.error);
      return null;
    },
    [refreshJobs]
  );

  const handleOpenJobProject = useCallback(async (projectId: string) => {
    setActiveTab('home');
    const res = await commands.getProject(projectId).catch(() => null);
    if (res && res.status === 'ok' && res.data) {
      setSelectedProject(res.data);
    }
  }, []);

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

  const handleUseDefaultDataRoot = async () => {
    setIpcError(null);
    const res = await commands.useDefaultDataRoot();
    if (res.status === 'ok') {
      await loadInitialState();
    } else {
      setIpcError(res.error.message);
    }
  };

  const navItems: { id: NavTab; label: string }[] = [
    { id: 'home', label: t('nav.home', undefined, locale) },
    { id: 'components', label: t('nav.components', undefined, locale) },
    { id: 'diagnostics', label: t('nav.diagnostics', undefined, locale) },
    { id: 'settings', label: t('nav.settings', undefined, locale) },
  ];

  const activeJob =
    jobs.find((j) => {
      const st = jobEvents[j.id]?.state ?? j.status;
      return st === 'running' || st === 'queued';
    }) ?? null;
  const interruptedJob = !activeJob ? (jobs.find((j) => j.status === 'interrupted') ?? null) : null;
  const headerChipJob = activeJob ?? interruptedJob;
  const headerChipEvent = headerChipJob ? (jobEvents[headerChipJob.id] ?? null) : null;
  const headerChipPct = Math.round(headerChipEvent?.overall.percent ?? 0);
  const headerChipStage = headerChipEvent?.stage.id ?? headerChipJob?.stage ?? 'download';
  const headerChipStageLabel = t(
    `job.stage.${headerChipStage === 'prepare_preview' ? 'preparePreview' : headerChipStage}`,
    undefined,
    locale
  );
  const headerChipEta = formatEtaDisplay(
    headerChipEvent?.eta,
    locale,
    headerChipEvent?.detail?.startup === 'true'
  );

  const selectedProjectJob = selectedProject
    ? (jobs.find((j) => j.project_id === selectedProject.id) ?? null)
    : null;
  const selectedProjectJobEvent = selectedProjectJob
    ? (jobEvents[selectedProjectJob.id] ?? null)
    : null;

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
                {appInfo ? formatComponentVersion(appInfo.version) : '…'} ·{' '}
                {appInfo?.platform ?? 'windows'}
              </p>
            </div>
          </div>

          <div className="flex flex-wrap items-center gap-3">
            {headerChipJob && (
              <button
                type="button"
                data-testid="global-job-activity-chip"
                onClick={() => void handleOpenJobProject(headerChipJob.project_id)}
                className={`inline-flex items-center gap-2 px-3 py-1 rounded-full text-xs font-medium border transition ${
                  headerChipJob.status === 'interrupted'
                    ? 'bg-amber-950/70 border-amber-700 text-amber-200 hover:bg-amber-900/70'
                    : 'bg-indigo-950/80 border-indigo-700 text-indigo-100 hover:bg-indigo-900/80'
                }`}
              >
                <span
                  aria-hidden="true"
                  className={
                    headerChipJob.status === 'interrupted'
                      ? 'text-amber-300'
                      : 'text-indigo-300 animate-pulse'
                  }
                >
                  ●
                </span>
                <span>
                  {headerChipJob.status === 'interrupted'
                    ? t('job.activityChipInterrupted', undefined, locale)
                    : t(
                        'job.activityChipRunning',
                        {
                          stage: headerChipStageLabel,
                          percent: headerChipPct,
                          eta: headerChipEta,
                        },
                        locale
                      )}
                </span>
              </button>
            )}

            <nav aria-label="Main navigation" className="flex flex-wrap gap-1.5">
              {navItems.map((item) => {
                const active = activeTab === item.id;
                return (
                  <button
                    key={item.id}
                    type="button"
                    onClick={() => {
                      setActiveTab(item.id);
                      if (item.id === 'home') {
                        setSelectedProject(null);
                      }
                    }}
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

        {/* Actionable screen when bootstrap.json points to an absent/unreachable drive */}
        {wsStatus && !wsStatus.is_initialized && wsStatus.configured_root_unreachable && (
          <div
            data-testid="unreachable-dataroot-screen"
            role="alert"
            className="bg-amber-950/70 border border-amber-700/80 rounded-xl p-5 space-y-3 text-xs text-amber-100"
          >
            <h2 className="text-base font-semibold text-amber-300">
              {t('dataroot.unreachableTitle', undefined, locale)}
            </h2>
            <p className="text-amber-100/90 leading-relaxed">
              {t(
                'dataroot.unreachableBody',
                { path: wsStatus.unreachable_root_redacted ?? '(unknown)' },
                locale
              )}
            </p>
            <div className="flex flex-wrap items-center gap-2 pt-1">
              <button
                type="button"
                onClick={() => void handleUseDefaultDataRoot()}
                className="bg-amber-600 hover:bg-amber-500 text-slate-950 font-semibold px-3.5 py-2 rounded-lg transition"
              >
                {t('dataroot.useDefault', undefined, locale)}
              </button>
              <button
                type="button"
                onClick={() => void loadInitialState()}
                className="bg-slate-800 hover:bg-slate-700 border border-slate-600 text-slate-100 font-medium px-3.5 py-2 rounded-lg transition"
              >
                {t('components.retry', undefined, locale)}
              </button>
            </div>
          </div>
        )}

        {/* Shared ErrorBanner for Workspace Init or IPC failure */}
        {wsStatus && !wsStatus.is_initialized && !wsStatus.configured_root_unreachable && (
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
          {activeTab === 'home' && !selectedProject && (
            <HomeView
              locale={locale}
              overview={overview}
              settings={settings}
              jobs={jobs}
              onNavigate={setActiveTab}
              onOpenProject={(proj) => {
                setJobActionError(null);
                setSelectedProject(proj);
              }}
              onRetryJob={async (jobId) => {
                await handleRetryJob(jobId);
              }}
            />
          )}
          {activeTab === 'home' && selectedProject && (
            <RangePickerView
              locale={locale}
              project={selectedProject}
              previewConfig={previewConfig}
              projectJob={selectedProjectJob}
              projectJobEvent={selectedProjectJobEvent}
              busyJobError={jobActionError}
              onBackHome={() => {
                setJobActionError(null);
                setSelectedProject(null);
              }}
              onOpenDiagnostics={() => setActiveTab('diagnostics')}
              onOpenComponents={() => setActiveTab('components')}
              onProjectUpdated={(updated) => setSelectedProject(updated)}
              onStartGenerate={handleStartGenerate}
              onCancelJob={handleCancelJob}
              onRetryJob={handleRetryJob}
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
