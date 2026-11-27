import { SupportedLocale, t } from '@clipper-helper/i18n';
import { useState } from 'react';
import { AppCommandError, JobEventPayload, JobRecord } from '../ipc/bindings';
import { formatEtaDisplay, formatRateDisplay } from '../utils/etaFormat';
import { formatBytesHuman } from '../utils/format';
import { ErrorBanner, ErrorBannerAction, toErrorBannerModel } from './ErrorBanner';

interface OperationPanelProps {
  locale: SupportedLocale;
  job: JobRecord | null;
  liveEvent: JobEventPayload | null;
  busyError?: AppCommandError | null;
  onCancelJob: (jobId: string) => Promise<void>;
  onRetryJob: (jobId: string) => Promise<void>;
  onNavigateRepair?: () => void;
  onNavigateDiagnostics?: () => void;
}

const STAGE_LABEL_BY_ID: Record<string, string> = {
  probe: 'job.stage.probe',
  download: 'job.stage.download',
  prepare_preview: 'job.stage.preparePreview',
  build: 'job.stage.build',
};

export function parseJobProgressPayload(
  job: JobRecord | null,
  liveEvent: JobEventPayload | null
): JobEventPayload | null {
  if (liveEvent && (!job || liveEvent.jobId === job.id)) {
    return liveEvent;
  }
  if (job?.progress_json) {
    try {
      return JSON.parse(job.progress_json) as JobEventPayload;
    } catch {
      return null;
    }
  }
  return null;
}

