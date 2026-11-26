import { SupportedLocale, t } from '@clipper-helper/i18n';
import { ClipboardEvent, FormEvent, useCallback, useEffect, useRef, useState } from 'react';
import {
  AppCommandError,
  commands,
  ComponentsOverview,
  events,
  ProbeProjectResponse,
  ProjectRecord,
  SettingsOverview,
} from '../ipc/bindings';
import { formatDurationHms } from '../utils/format';
import { ErrorBanner, ErrorBannerAction, toErrorBannerModel } from './ErrorBanner';

export type NavTab = 'home' | 'components' | 'diagnostics' | 'settings';

interface HomeViewProps {
  locale: SupportedLocale;
  overview: ComponentsOverview | null;
  settings: SettingsOverview | null;
  onNavigate: (tab: NavTab) => void;
}

const RETRYABLE_PROBE_CODES = new Set(['E_NET_OFFLINE', 'E_NET_TIMEOUT', 'E_YT_RATE_LIMITED']);

export function HomeView({ locale, overview, settings, onNavigate }: HomeViewProps) {
  const [urlInput, setUrlInput] = useState('');
  const [isProbing, setIsProbing] = useState(false);
  const [elapsedMs, setElapsedMs] = useState(0);
  const [probeError, setProbeError] = useState<AppCommandError | null>(null);
  const [probeResult, setProbeResult] = useState<ProbeProjectResponse | null>(null);
  const [cancelledNotice, setCancelledNotice] = useState(false);
  const [recentProjects, setRecentProjects] = useState<ProjectRecord[]>([]);
  const [deletingId, setDeletingId] = useState<string | null>(null);

  const activeRequestIdRef = useRef<string | null>(null);
  const timerStartRef = useRef<number | null>(null);

  const required = overview?.required_components ?? [];
  const installedNames = new Set(
    (overview?.diagnostics.components ?? [])
      .filter((c) => Boolean(c.current_version))
      .map((c) => c.component)
  );
  const allReady = required.length > 0 && required.every((name) => installedNames.has(name));

  const loadRecentProjects = useCallback(async () => {
    try {
      const res = await commands.listRecentProjects();
      if (res.status === 'ok') {
        setRecentProjects(res.data);
      }
    } catch {
      // Non-fatal in test/startup transitions
    }
  }, []);

  useEffect(() => {
    void loadRecentProjects();
  }, [loadRecentProjects]);

  useEffect(() => {
    let disposed = false;
    let unlistenFn: (() => void) | null = null;

    events.probeProgressEvent
      .listen((ev) => {
        if (disposed) return;
        if (activeRequestIdRef.current && ev.payload.request_id === activeRequestIdRef.current) {
          setElapsedMs((prev) => Math.max(prev, ev.payload.elapsed_ms));
        }
      })
      .then((fn) => {
        if (disposed) {
          fn();
        } else {
          unlistenFn = fn;
        }
      })
      .catch(() => undefined);

    return () => {
      disposed = true;
      if (unlistenFn) unlistenFn();
    };
  }, []);

  useEffect(() => {
    if (!isProbing) return;
    timerStartRef.current = Date.now();
    const handle = window.setInterval(() => {
      if (timerStartRef.current !== null) {
        setElapsedMs(Date.now() - timerStartRef.current);
      }
    }, 250);
    return () => {
      window.clearInterval(handle);
    };
  }, [isProbing]);

  const runProbe = useCallback(
    async (rawInput: string) => {
      const trimmed = rawInput.trim();
      if (!trimmed) return;

      const reqId =
        typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function'
          ? crypto.randomUUID()
          : `probe-${Date.now()}`;

      activeRequestIdRef.current = reqId;
      setProbeError(null);
      setCancelledNotice(false);
      setElapsedMs(0);
      setIsProbing(true);

      try {
        const res = await commands.probeYoutubeUrl(trimmed, reqId);
        if (res.status === 'ok') {
          setProbeResult(res.data);
          setProbeError(null);
          await loadRecentProjects();
        } else if (res.error.message_key === 'home.probeCancelled') {
          setCancelledNotice(true);
        } else {
          setProbeResult(null);
          setProbeError(res.error);
        }
      } catch (err) {
        setProbeResult(null);
        setProbeError({
          code: 'E_UNKNOWN',
          message_key: 'err.E_UNKNOWN.body',
          message: err instanceof Error ? err.message : String(err),
          detail: err instanceof Error ? err.message : String(err),
          disk_bounds: null,
        });
      } finally {
        if (activeRequestIdRef.current === reqId) {
          activeRequestIdRef.current = null;
        }
        setIsProbing(false);
      }
    },
    [loadRecentProjects]
  );

  const handleSubmit = (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    void runProbe(urlInput);
  };

  const handlePaste = (e: ClipboardEvent<HTMLInputElement>) => {
    const pasted = e.clipboardData?.getData('text');
    if (typeof pasted === 'string') {
      e.preventDefault();
      setUrlInput(pasted.trim());
    }
  };

  const handleCancelProbe = async () => {
    const reqId = activeRequestIdRef.current;
    if (!reqId) return;
    await commands.cancelYoutubeProbe(reqId).catch(() => undefined);
  };

  const handleDeleteProject = async (projectId: string) => {
    setDeletingId(projectId);
    try {
      const res = await commands.deleteProject(projectId);
      if (res.status === 'ok' && res.data) {
        setRecentProjects((prev) => prev.filter((p) => p.id !== projectId));
        setProbeResult((prev) => (prev && prev.project.id === projectId ? null : prev));
      }
    } finally {
      setDeletingId(null);
    }
  };

  const errorBannerModel = probeError
    ? (() => {
        const actions: ErrorBannerAction[] = [];
        if (RETRYABLE_PROBE_CODES.has(probeError.code)) {
          actions.push({
            label: t('home.retryButton', undefined, locale),
            variant: 'primary',
            onClick: () => void runProbe(urlInput),
          });
        }
        if (probeError.code === 'E_COMPONENT_MISSING') {
          actions.push({
            label: t('home.goComponents', undefined, locale),
            variant: 'primary',
            onClick: () => onNavigate('components'),
          });
        }
        return toErrorBannerModel(probeError, locale, actions);
      })()
    : null;

  const elapsedSeconds = Math.max(0, Math.floor(elapsedMs / 1000));

  return (
    <section className="space-y-4">
      <div className="bg-slate-900/70 border border-slate-700/70 rounded-xl p-5 space-y-4">
        <div>
          <h2 className="text-lg font-semibold text-white">{t('home.title', undefined, locale)}</h2>
          <p className="text-sm text-slate-300 leading-relaxed">
            {t('home.subtitle', undefined, locale)}
          </p>
        </div>

        <form onSubmit={handleSubmit} className="space-y-3">
          <label htmlFor="youtube-url-input" className="block text-xs font-medium text-slate-300">
            {t('home.urlLabel', undefined, locale)}
          </label>
          <div className="flex flex-col sm:flex-row gap-2">
            <input
              id="youtube-url-input"
              type="text"
              value={urlInput}
              onChange={(e) => setUrlInput(e.target.value)}
              onPaste={handlePaste}
              placeholder={t('home.urlPlaceholder', undefined, locale)}
              disabled={isProbing}
              className="flex-1 bg-slate-950/90 border border-slate-700 focus:border-indigo-500 rounded-lg px-3 py-2 text-sm text-slate-100 placeholder-slate-500 outline-none disabled:opacity-60"
            />
            <button
              type="submit"
              disabled={isProbing || !urlInput.trim()}
              className="bg-indigo-600 hover:bg-indigo-500 disabled:opacity-50 disabled:cursor-not-allowed text-white text-sm font-medium px-4 py-2 rounded-lg transition"
            >
              {isProbing
                ? t('home.checkingButton', undefined, locale)
                : t('home.checkLinkButton', undefined, locale)}
            </button>
          </div>
        </form>

        {isProbing && (
          <div
            data-testid="probe-loading"
            className="bg-slate-950/80 border border-indigo-700/60 rounded-lg p-3 space-y-2"
          >
            <div className="flex items-center justify-between gap-2 text-xs">
              <span className="text-indigo-200 font-medium">
                {t('stage.probe', undefined, locale)}
              </span>
              <span data-testid="probe-elapsed" className="text-slate-300 font-mono">
                {t('home.elapsedSeconds', { seconds: elapsedSeconds }, locale)}
              </span>
            </div>
            <div className="h-1.5 w-full bg-slate-800 rounded-full overflow-hidden">
              <div className="h-full w-1/3 bg-indigo-500 animate-pulse rounded-full" />
            </div>
            <div className="flex justify-end">
              <button
                type="button"
                onClick={() => void handleCancelProbe()}
                className="bg-slate-800 hover:bg-slate-700 border border-slate-600 text-slate-200 text-xs font-medium px-3 py-1 rounded transition"
              >
                {t('home.cancelProbe', undefined, locale)}
              </button>
            </div>
          </div>
        )}

        {cancelledNotice && !isProbing && !probeError && (
          <div className="bg-slate-800/70 border border-slate-700 rounded-lg px-3 py-2 text-xs text-slate-300">
            {t('home.probeCancelled', undefined, locale)}
          </div>
        )}

        {errorBannerModel && (
          <ErrorBanner
            error={errorBannerModel}
            locale={locale}
            onOpenDiagnostics={() => onNavigate('diagnostics')}
            testId="home-probe-error-banner"
          />
        )}

        {probeResult && !isProbing && (
          <div
            data-testid="probe-result-card"
            className="bg-slate-950/80 border border-emerald-800/60 rounded-xl p-4 space-y-3"
          >
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span className="text-xs font-semibold uppercase tracking-wide text-emerald-300">
                {t('home.resultHeading', undefined, locale)}
              </span>
              <span
                className={`px-2 py-0.5 rounded text-xs font-medium border ${
                  probeResult.probe_info.preview_mode === 'embed'
                    ? 'bg-emerald-950/70 text-emerald-300 border-emerald-700'
                    : 'bg-amber-950/70 text-amber-300 border-amber-700'
                }`}
              >
                {probeResult.probe_info.preview_mode === 'embed'
                  ? t('home.previewAvailable', undefined, locale)
                  : t('home.previewTimecodeOnly', undefined, locale)}
              </span>
            </div>

            <div className="space-y-1">
              <h3 className="text-base font-semibold text-white break-words">
                {probeResult.probe_info.title}
              </h3>
              <p className="text-xs text-slate-300">
                {t('home.channelLabel', undefined, locale)}:{' '}
                <span className="text-slate-100 font-medium">{probeResult.probe_info.channel}</span>
              </p>
            </div>

            <div className="flex flex-wrap gap-3 text-xs text-slate-300">
              <span>
                {t('home.durationLabel', undefined, locale)}:{' '}
                <strong className="text-slate-100 font-mono">
                  {formatDurationHms(probeResult.probe_info.duration_seconds)}
                </strong>
              </span>
              <span>
                {t('home.statusLabel', undefined, locale)}:{' '}
                <strong className="text-slate-100">
                  {probeResult.probe_info.was_live
                    ? t('home.statusRecordedLive', undefined, locale)
                    : t('home.statusVod', undefined, locale)}
                </strong>
              </span>
              {probeResult.suggested_start_seconds !== null && (
                <span>
                  {t(
                    'home.suggestedStartLabel',
                    { time: formatDurationHms(probeResult.suggested_start_seconds) },
                    locale
                  )}
                </span>
              )}
              <span>
                {t('home.videoIdLabel', undefined, locale)}:{' '}
                <code className="text-cyan-300">{probeResult.probe_info.id}</code>
              </span>
            </div>

            <div className="flex items-center gap-3 pt-1">
              <button
                type="button"
                disabled
                aria-disabled="true"
                className="bg-indigo-600/40 text-slate-300 text-xs font-medium px-4 py-2 rounded-lg cursor-not-allowed"
              >
                {t('home.nextButton', undefined, locale)}
              </button>
              <span className="text-xs text-slate-400">
                {t('home.comingSoonHint', undefined, locale)}
              </span>
            </div>
          </div>
        )}
      </div>

      <div className="bg-slate-900/70 border border-slate-700/70 rounded-xl p-5 space-y-3">
        <h3 className="text-sm font-semibold text-white">
          {t('home.recentTitle', undefined, locale)}
        </h3>
        {recentProjects.length === 0 ? (
          <p className="text-xs text-slate-400">{t('home.recentEmpty', undefined, locale)}</p>
        ) : (
          <ul className="divide-y divide-slate-800">
            {recentProjects.map((proj) => (
              <li
                key={proj.id}
                data-testid={`recent-project-${proj.id}`}
                className="py-2.5 flex items-center justify-between gap-3"
              >
                <div className="min-w-0 space-y-0.5">
                  <div className="text-sm font-medium text-slate-100 truncate">{proj.title}</div>
                  <div className="text-xs text-slate-400 flex flex-wrap gap-2">
                    <span>{proj.channel}</span>
                    <span>·</span>
                    <span className="font-mono">{formatDurationHms(proj.duration_seconds)}</span>
                    <span>·</span>
                    <span>
                      {proj.preview_mode === 'embed'
                        ? t('home.previewAvailable', undefined, locale)
                        : t('home.previewTimecodeOnly', undefined, locale)}
                    </span>
                  </div>
                </div>
                <button
                  type="button"
                  disabled={deletingId === proj.id}
                  onClick={() => void handleDeleteProject(proj.id)}
                  className="shrink-0 bg-slate-800 hover:bg-rose-900/70 border border-slate-700 hover:border-rose-700 text-slate-200 text-xs px-2.5 py-1 rounded transition disabled:opacity-50"
                >
                  {t('home.deleteProject', undefined, locale)}
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        <div className="bg-slate-900/70 border border-slate-700/70 rounded-xl p-4 space-y-3">
          <div className="flex items-center justify-between">
            <span className="text-xs uppercase tracking-wide text-slate-400 font-semibold">
              {t('nav.components', undefined, locale)}
            </span>
            <span
              className={`px-2 py-0.5 rounded text-xs font-medium ${
                allReady
                  ? 'bg-emerald-950/70 text-emerald-300 border border-emerald-800'
                  : 'bg-amber-950/70 text-amber-300 border border-amber-800'
              }`}
            >
              {allReady
                ? t('components.statusInstalled', undefined, locale)
                : t('components.statusMissing', undefined, locale)}
            </span>
          </div>
          <p className="text-xs text-slate-300">
            {allReady
              ? t('home.componentsReady', undefined, locale)
              : t('home.componentsMissing', undefined, locale)}
          </p>
          <button
            type="button"
            onClick={() => onNavigate('components')}
            className="bg-indigo-600 hover:bg-indigo-500 text-white text-xs font-medium px-3 py-2 rounded-lg transition"
          >
            {t('home.goComponents', undefined, locale)}
          </button>
        </div>

        <div className="bg-slate-900/70 border border-slate-700/70 rounded-xl p-4 space-y-3">
          <div className="text-xs uppercase tracking-wide text-slate-400 font-semibold">
            {t('settings.title', undefined, locale)}
          </div>
          <div className="text-xs text-slate-300 space-y-1">
            <div>
              {t('settings.speechTitle', undefined, locale)}:{' '}
              <strong className="text-cyan-300">
                {settings?.speech_state.active_model ?? 'base'}
              </strong>
            </div>
            <div>
              {t('settings.encoderTitle', undefined, locale)}:{' '}
              <strong className="text-indigo-300">
                {settings?.encoder_status.cached_encoder ?? 'lazy (auto)'}
              </strong>
            </div>
          </div>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              onClick={() => onNavigate('settings')}
              className="bg-slate-800 hover:bg-slate-700 border border-slate-600 text-slate-100 text-xs font-medium px-3 py-2 rounded-lg transition"
            >
              {t('home.goSettings', undefined, locale)}
            </button>
            <button
              type="button"
              onClick={() => onNavigate('diagnostics')}
              className="bg-slate-800 hover:bg-slate-700 border border-slate-600 text-slate-100 text-xs font-medium px-3 py-2 rounded-lg transition"
            >
              {t('home.goDiagnostics', undefined, locale)}
            </button>
          </div>
        </div>
      </div>
    </section>
  );
}
