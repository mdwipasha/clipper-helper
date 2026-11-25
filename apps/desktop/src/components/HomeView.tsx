import { SupportedLocale, t } from '@clipper-helper/i18n';
import { ComponentsOverview, SettingsOverview } from '../ipc/bindings';

export type NavTab = 'home' | 'components' | 'diagnostics' | 'settings';

interface HomeViewProps {
  locale: SupportedLocale;
  overview: ComponentsOverview | null;
  settings: SettingsOverview | null;
  onNavigate: (tab: NavTab) => void;
}

export function HomeView({ locale, overview, settings, onNavigate }: HomeViewProps) {
  const required = overview?.required_components ?? [];
  const installedNames = new Set(
    (overview?.diagnostics.components ?? [])
      .filter((c) => Boolean(c.current_version))
      .map((c) => c.component)
  );
  const allReady = required.length > 0 && required.every((name) => installedNames.has(name));

  return (
    <section className="space-y-4">
      <div className="bg-slate-900/70 border border-slate-700/70 rounded-xl p-5 space-y-2">
        <h2 className="text-lg font-semibold text-white">{t('home.title', undefined, locale)}</h2>
        <p className="text-sm text-slate-300 leading-relaxed">
          {t('home.subtitle', undefined, locale)}
        </p>
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
