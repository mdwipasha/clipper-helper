import { SupportedLocale, t } from '@clipper-helper/i18n';
import { open } from '@tauri-apps/plugin-dialog';
import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  AppCommandError,
  CapabilityReportView,
  commands,
  ComponentInstallProgressEvent,
  ComponentsOverview,
  ComponentStateSummary,
  DataRootChangeOutcome,
  events,
} from '../ipc/bindings';
import { formatBytesHuman, formatEtaText, formatVersionBadge } from '../utils/format';
import { ErrorBanner, ErrorBannerModel } from './ErrorBanner';

interface ComponentsViewProps {
  locale: SupportedLocale;
  overview: ComponentsOverview | null;
  onRefreshOverview: (overrideUrl?: string | null) => Promise<void>;
  onOpenDiagnostics: () => void;
}

function isModelName(name: string): boolean {
  return name.startsWith('ggml-') || name.endsWith('.bin');
}

function CapabilityDetailsDrawer({
  report,
  locale,
}: {
  report: CapabilityReportView;
  locale: SupportedLocale;
}) {
  return (
    <div className="mt-2 bg-black/50 border border-slate-800 rounded p-2.5 text-[11px] space-y-1">
      <div className="flex items-center justify-between text-slate-300">
        <span>
          Verifikasi ({report.mode})
          {report.reported_version ? ` · ${formatVersionBadge(report.reported_version)}` : ''}
        </span>
        <span className="uppercase font-mono font-bold text-slate-200">{report.status}</span>
      </div>
      {report.ffmpeg && (
        <div className="text-slate-300 space-y-0.5 border-t border-slate-800 pt-1">
          <div>
            Lisensi: <strong>{report.ffmpeg.effective_license}</strong> · ffprobe:{' '}
            {report.ffmpeg.ffprobe_version}
          </div>
          <div>
            {t('components.detail', undefined, locale)} Encoders in build:{' '}
            {report.ffmpeg.hardware_encoders_available.length > 0
              ? report.ffmpeg.hardware_encoders_available.join(', ')
              : 'none'}
          </div>
        </div>
      )}
      {report.reasons.length > 0 && (
        <div className="text-rose-300 space-y-0.5">
          {report.reasons.map((r, i) => (
            <div key={i}>• {r}</div>
          ))}
        </div>
      )}
    </div>
  );
}

