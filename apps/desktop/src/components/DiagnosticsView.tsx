import { SupportedLocale, t } from '@clipper-helper/i18n';
import { save } from '@tauri-apps/plugin-dialog';
import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  AppCommandError,
  commands,
  CrashRecoveryStatus,
  DiagnosticsReportBundle,
  ErrorEventRow,
} from '../ipc/bindings';
import { copyTextToClipboard } from '../utils/format';
import { ErrorBanner, ErrorBannerModel, toErrorBannerModel } from './ErrorBanner';

interface DiagnosticsViewProps {
  locale: SupportedLocale;
  crashStatus: CrashRecoveryStatus | null;
  onDismissCrashBanner: () => Promise<void>;
  onNavigateToDiagnostics: () => void;
}

export function DiagnosticsView({
  locale,
  crashStatus,
  onDismissCrashBanner,
  onNavigateToDiagnostics,
}: DiagnosticsViewProps) {
  const [userNote, setUserNote] = useState<string>('');
  const [includeVideoIds, setIncludeVideoIds] = useState<boolean>(true);
  const [codeFilter, setCodeFilter] = useState<string>('');
  const [errors, setErrors] = useState<ErrorEventRow[]>([]);
  const [bundle, setBundle] = useState<DiagnosticsReportBundle | null>(null);
  const [copiedReport, setCopiedReport] = useState<boolean>(false);
  const [copiedErrorId, setCopiedErrorId] = useState<number | null>(null);
  const [saveFeedback, setSaveFeedback] = useState<string | null>(null);
  const [cmdError, setCmdError] = useState<AppCommandError | null>(null);

  const loadErrors = useCallback(async (filter: string) => {
    const res = await commands.listErrorEvents(filter || null, 100);
    if (res.status === 'ok') {
      setErrors(res.data);
    } else {
      setCmdError(res.error);
    }
  }, []);

  const refreshReportPreview = useCallback(async (note: string, includeIds: boolean) => {
    const res = await commands.generateDiagnosticsReport(note, includeIds);
    if (res.status === 'ok') {
      setBundle(res.data);
    } else {
      setCmdError(res.error);
    }
  }, []);

  useEffect(() => {
    void loadErrors(codeFilter);
  }, [codeFilter, loadErrors]);

  useEffect(() => {
    void refreshReportPreview(userNote, includeVideoIds);
  }, [userNote, includeVideoIds, refreshReportPreview]);

  const distinctCodes = useMemo(() => {
    const set = new Set<string>();
    for (const row of errors) set.add(row.code);
    return Array.from(set).sort();
  }, [errors]);

  const handleCopyReport = async () => {
    if (!bundle) return;
    await copyTextToClipboard(bundle.preview_text).catch(() => undefined);
    setCopiedReport(true);
    setTimeout(() => setCopiedReport(false), 2000);
  };

  const handleCopySingleError = async (row: ErrorEventRow) => {
    const snippet = [
      `[${row.code}] stage=${row.stage} ts=${Math.round(row.ts)}`,
      row.detail_redacted,
      row.stderr_tail_redacted ? `stderr:\n${row.stderr_tail_redacted}` : null,
    ]
      .filter(Boolean)
      .join('\n');
    await copyTextToClipboard(snippet).catch(() => undefined);
    setCopiedErrorId(row.id);
    setTimeout(() => setCopiedErrorId(null), 1500);
  };

  const handleSaveTxt = async () => {
    setSaveFeedback(null);
    setCmdError(null);
    const defaultName = `clipper-diagnostics-${bundle?.report_id ?? 'report'}.txt`;
    const dest = await save({
      defaultPath: defaultName,
      filters: [{ name: 'Text', extensions: ['txt'] }],
    });
    if (!dest) return;
    const res = await commands.saveDiagnosticsReportFile(dest, userNote, includeVideoIds);
    if (res.status === 'ok') {
      setSaveFeedback(t('diagnostics.savedTxt', { path: res.data.saved_path_redacted }, locale));
    } else {
      setCmdError(res.error);
    }
  };

  const handleOpenLogsFolder = async () => {
    setCmdError(null);
    const res = await commands.openLogsFolder();
    if (res.status === 'error') {
      setCmdError(res.error);
    }
  };

  const bannerModel: ErrorBannerModel | null = cmdError
    ? toErrorBannerModel(cmdError, locale, [{ label: 'Tutup', onClick: () => setCmdError(null) }])
    : null;

  return (
    <section className="space-y-4">
      {crashStatus?.unclean_shutdown_detected && !crashStatus.banner_dismissed && (
        <div
          data-testid="diagnostics-crash-banner"
          role="alert"
          className="bg-amber-950/70 border border-amber-700/80 rounded-lg p-3.5 text-xs flex flex-wrap items-center justify-between gap-2 text-amber-100"
        >
          <span className="font-medium">{t('crash.bannerText', undefined, locale)}</span>
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={() => void handleCopyReport()}
              className="bg-amber-600 hover:bg-amber-500 text-slate-950 font-semibold px-3 py-1 rounded transition"
            >
              {t('crash.copyReport', undefined, locale)}
            </button>
            <button
              type="button"
              onClick={() => void onDismissCrashBanner()}
              className="border border-amber-700/70 hover:bg-amber-900/50 text-amber-200 px-2.5 py-1 rounded transition"
            >
              {t('crash.dismiss', undefined, locale)}
            </button>
          </div>
        </div>
      )}

      <div className="bg-slate-900/70 border border-slate-700/70 rounded-xl p-5 space-y-4">
        <div>
          <h2 className="text-lg font-semibold text-white">
            {t('diagnostics.title', undefined, locale)}
          </h2>
          <p className="text-xs text-slate-300 mt-1">
            {t('diagnostics.subtitle', undefined, locale)}
          </p>
        </div>

        {bannerModel && (
          <ErrorBanner
            error={bannerModel}
            locale={locale}
            onOpenDiagnostics={onNavigateToDiagnostics}
          />
        )}

        {/* Free-text context input placed at the top of the generated report */}
        <div className="space-y-1.5">
          <label
            htmlFor="diagnostics-user-note"
            className="block text-xs font-medium text-slate-200"
          >
            {t('diagnostics.whatDoingLabel', undefined, locale)}
          </label>
          <textarea
            id="diagnostics-user-note"
            rows={2}
            value={userNote}
            onChange={(e) => setUserNote(e.target.value)}
            placeholder={t('diagnostics.whatDoingPlaceholder', undefined, locale)}
            className="w-full bg-slate-950 border border-slate-700 rounded-lg p-2.5 text-xs text-slate-100 focus:border-indigo-500 focus:outline-none"
          />
        </div>

        {/* Toggle video ID (11 chars only) */}
        <label className="flex items-center gap-2 text-xs text-slate-200 cursor-pointer select-none">
          <input
            type="checkbox"
            checked={includeVideoIds}
            onChange={(e) => setIncludeVideoIds(e.target.checked)}
            className="rounded border-slate-600 bg-slate-950 text-indigo-500"
          />
          <span>{t('diagnostics.includeVideoIdLabel', undefined, locale)}</span>
        </label>

        {/* Report Actions */}
        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            disabled={!bundle}
            onClick={() => void handleCopyReport()}
            className="bg-indigo-600 hover:bg-indigo-500 disabled:opacity-50 text-white text-xs font-semibold px-4 py-2 rounded-lg transition"
          >
            {copiedReport
              ? t('diagnostics.copiedReport', undefined, locale)
              : t('diagnostics.copyReport', undefined, locale)}
          </button>

          <button
            type="button"
            disabled={!bundle}
            onClick={() => void handleSaveTxt()}
            className="bg-slate-800 hover:bg-slate-700 border border-slate-600 text-slate-100 text-xs font-medium px-3.5 py-2 rounded-lg transition"
          >
            {t('diagnostics.saveTxt', undefined, locale)}
          </button>

          <button
            type="button"
            onClick={() => void handleOpenLogsFolder()}
            className="bg-slate-800 hover:bg-slate-700 border border-slate-600 text-slate-100 text-xs font-medium px-3.5 py-2 rounded-lg transition"
          >
            {t('diagnostics.openLogsFolder', undefined, locale)}
          </button>
        </div>

        {saveFeedback && (
          <div className="text-xs text-emerald-300 bg-emerald-950/50 border border-emerald-800/70 rounded p-2">
            {saveFeedback}
          </div>
        )}

        {bundle?.preview_truncated && (
          <div
            data-testid="diagnostics-truncation-notice"
            className="text-xs text-amber-200 bg-amber-950/50 border border-amber-700/70 rounded p-2.5"
          >
            {t('diagnostics.truncationNotice', undefined, locale)}
          </div>
        )}

        {/* Exact Visible Preview */}
        <div className="space-y-1.5">
          <div className="text-[11px] font-medium text-slate-400">
            {t(
              'diagnostics.previewTitle',
              {
                sizeKb: bundle ? (bundle.preview_bytes / 1024).toFixed(1) : '0.0',
                reportId: bundle?.report_id ?? '-',
              },
              locale
            )}
          </div>
          <pre
            data-testid="diagnostics-report-preview"
            className="bg-black/70 border border-slate-800 rounded-lg p-3 text-[11px] font-mono text-slate-200 max-h-64 overflow-auto whitespace-pre-wrap break-words"
          >
            {bundle?.preview_text ?? ''}
          </pre>
        </div>
      </div>

      {/* Recent Errors List */}
      <div className="bg-slate-900/70 border border-slate-700/70 rounded-xl p-5 space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h3 className="text-sm font-semibold text-white">
            {t('diagnostics.recentErrorsTitle', undefined, locale)} (
            {
              errors.filter(
                (e) => e.stage !== 'app_lifecycle' && e.code !== 'app_lifecycle/unclean_shutdown'
              ).length
            }
            )
          </h3>
          <div className="flex items-center gap-2 text-xs">
            <label htmlFor="error-code-filter" className="text-slate-400">
              {t('diagnostics.filterCodeLabel', undefined, locale)}:
            </label>
            <select
              id="error-code-filter"
              value={codeFilter}
              onChange={(e) => setCodeFilter(e.target.value)}
              className="bg-slate-950 border border-slate-700 rounded px-2 py-1 text-slate-200"
            >
              <option value="">{t('diagnostics.filterAll', undefined, locale)}</option>
              {distinctCodes.map((c) => (
                <option key={c} value={c}>
                  {c}
                </option>
              ))}
            </select>
          </div>
        </div>

        {errors.length === 0 ? (
          <div
            data-testid="diagnostics-empty-errors"
            className="text-xs text-slate-400 bg-slate-950/60 border border-slate-800 rounded-lg p-4 text-center"
          >
            {t('diagnostics.emptyErrors', undefined, locale)}
          </div>
        ) : (
          <div className="space-y-2">
            {errors.map((err) => (
              <div
                key={err.id}
                data-testid={`error-event-row-${err.id}`}
                className="bg-slate-950/60 border border-slate-800 rounded-lg p-3 text-xs flex flex-wrap items-start justify-between gap-2"
              >
                <div className="space-y-1 min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="font-mono font-semibold text-rose-300">{err.code}</span>
                    <span className="text-slate-400">stage: {err.stage}</span>
                    <span className="text-slate-500 font-mono text-[10px]">
                      {new Date(err.ts * 1000).toLocaleString()}
                    </span>
                  </div>
                  <div className="text-slate-200 break-words">{err.detail_redacted}</div>
                </div>
                <button
                  type="button"
                  onClick={() => void handleCopySingleError(err)}
                  className="bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200 px-2.5 py-1 rounded text-[11px] shrink-0 transition"
                >
                  {copiedErrorId === err.id
                    ? t('errorBanner.copiedDetail', undefined, locale)
                    : t('diagnostics.copySingleError', undefined, locale)}
                </button>
              </div>
            ))}
          </div>
        )}
      </div>
    </section>
  );
}
