import { SupportedLocale, t } from '@clipper-helper/i18n';
import { useState } from 'react';
import type { AppCommandError } from '../ipc/bindings';
import { copyTextToClipboard } from '../utils/format';

export interface ErrorBannerAction {
  label: string;
  onClick: () => void;
  variant?: 'primary' | 'secondary';
}

export interface ErrorBannerModel {
  code: string;
  title?: string;
  message: string;
  redactedDetail?: string;
  actions?: ErrorBannerAction[];
}

export function resolveFriendlyErrorBody(err: AppCommandError, locale: SupportedLocale): string {
  const translated = t(err.message_key, undefined, locale);
  return translated !== err.message_key ? translated : err.message;
}

export function toErrorBannerModel(
  err: AppCommandError,
  locale: SupportedLocale,
  actions?: ErrorBannerAction[]
): ErrorBannerModel {
  return {
    code: err.code,
    message: resolveFriendlyErrorBody(err, locale),
    redactedDetail: err.detail || err.message,
    actions,
  };
}

export interface ErrorBannerProps {
  error: ErrorBannerModel;
  locale?: SupportedLocale;
  onOpenDiagnostics: () => void;
  onCopyDetail?: (copiedText: string) => Promise<void> | void;
  testId?: string;
}

export function ErrorBanner({
  error,
  locale = 'id',
  onOpenDiagnostics,
  onCopyDetail,
  testId = 'shared-error-banner',
}: ErrorBannerProps) {
  const [copied, setCopied] = useState(false);
  const [techExpanded, setTechExpanded] = useState(false);

  const resolvedTitle =
    error.title ??
    (() => {
      const key = `err.${error.code}.title`;
      const translated = t(key, undefined, locale);
      return translated === key ? t('err.E_UNKNOWN.title', undefined, locale) : translated;
    })();

  const technicalText = error.redactedDetail?.trim() ? error.redactedDetail.trim() : null;

  const singleErrorPayload = [
    `[${error.code}] ${resolvedTitle}`,
    error.message,
    technicalText && technicalText !== error.message ? technicalText : null,
  ]
    .filter(Boolean)
    .join('\n');

  const handleCopy = async () => {
    if (onCopyDetail) {
      await onCopyDetail(singleErrorPayload);
    } else {
      await copyTextToClipboard(singleErrorPayload).catch(() => undefined);
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div
      data-testid={testId}
      role="alert"
      className="bg-rose-950/60 border border-rose-800/80 rounded-lg p-4 text-xs space-y-2.5 text-rose-100"
    >
      <div className="flex items-center justify-between gap-2">
        <span className="font-semibold text-sm text-rose-300">{resolvedTitle}</span>
        <span className="font-mono text-[10px] bg-rose-900/70 text-rose-200 px-2 py-0.5 rounded border border-rose-700/60">
          {error.code}
        </span>
      </div>

      <p className="text-rose-200/95 leading-relaxed">{error.message}</p>

      <div className="flex flex-wrap items-center gap-2 pt-0.5">
        {error.actions?.map((act) => (
          <button
            key={act.label}
            type="button"
            onClick={act.onClick}
            className={
              act.variant === 'primary'
                ? 'bg-rose-700 hover:bg-rose-600 text-white font-medium px-3 py-1.5 rounded text-xs transition'
                : 'bg-slate-800 hover:bg-slate-700 text-slate-100 border border-slate-600 font-medium px-3 py-1.5 rounded text-xs transition'
            }
          >
            {act.label}
          </button>
        ))}

        <button
          type="button"
          onClick={() => void handleCopy()}
          className="bg-slate-900/90 hover:bg-slate-800 text-slate-200 border border-slate-700 px-3 py-1.5 rounded text-xs transition"
        >
          {copied
            ? t('errorBanner.copiedDetail', undefined, locale)
            : t('errorBanner.copyDetail', undefined, locale)}
        </button>

        {technicalText && (
          <button
            type="button"
            aria-expanded={techExpanded}
            onClick={() => setTechExpanded((prev) => !prev)}
            className="bg-slate-900/60 hover:bg-slate-800 text-slate-300 border border-slate-700 px-3 py-1.5 rounded text-xs transition"
          >
            {techExpanded
              ? t('errorBanner.hideTechnicalDetail', undefined, locale)
              : t('errorBanner.technicalDetail', undefined, locale)}
          </button>
        )}

        <button
          type="button"
          onClick={onOpenDiagnostics}
          className="text-indigo-300 hover:text-indigo-200 underline underline-offset-2 px-2 py-1 text-xs transition"
        >
          {t('errorBanner.openDiagnostics', undefined, locale)}
        </button>
      </div>

      {techExpanded && technicalText && (
        <pre
          data-testid={`${testId}-technical-detail`}
          className="bg-black/60 border border-rose-900/70 rounded p-2.5 text-[11px] font-mono text-rose-200/90 whitespace-pre-wrap break-words max-h-40 overflow-auto"
        >
          {technicalText}
        </pre>
      )}
    </div>
  );
}
