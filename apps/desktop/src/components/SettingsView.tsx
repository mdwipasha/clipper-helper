import { SupportedLocale, t } from '@clipper-helper/i18n';
import { open } from '@tauri-apps/plugin-dialog';
import { useState } from 'react';
import {
  AppCommandError,
  commands,
  DataRootChangeOutcome,
  SettingsOverview,
  SpeechModel,
} from '../ipc/bindings';
import { formatBytesHuman } from '../utils/format';
import { ErrorBanner, ErrorBannerModel, toErrorBannerModel } from './ErrorBanner';

interface SettingsViewProps {
  locale: SupportedLocale;
  settings: SettingsOverview | null;
  onUpdated: (next: SettingsOverview) => void;
  onOpenDiagnostics: () => void;
}

const SPEED_HINT_LABELS: Record<string, string> = {
  very_fast: 'Sangat cepat (~0.8–1.8×)',
  fast: 'Cepat (~2.5–4×)',
  balanced: 'Seimbang (~1.1–2.2×)',
  slower: 'Lebih lambat',
  heavy: 'Berat (akurasi tinggi)',
};

export function SettingsView({
  locale,
  settings,
  onUpdated,
  onOpenDiagnostics,
}: SettingsViewProps) {
  const [busy, setBusy] = useState(false);
  const [cmdError, setCmdError] = useState<AppCommandError | null>(null);
  const [dataRootOutcome, setDataRootOutcome] = useState<DataRootChangeOutcome | null>(null);

  const patchSettings = async (patch: {
    language?: string | null;
    speech_model_selection?: string | null;
    dismiss_speech_downgrade?: boolean | null;
  }) => {
    setBusy(true);
    setCmdError(null);
    try {
      const res = await commands.updateSettings({
        language: patch.language ?? null,
        speech_model_selection: patch.speech_model_selection ?? null,
        dismiss_speech_downgrade: patch.dismiss_speech_downgrade ?? null,
      });
      if (res.status === 'ok') {
        onUpdated(res.data);
      } else {
        setCmdError(res.error);
      }
    } finally {
      setBusy(false);
    }
  };

  const handleChooseDataRoot = async () => {
    const picked = await open({
      directory: true,
      multiple: false,
      title: t('components.chooseAnotherLocation', undefined, locale),
    });
    if (!picked || typeof picked !== 'string') return;
    const res = await commands.selectCustomDataRoot(picked);
    if (res.status === 'ok') {
      setDataRootOutcome(res.data);
    } else {
      setCmdError(res.error);
    }
  };

  const errBanner: ErrorBannerModel | null = cmdError
    ? toErrorBannerModel(cmdError, locale, [{ label: 'Tutup', onClick: () => setCmdError(null) }])
    : null;

  const speechState = settings?.speech_state;
  const encoderStatus = settings?.encoder_status;

  return (
    <section className="space-y-4">
      <div className="bg-slate-900/70 border border-slate-700/70 rounded-xl p-5 space-y-1">
        <h2 className="text-lg font-semibold text-white">
          {t('settings.title', undefined, locale)}
        </h2>
        <p className="text-xs text-slate-300">{t('settings.subtitle', undefined, locale)}</p>
      </div>

      {errBanner && (
        <ErrorBanner error={errBanner} locale={locale} onOpenDiagnostics={onOpenDiagnostics} />
      )}

      {/* Non-blocking RTF downgrade suggestion */}
      {speechState?.downgrade_suggested && (
        <div
          data-testid="speech-downgrade-suggestion"
          className="bg-amber-950/60 border border-amber-700/80 rounded-lg p-4 text-xs text-amber-100 space-y-2"
        >
          <div className="font-semibold text-amber-300">
            {t('settings.speechDowngradeTitle', undefined, locale)}
          </div>
          <p>{t('settings.speechDowngradeBody', undefined, locale)}</p>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              disabled={busy}
              onClick={() => void patchSettings({ speech_model_selection: 'base' })}
              className="bg-amber-600 hover:bg-amber-500 text-slate-950 font-semibold px-3 py-1.5 rounded transition"
            >
              {t('settings.switchToBase', undefined, locale)}
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() => void patchSettings({ dismiss_speech_downgrade: true })}
              className="border border-amber-700/80 text-amber-200 px-3 py-1.5 rounded transition"
            >
              {t('settings.dismiss', undefined, locale)}
            </button>
          </div>
        </div>
      )}

      {/* Language Switcher */}
      <div className="bg-slate-900/70 border border-slate-700/70 rounded-xl p-5 text-xs space-y-2">
        <label htmlFor="settings-language" className="block text-sm font-semibold text-white">
          {t('settings.languageLabel', undefined, locale)}
        </label>
        <select
          id="settings-language"
          disabled={busy}
          value={settings?.language ?? locale}
          onChange={(e) => void patchSettings({ language: e.target.value })}
          className="bg-slate-950 border border-slate-700 rounded-lg px-3 py-2 text-slate-100"
        >
          <option value="id">Bahasa Indonesia (Default)</option>
          <option value="en">English</option>
        </select>
      </div>

      {/* Speech Model Override & RAM-aware safe default */}
      <div className="bg-slate-900/70 border border-slate-700/70 rounded-xl p-5 text-xs space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h3 className="text-sm font-semibold text-white">
            {t('settings.speechTitle', undefined, locale)}
          </h3>
          {settings?.hardware.ram_bytes && (
            <span className="text-slate-400">
              RAM terdeteksi: {formatBytesHuman(settings.hardware.ram_bytes)}
            </span>
          )}
        </div>

        <select
          aria-label={t('settings.speechTitle', undefined, locale)}
          disabled={busy || !speechState}
          value={speechState?.is_user_override ? (speechState.active_model as SpeechModel) : 'auto'}
          onChange={(e) => void patchSettings({ speech_model_selection: e.target.value })}
          className="w-full max-w-md bg-slate-950 border border-slate-700 rounded-lg px-3 py-2 text-slate-100"
        >
          <option value="auto">
            {t('settings.speechAuto', { model: speechState?.recommended_model ?? 'base' }, locale)}
          </option>
          {(speechState?.options ?? []).map((opt) => (
            <option key={opt.id} value={opt.id}>
              {opt.id} · min RAM {opt.min_ram_gb} GB ·{' '}
              {SPEED_HINT_LABELS[opt.speed_tier] ?? opt.speed_tier}
            </option>
          ))}
        </select>
      </div>

      {/* Encoder Lazy Chain Status */}
      <div className="bg-slate-900/70 border border-slate-700/70 rounded-xl p-5 text-xs space-y-2">
        <h3 className="text-sm font-semibold text-white">
          {t('settings.encoderTitle', undefined, locale)}
        </h3>
        <div className="text-slate-300">
          Encoder aktif:{' '}
          <strong className="text-indigo-300">
            {encoderStatus?.cached_encoder ?? t('settings.encoderUntested', undefined, locale)}
          </strong>
        </div>
        <div className="text-slate-400">
          Urutan fallback: {(encoderStatus?.chain_order ?? []).join(' → ')}
        </div>
        {encoderStatus && (
          <div className="text-slate-500 font-mono text-[11px]">
            Hardware fingerprint: {encoderStatus.current_fingerprint.slice(0, 12)}… (
            {encoderStatus.fingerprint_matches ? 'terverifikasi' : 'akan diuji saat export'})
          </div>
        )}
        {encoderStatus?.is_windows_n_kn && (
          <div className="text-amber-300">
            Windows edisi N/KN terdeteksi: pasang Media Feature Pack jika encoder Media Foundation
            diperlukan.
          </div>
        )}
      </div>

      {/* DataRoot Location */}
      <div className="bg-slate-900/70 border border-slate-700/70 rounded-xl p-5 text-xs space-y-2.5">
        <h3 className="text-sm font-semibold text-white">
          {t('settings.dataLocationTitle', undefined, locale)}
        </h3>
        <div className="font-mono text-slate-200">{settings?.data_root_redacted ?? '-'}</div>
        {settings?.is_onedrive && (
          <div className="text-amber-300">{t('components.oneDriveWarning', undefined, locale)}</div>
        )}
        <button
          type="button"
          onClick={() => void handleChooseDataRoot()}
          className="bg-slate-800 hover:bg-slate-700 border border-slate-600 text-slate-200 px-3 py-1.5 rounded text-xs transition"
        >
          {t('components.chooseAnotherLocation', undefined, locale)}
        </button>
        {dataRootOutcome && (
          <div className="text-indigo-200 bg-indigo-950/50 border border-indigo-700/70 rounded p-2.5">
            {t('components.restartWarning', { path: dataRootOutcome.new_root_redacted }, locale)}
          </div>
        )}
      </div>
    </section>
  );
}