export function ComponentsView({
  locale,
  overview,
  onRefreshOverview,
  onOpenDiagnostics,
}: ComponentsViewProps) {
  const [overrideUrl, setOverrideUrl] = useState<string>('');
  const [manifestError, setManifestError] = useState<AppCommandError | null>(null);
  const [actionError, setActionError] = useState<AppCommandError | null>(null);
  const [busy, setBusy] = useState<boolean>(false);
  const [statusMessage, setStatusMessage] = useState<string | null>(null);
  const [progress, setProgress] = useState<ComponentInstallProgressEvent | null>(null);
  const [expandedDetails, setExpandedDetails] = useState<Record<string, boolean>>({});
  const [dataRootOutcome, setDataRootOutcome] = useState<DataRootChangeOutcome | null>(null);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void events.componentInstallProgressEvent
      .listen((ev) => {
        setProgress(ev.payload);
      })
      .then((fn) => {
        unlisten = fn;
      });
    return () => {
      if (unlisten) unlisten();
    };
  }, []);

  const handleFetchManifest = useCallback(async () => {
    setBusy(true);
    setManifestError(null);
    setActionError(null);
    try {
      const res = await commands.fetchComponentsManifest(overrideUrl.trim() || null);
      if (res.status === 'ok') {
        await onRefreshOverview(overrideUrl.trim() || null);
      } else {
        setManifestError(res.error);
      }
    } finally {
      setBusy(false);
    }
  }, [onRefreshOverview, overrideUrl]);

  const handleInstallAll = async () => {
    setBusy(true);
    setActionError(null);
    setStatusMessage(null);
    setProgress(null);
    try {
      const res = await commands.installAllRequiredComponents();
      if (res.status === 'ok') {
        setStatusMessage(
          `Selesai: ${res.data.installed_count} komponen dipasang, ${res.data.skipped_count} sudah mutakhir.`
        );
        await onRefreshOverview(overrideUrl.trim() || null);
      } else {
        setActionError(res.error);
      }
    } finally {
      setBusy(false);
    }
  };

  const handleRepair = async (component: string, version: string | null) => {
    setBusy(true);
    setActionError(null);
    setStatusMessage(null);
    try {
      const res = await commands.repairComponent(component, version);
      if (res.status === 'ok') {
        setStatusMessage(`${component}: ${res.data.message}`);
        await onRefreshOverview(overrideUrl.trim() || null);
      } else {
        setActionError(res.error);
      }
    } finally {
      setBusy(false);
    }
  };

  const handleRetryQuarantine = async (component: string, version: string) => {
    setBusy(true);
    setActionError(null);
    setStatusMessage(null);
    try {
      const res = await commands.retryQuarantinedComponent(component, version);
      if (res.status === 'ok') {
        setStatusMessage(`${component}: ${res.data.message}`);
        await onRefreshOverview(overrideUrl.trim() || null);
      } else {
        setActionError(res.error);
      }
    } finally {
      setBusy(false);
    }
  };

  const handleVerify = async (component: string) => {
    setBusy(true);
    setActionError(null);
    setStatusMessage(null);
    try {
      const res = await commands.verifyComponent(component);
      if (res.status === 'ok') {
        setStatusMessage(`${component}: ${res.data.message}`);
        await onRefreshOverview(overrideUrl.trim() || null);
      } else {
        setActionError(res.error);
      }
    } finally {
      setBusy(false);
    }
  };

  const handlePickDataRoot = async () => {
    setActionError(null);
    const chosen = await open({
      directory: true,
      multiple: false,
      title: t('components.chooseAnotherLocation', undefined, locale),
    });
    if (!chosen || typeof chosen !== 'string') return;
    const res = await commands.selectCustomDataRoot(chosen);
    if (res.status === 'ok') {
      setDataRootOutcome(res.data);
      await onRefreshOverview(overrideUrl.trim() || null);
    } else {
      setActionError(res.error);
    }
  };

  const componentRows = useMemo(() => {
    const byName = new Map<string, ComponentStateSummary>();
    for (const c of overview?.diagnostics.components ?? []) {
      byName.set(c.component, c);
    }
    const order = [...(overview?.required_components ?? [])];
    for (const key of byName.keys()) {
      if (!order.includes(key)) order.push(key);
    }
    return order.map((name) => ({
      name,
      summary: byName.get(name) ?? null,
      manifestItem: overview?.manifest?.components.find((m) => m.component === name) ?? null,
    }));
  }, [overview]);

  const diskStatus = overview?.disk_space;
  const isDiskBlocked = Boolean(
    (diskStatus && !diskStatus.sufficient) || actionError?.code === 'E_DISK_FULL'
  );
  const availBytes = actionError?.disk_bounds?.available_bytes ?? diskStatus?.available_bytes ?? 0;
  const reqBytes = actionError?.disk_bounds?.required_bytes ?? diskStatus?.required_bytes ?? 0;

  const showNeutralOfflineBanner =
    !overview?.manifest ||
    manifestError?.code === 'E_NET_OFFLINE' ||
    manifestError?.code === 'E_NET_TIMEOUT';

  const criticalManifestError: ErrorBannerModel | null =
    manifestError &&
    manifestError.code !== 'E_NET_OFFLINE' &&
    manifestError.code !== 'E_NET_TIMEOUT'
      ? {
          code: manifestError.code,
          message: manifestError.message,
          redactedDetail: manifestError.detail,
          actions: [
            {
              label: t('components.retry', undefined, locale),
              variant: 'primary',
              onClick: () => void handleFetchManifest(),
            },
          ],
        }
      : null;

  const activeErrorBanner: ErrorBannerModel | null =
    actionError && actionError.code !== 'E_DISK_FULL'
      ? {
          code: actionError.code,
          message: actionError.message,
          redactedDetail: actionError.detail,
          actions: [
            {
              label: t('components.retry', undefined, locale),
              onClick: () => setActionError(null),
            },
          ],
        }
      : null;

  return (
    <section className="space-y-4">
      <div className="bg-slate-900/70 border border-slate-700/70 rounded-xl p-5 space-y-3">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div>
            <h2 className="text-lg font-semibold text-white">
              {t('components.title', undefined, locale)}
            </h2>
            <p className="text-xs text-slate-300 mt-1">
              {t('components.subtitle', undefined, locale)}
            </p>
          </div>

          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              disabled={busy}
              onClick={() => void handleFetchManifest()}
              className="bg-slate-800 hover:bg-slate-700 disabled:opacity-50 border border-slate-600 text-slate-100 text-xs font-medium px-3 py-2 rounded-lg transition"
            >
              {t('components.retry', undefined, locale)}
            </button>
            <button
              type="button"
              disabled={busy || isDiskBlocked || !overview?.manifest}
              onClick={() => void handleInstallAll()}
              className="bg-indigo-600 hover:bg-indigo-500 disabled:opacity-50 text-white text-xs font-semibold px-4 py-2 rounded-lg transition"
            >
              {busy
                ? t('components.installing', undefined, locale)
                : t('components.installAll', undefined, locale)}
            </button>
          </div>
        </div>

        {overview?.manifest_url_config.can_override_in_ui && (
          <div className="flex flex-wrap items-center gap-2 pt-2 border-t border-slate-800 text-xs">
            <label htmlFor="debug-manifest-url" className="text-slate-400">
              Debug Manifest URL:
            </label>
            <input
              id="debug-manifest-url"
              type="text"
              value={overrideUrl}
              onChange={(e) => setOverrideUrl(e.target.value)}
              placeholder={overview.manifest_url_config.url}
              className="flex-1 min-w-[240px] bg-slate-950 border border-slate-700 rounded px-2.5 py-1.5 text-slate-200 font-mono text-[11px]"
            />
          </div>
        )}
      </div>

      {/* Neutral offline state with Retry */}
      {showNeutralOfflineBanner && (
        <div
          data-testid="components-offline-banner"
          className="bg-slate-800/80 border border-slate-600/80 rounded-lg p-4 text-xs flex flex-wrap items-center justify-between gap-3"
        >
          <div className="space-y-1">
            <div className="font-semibold text-slate-100">
              {t('components.offlineTitle', undefined, locale)}
            </div>
            <div className="text-slate-300">
              {manifestError?.message ?? t('components.offlineBody', undefined, locale)}
            </div>
          </div>
          <button
            type="button"
            disabled={busy}
            onClick={() => void handleFetchManifest()}
            className="bg-slate-700 hover:bg-slate-600 text-white font-medium px-3 py-1.5 rounded text-xs transition"
          >
            {t('components.retry', undefined, locale)}
          </button>
        </div>
      )}

      {criticalManifestError && (
        <ErrorBanner
          error={criticalManifestError}
          locale={locale}
          onOpenDiagnostics={onOpenDiagnostics}
        />
      )}

      {activeErrorBanner && (
        <ErrorBanner
          error={activeErrorBanner}
          locale={locale}
          onOpenDiagnostics={onOpenDiagnostics}
        />
      )}

      {/* Blocking Pre-Install Disk Space Check */}
      {isDiskBlocked && (
        <div
          data-testid="disk-insufficient-blocking"
          role="alert"
          className="bg-amber-950/60 border border-amber-700/80 rounded-lg p-4 text-xs space-y-2.5 text-amber-100"
        >
          <div className="font-semibold text-sm text-amber-300">
            {t('components.diskInsufficientTitle', undefined, locale)}
          </div>
          <p>
            {t(
              'components.diskInsufficientBody',
              {
                required: formatBytesHuman(reqBytes),
                available: formatBytesHuman(availBytes),
              },
              locale
            )}
          </p>
          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              onClick={() => void handlePickDataRoot()}
              className="bg-amber-600 hover:bg-amber-500 text-slate-950 font-semibold px-3 py-1.5 rounded text-xs transition"
            >
              {t('components.chooseAnotherLocation', undefined, locale)}
            </button>
            <button
              type="button"
              onClick={onOpenDiagnostics}
              className="text-indigo-300 underline text-xs px-2 py-1"
            >
              {t('errorBanner.openDiagnostics', undefined, locale)}
            </button>
          </div>
        </div>
      )}

      {/* Bootstrap.json restart & no-migration notice */}
      {dataRootOutcome && (
        <div
          data-testid="dataroot-restart-notice"
          className="bg-indigo-950/60 border border-indigo-700/70 rounded-lg p-3.5 text-xs text-indigo-100 space-y-1"
        >
          <div>
            {t('components.restartWarning', { path: dataRootOutcome.new_root_redacted }, locale)}
          </div>
          {dataRootOutcome.is_onedrive && (
            <div className="text-amber-300 font-medium">
              {t('components.oneDriveWarning', undefined, locale)}
            </div>
          )}
        </div>
      )}

      {overview?.diagnostics.custom_ffmpeg.warning_flag && (
        <div className="bg-amber-950/50 border border-amber-700/70 rounded-lg p-3 text-xs text-amber-200">
          {overview.diagnostics.custom_ffmpeg.warning_message ??
            t('warning.custom_ffmpeg', undefined, locale)}
        </div>
      )}

      {/* Live Per-Item + Overall Install Progress / ETA */}
      {progress && progress.stage !== 'complete' && (
        <div
          data-testid="install-progress-card"
          className="bg-slate-900/90 border border-indigo-800/70 rounded-lg p-4 text-xs space-y-2.5"
        >
          <div className="flex items-center justify-between text-slate-200 font-medium">
            <span>
              Keseluruhan ({progress.completed_items}/{progress.total_items}):{' '}
              {progress.overall_percent.toFixed(0)}%
            </span>
            <span className="text-slate-400">
              ETA: {formatEtaText(progress.overall_eta_seconds)}
            </span>
          </div>
          <div className="w-full bg-slate-800 rounded-full h-2 overflow-hidden">
            <div
              className="bg-indigo-500 h-2 transition-all"
              style={{ width: `${Math.min(100, Math.max(0, progress.overall_percent))}%` }}
            />
          </div>

          <div className="flex items-center justify-between text-slate-300 pt-1">
            <span>
              {progress.component} ({progress.stage}): {progress.item_percent.toFixed(0)}%
            </span>
            <span className="text-slate-400 font-mono">
              {formatBytesHuman(progress.item_completed_bytes)} /{' '}
              {formatBytesHuman(progress.item_total_bytes)} · ETA{' '}
              {formatEtaText(progress.item_eta_seconds)}
            </span>
          </div>
          <div className="w-full bg-slate-800 rounded-full h-1.5 overflow-hidden">
            <div
              className="bg-emerald-500 h-1.5 transition-all"
              style={{ width: `${Math.min(100, Math.max(0, progress.item_percent))}%` }}
            />
          </div>
        </div>
      )}

      {statusMessage && (
        <div className="bg-emerald-950/60 border border-emerald-800/70 rounded-lg p-3 text-xs text-emerald-200">
          {statusMessage}
        </div>
      )}

      {/* Component Rows */}
      <div className="space-y-2.5">
        {componentRows.map(({ name, summary, manifestItem }) => {
          const model = isModelName(name);
          const currentVer = summary?.current_version ?? null;
          const quarantinedVers = (summary?.versions ?? []).filter(
            (v) => v.status === 'quarantined'
          );
          const activeVerInfo = (summary?.versions ?? []).find((v) => v.version === currentVer);
          const capReport = activeVerInfo?.capability_report ?? null;
          const isMissing = Boolean(summary?.missing_advice) || !currentVer;
          const detailsOpen = Boolean(expandedDetails[name]);

          const plainStatusLabel = isMissing
            ? t('components.statusMissing', undefined, locale)
            : model
              ? t('components.statusModelVerified', undefined, locale)
              : t('components.statusInstalled', undefined, locale);

          return (
            <div
              key={name}
              data-testid={`component-row-${name}`}
              className="bg-slate-900/70 border border-slate-700/70 rounded-xl p-4 text-xs space-y-2"
            >
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-semibold text-sm text-white">{name}</span>
                  {currentVer && (
                    <span className="font-mono text-[11px] bg-slate-800 text-slate-200 px-2 py-0.5 rounded">
                      {formatVersionBadge(currentVer)}
                    </span>
                  )}
                  <span
                    className={`px-2 py-0.5 rounded text-[11px] font-medium border ${
                      isMissing
                        ? 'bg-amber-950/70 text-amber-300 border-amber-800'
                        : 'bg-emerald-950/70 text-emerald-300 border-emerald-800'
                    }`}
                  >
                    {plainStatusLabel}
                  </span>
                  {manifestItem && !currentVer && (
                    <span className="text-slate-400">
                      ({formatVersionBadge(manifestItem.version)} ·{' '}
                      {formatBytesHuman(manifestItem.size_bytes)})
                    </span>
                  )}
                </div>

                <div className="flex flex-wrap items-center gap-2">
                  {capReport && !model && (
                    <button
                      type="button"
                      onClick={() =>
                        setExpandedDetails((prev) => ({ ...prev, [name]: !prev[name] }))
                      }
                      className="text-slate-300 hover:text-white border border-slate-700 px-2.5 py-1 rounded transition"
                    >
                      {detailsOpen
                        ? t('components.hideDetail', undefined, locale)
                        : t('components.detail', undefined, locale)}
                    </button>
                  )}

                  {currentVer && (
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => void handleVerify(name)}
                      className="bg-slate-800 hover:bg-slate-700 border border-slate-600 text-slate-200 px-2.5 py-1 rounded transition"
                    >
                      {t('components.verify', undefined, locale)}
                    </button>
                  )}

                  {overview?.manifest && (
                    <button
                      type="button"
                      disabled={busy || isDiskBlocked}
                      onClick={() =>
                        void handleRepair(name, manifestItem?.version ?? currentVer ?? null)
                      }
                      className="bg-slate-800 hover:bg-slate-700 border border-slate-600 text-slate-200 px-2.5 py-1 rounded transition"
                    >
                      {t('components.repair', undefined, locale)}
                    </button>
                  )}
                </div>
              </div>

              {summary?.missing_advice && (
                <div className="text-amber-300 bg-amber-950/40 border border-amber-800/60 rounded p-2">
                  {summary.missing_advice.message}
                </div>
              )}

              {quarantinedVers.map((qv) => (
                <div
                  key={qv.version}
                  className="bg-rose-950/40 border border-rose-800/70 rounded p-2.5 flex flex-wrap items-center justify-between gap-2"
                >
                  <div className="text-rose-200">
                    <strong>
                      {t('components.statusQuarantined', undefined, locale)}{' '}
                      {formatVersionBadge(qv.version)}:
                    </strong>{' '}
                    {qv.quarantine_reason ?? 'Gagal pemeriksaan kapabilitas'}
                  </div>
                  {!model && (
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => void handleRetryQuarantine(name, qv.version)}
                      className="bg-rose-800 hover:bg-rose-700 text-white px-2.5 py-1 rounded transition"
                    >
                      {t('components.retryVerification', undefined, locale)}
                    </button>
                  )}
                </div>
              ))}

              {detailsOpen && capReport && (
                <CapabilityDetailsDrawer report={capReport} locale={locale} />
              )}
            </div>
          );
        })}
      </div>

      {/* Footer Disk Summary & Manual Location Picker */}
      {diskStatus && (
        <div className="bg-slate-900/50 border border-slate-800 rounded-xl p-4 text-xs flex flex-wrap items-center justify-between gap-3">
          <div className="text-slate-300 space-y-0.5">
            <div>
              Lokasi Data:{' '}
              <span className="font-mono text-slate-200">{diskStatus.data_root_redacted}</span>
            </div>
            <div className="text-slate-400">
              Tersedia: {formatBytesHuman(diskStatus.available_bytes)} · Dibutuhkan min:{' '}
              {formatBytesHuman(diskStatus.required_bytes)}
            </div>
          </div>
          <button
            type="button"
            onClick={() => void handlePickDataRoot()}
            className="bg-slate-800 hover:bg-slate-700 border border-slate-600 text-slate-200 px-3 py-1.5 rounded text-xs transition"
          >
            {t('components.chooseAnotherLocation', undefined, locale)}
          </button>
        </div>
      )}
    </section>
  );
}