export function OperationPanel({
  locale,
  job,
  liveEvent,
  busyError,
  onCancelJob,
  onRetryJob,
  onNavigateRepair,
  onNavigateDiagnostics,
}: OperationPanelProps) {
  const [confirmingCancel, setConfirmingCancel] = useState(false);
  const [submittingCancel, setSubmittingCancel] = useState(false);
  const [retrying, setRetrying] = useState(false);

  if (!job && !busyError) {
    return null;
  }

  const event = parseJobProgressPayload(job, liveEvent);
  const effectiveState = liveEvent?.state ?? job?.status ?? 'queued';
  const isActive = effectiveState === 'queued' || effectiveState === 'running';
  const isInterrupted = effectiveState === 'interrupted';
  const isFailed = effectiveState === 'failed';
  const isCancelled = effectiveState === 'cancelled';
  const isSucceeded = effectiveState === 'succeeded';

  const currentStageId = event?.stage.id ?? job?.stage ?? 'probe';
  const detail = event?.detail ?? {};
  const needsProxy =
    detail.needsProxy === 'true' ||
    event?.stage.count === 4 ||
    currentStageId === 'prepare_preview';

  const stages = needsProxy
    ? ['probe', 'download', 'prepare_preview', 'build']
    : ['probe', 'download', 'build'];

  const currentStageIdx = Math.max(
    0,
    stages.findIndex((s) => s === currentStageId)
  );
  const overallPct = Math.min(100, Math.max(0, event?.overall.percent ?? (isSucceeded ? 100 : 0)));
  const isStartupIndeterminate =
    isActive && currentStageId === 'download' && detail.startup === 'true';
  const stallStatus = detail.stallStatus ?? 'normal';
  const isCacheHit = detail.cacheHit === 'true';

  const stageTitleKey = STAGE_LABEL_BY_ID[currentStageId] ?? 'job.stage.download';
  const stageLabel = isStartupIndeterminate
    ? t('job.preparingDownload', undefined, locale)
    : t(stageTitleKey, undefined, locale);

  const etaText = formatEtaDisplay(event?.eta, locale, isStartupIndeterminate);
  const rateText = formatRateDisplay(event?.rate, locale);

  const handleConfirmCancel = async () => {
    if (!job) return;
    setSubmittingCancel(true);
    try {
      await onCancelJob(job.id);
      setConfirmingCancel(false);
    } finally {
      setSubmittingCancel(false);
    }
  };

  const handleRetry = async () => {
    if (!job) return;
    setRetrying(true);
    try {
      await onRetryJob(job.id);
    } finally {
      setRetrying(false);
    }
  };

  const sizeBytesRaw = Number(detail.sizeBytes ?? '0');
  const widthRaw = detail.width;
  const heightRaw = detail.height;
  const fpsRaw = detail.fps;
  const codecRaw = detail.videoCodec;

  const failureActions: ErrorBannerAction[] = [];
  if (isFailed && job) {
    const actionKeys = event?.error?.actionKeys ?? ['retry'];
    for (const key of actionKeys) {
      if (key === 'repair' && onNavigateRepair) {
        failureActions.push({
          label: t('job.actionRepair', undefined, locale),
          variant: 'primary',
          onClick: onNavigateRepair,
        });
      } else if (key === 'check_connection') {
        failureActions.push({
          label: t('job.actionCheckConnection', undefined, locale),
          onClick: () => void handleRetry(),
        });
      } else if (key === 'clear_cache' && onNavigateDiagnostics) {
        failureActions.push({
          label: t('job.actionClearCache', undefined, locale),
          onClick: onNavigateDiagnostics,
        });
      } else if (key === 'retry') {
        failureActions.push({
          label: t('job.actionRetry', undefined, locale),
          variant: failureActions.length === 0 ? 'primary' : 'secondary',
          onClick: () => void handleRetry(),
        });
      }
    }
    if (failureActions.length === 0) {
      failureActions.push({
        label: t('job.actionRetry', undefined, locale),
        variant: 'primary',
        onClick: () => void handleRetry(),
      });
    }
  }

  return (
    <section
      data-testid="operation-panel"
      className="bg-slate-950/80 border border-slate-700/80 rounded-xl p-4 space-y-3 text-xs"
    >
      {busyError && (
        <ErrorBanner
          testId="operation-busy-banner"
          locale={locale}
          error={toErrorBannerModel(busyError, locale)}
          onOpenDiagnostics={onNavigateDiagnostics ?? (() => undefined)}
        />
      )}

      {job && (
        <>
          {/* Stage list (✓ / ● / ○) */}
          <ol className="flex flex-wrap items-center gap-3 text-xs">
            {stages.map((sid, idx) => {
              const isDone = isSucceeded || idx < currentStageIdx;
              const isCurr = !isDone && idx === currentStageIdx;
              const marker = isDone ? '✓' : isCurr ? '●' : '○';
              const tone = isDone
                ? 'text-emerald-300 border-emerald-800/80 bg-emerald-950/50'
                : isCurr
                  ? 'text-indigo-200 border-indigo-600 bg-indigo-950/60 font-semibold'
                  : 'text-slate-400 border-slate-800 bg-slate-900/50';
              return (
                <li
                  key={sid}
                  data-testid={`job-stage-${sid}`}
                  className={`inline-flex items-center gap-1.5 px-2.5 py-1 rounded-full border ${tone}`}
                >
                  <span aria-hidden="true">{marker}</span>
                  <span>
                    {t(STAGE_LABEL_BY_ID[sid] ?? 'job.stage.download', undefined, locale)}
                  </span>
                </li>
              );
            })}
          </ol>

          {/* Status summary line + overall bar */}
          {!isFailed && !isInterrupted && !isCancelled && !isSucceeded && (
            <div className="space-y-2">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <span data-testid="operation-status-line" className="text-slate-100 font-medium">
                  {t('job.stepOf', { current: currentStageIdx + 1, total: stages.length }, locale)}{' '}
                  · {stageLabel} · <strong className="font-mono">{Math.round(overallPct)}%</strong>{' '}
                  · <span className="text-cyan-300">{etaText}</span>
                </span>
                {rateText && (
                  <span data-testid="operation-rate-text" className="text-slate-300 font-mono">
                    {rateText}
                  </span>
                )}
              </div>

              <div className="w-full h-2.5 bg-slate-900 rounded-full overflow-hidden border border-slate-800">
                {isStartupIndeterminate ? (
                  <div
                    data-testid="operation-indeterminate-bar"
                    className="h-full w-1/3 bg-indigo-500 rounded-full animate-pulse"
                  />
                ) : (
                  <div
                    data-testid="operation-progress-bar"
                    className="h-full bg-indigo-500 transition-all duration-200"
                    style={{ width: `${overallPct}%` }}
                  />
                )}
              </div>
            </div>
          )}

          {/* Stall warnings (15 s / 60 s) */}
          {isActive && stallStatus === 'slow_warning' && (
            <div
              data-testid="operation-stall-warning"
              role="status"
              className="bg-amber-950/60 border border-amber-700/70 text-amber-200 rounded-lg px-3 py-2"
            >
              {t('job.stallWarning', undefined, locale)}
            </div>
          )}

          {isActive && stallStatus === 'stalled_retry' && (
            <div
              data-testid="operation-stall-retry"
              role="alert"
              className="bg-amber-950/70 border border-amber-600/80 text-amber-100 rounded-lg px-3 py-2 flex flex-wrap items-center justify-between gap-2"
            >
              <span>{t('job.stallRetryHint', undefined, locale)}</span>
              <button
                type="button"
                disabled={retrying}
                onClick={() => void handleConfirmCancel()}
                className="bg-amber-600 hover:bg-amber-500 text-slate-950 font-semibold px-3 py-1 rounded transition"
              >
                {t('job.cancelButton', undefined, locale)}
              </button>
            </div>
          )}

          {/* Interrupted job banner */}
          {isInterrupted && (
            <div
              data-testid="operation-interrupted-banner"
              role="alert"
              className="bg-amber-950/70 border border-amber-700/80 text-amber-100 rounded-lg px-3.5 py-2.5 flex flex-wrap items-center justify-between gap-2"
            >
              <span>{t('job.interruptedNotice', { stage: stageLabel }, locale)}</span>
              <button
                type="button"
                disabled={retrying}
                onClick={() => void handleRetry()}
                className="bg-amber-600 hover:bg-amber-500 text-slate-950 font-semibold px-3.5 py-1.5 rounded-lg transition"
              >
                {t('job.retryButton', undefined, locale)}
              </button>
            </div>
          )}

          {/* Cancelled banner */}
          {isCancelled && (
            <div
              data-testid="operation-cancelled-banner"
              className="bg-slate-900 border border-slate-700 text-slate-200 rounded-lg px-3.5 py-2.5 flex flex-wrap items-center justify-between gap-2"
            >
              <span>{t('job.cancelledNotice', undefined, locale)}</span>
              <button
                type="button"
                disabled={retrying}
                onClick={() => void handleRetry()}
                className="bg-indigo-600 hover:bg-indigo-500 text-white font-semibold px-3 py-1.5 rounded-lg transition"
              >
                {t('job.retryButton', undefined, locale)}
              </button>
            </div>
          )}

          {/* Failure banner */}
          {isFailed && (
            <ErrorBanner
              testId="operation-error-banner"
              locale={locale}
              error={toErrorBannerModel(
                {
                  code: event?.error?.code ?? job.error_code ?? 'E_UNKNOWN',
                  message_key:
                    event?.error?.messageKey ??
                    `err.${event?.error?.code ?? job.error_code ?? 'E_UNKNOWN'}.body`,
                  message: job.error_detail ?? t('err.E_UNKNOWN.body', undefined, locale),
                  detail: detail.errorDetail ?? job.error_detail ?? '',
                  disk_bounds: null,
                },
                locale,
                failureActions
              )}
              onOpenDiagnostics={onNavigateDiagnostics ?? (() => undefined)}
            />
          )}

          {/* Cache hit notice */}
          {isCacheHit && (isSucceeded || isActive) && (
            <div
              data-testid="operation-cache-hit-notice"
              className="bg-cyan-950/50 border border-cyan-800/70 text-cyan-200 rounded-lg px-3 py-2"
            >
              {t('job.cacheHitNotice', undefined, locale)}
            </div>
          )}

          {/* Success summary card */}
          {isSucceeded && (
            <div
              data-testid="operation-success-card"
              className="bg-emerald-950/40 border border-emerald-700/70 rounded-lg p-3.5 space-y-2.5 text-emerald-100"
            >
              <div className="flex flex-wrap items-center justify-between gap-2">
                <strong className="text-sm text-emerald-200">
                  {t('job.successTitle', undefined, locale)}
                </strong>
                {widthRaw && heightRaw && (
                  <span className="text-xs font-mono text-emerald-200">
                    {t(
                      'job.successMeta',
                      {
                        width: widthRaw,
                        height: heightRaw,
                        fps: fpsRaw ?? '30',
                        codec: codecRaw ?? 'h264',
                        size: sizeBytesRaw > 0 ? formatBytesHuman(sizeBytesRaw) : '—',
                      },
                      locale
                    )}
                  </span>
                )}
              </div>

              {needsProxy && (
                <p data-testid="operation-proxy-note" className="text-xs text-emerald-200/90">
                  {t('job.proxyGeneratedNote', undefined, locale)}
                </p>
              )}

              <div className="flex items-center gap-2 pt-0.5">
                <button
                  type="button"
                  disabled
                  aria-disabled="true"
                  className="bg-slate-800 text-slate-400 text-xs font-medium px-3.5 py-1.5 rounded-lg cursor-not-allowed"
                >
                  {t('job.openEditor', undefined, locale)}
                </button>
                <span className="text-xs text-slate-400">
                  {t('range.comingSoon', undefined, locale)}
                </span>
              </div>
            </div>
          )}

          {/* Details disclosure & Cancel button */}
          <div className="flex flex-wrap items-center justify-between gap-3 pt-1">
            <details className="text-slate-400">
              <summary className="cursor-pointer hover:text-slate-200">
                {t('job.detailsToggle', undefined, locale)}
              </summary>
              <div className="mt-1.5 font-mono text-[11px] text-slate-300 space-y-0.5">
                <div>
                  job={job.id.slice(0, 8)} · attempt=#{job.attempt} · stage={currentStageId}
                </div>
                {detail.handleBeforeMs !== undefined && (
                  <div>
                    handles: -{detail.handleBeforeMs}ms / +{detail.handleAfterMs ?? '0'}ms
                  </div>
                )}
                {Number.isFinite(sizeBytesRaw) && sizeBytesRaw > 0 && (
                  <div>size: {formatBytesHuman(sizeBytesRaw)}</div>
                )}
              </div>
            </details>

            {isActive && !confirmingCancel && (
              <button
                type="button"
                data-testid="operation-cancel-button"
                onClick={() => setConfirmingCancel(true)}
                className="bg-slate-800 hover:bg-rose-900/70 border border-slate-700 hover:border-rose-700 text-slate-200 px-3 py-1.5 rounded-lg font-medium transition"
              >
                {t('job.cancelButton', undefined, locale)}
              </button>
            )}
          </div>

          {/* Non-resumable download cancel confirmation */}
          {isActive && confirmingCancel && (
            <div
              data-testid="cancel-confirm-dialog"
              role="alertdialog"
              aria-labelledby="cancel-confirm-title"
              className="bg-rose-950/60 border border-rose-700/80 rounded-lg p-3 space-y-2 text-rose-100"
            >
              <div id="cancel-confirm-title" className="font-semibold">
                {t('job.cancelConfirmTitle', undefined, locale)}
              </div>
              <p className="text-rose-200/90">{t('job.cancelConfirmBody', undefined, locale)}</p>
              <div className="flex flex-wrap gap-2">
                <button
                  type="button"
                  disabled={submittingCancel}
                  onClick={() => void handleConfirmCancel()}
                  className="bg-rose-600 hover:bg-rose-500 text-white font-semibold px-3 py-1.5 rounded transition"
                >
                  {t('job.cancelConfirmStop', undefined, locale)}
                </button>
                <button
                  type="button"
                  disabled={submittingCancel}
                  onClick={() => setConfirmingCancel(false)}
                  className="bg-slate-800 hover:bg-slate-700 border border-slate-600 text-slate-100 px-3 py-1.5 rounded transition"
                >
                  {t('job.cancelConfirmContinue', undefined, locale)}
                </button>
              </div>
            </div>
          )}
        </>
      )}
    </section>
  );
}
