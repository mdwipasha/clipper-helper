import { SupportedLocale, t } from '@clipper-helper/i18n';
import { MouseEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { AppCommandError, commands, PreviewConfigResponse, ProjectRecord } from '../ipc/bindings';
import { BridgeFallbackInfo, BridgePreviewState } from '../utils/bridgeClient';
import { formatDurationHms } from '../utils/format';
import {
  estimateDownloadSeconds,
  formatEstimateMinutes,
  formatTimecodeInput,
  parseTimecode,
  validateClipRange,
} from '../utils/timecode';
import { BridgePlayer, BridgePlayerControls } from './BridgePlayer';
import { ErrorBanner, toErrorBannerModel } from './ErrorBanner';

interface RangePickerViewProps {
  locale: SupportedLocale;
  project: ProjectRecord;
  previewConfig: PreviewConfigResponse;
  onBackHome: () => void;
  onOpenDiagnostics: () => void;
  onProjectUpdated: (project: ProjectRecord) => void;
}

export function RangePickerView({
  locale,
  project,
  previewConfig,
  onBackHome,
  onOpenDiagnostics,
  onProjectUpdated,
}: RangePickerViewProps) {
  const videoDuration = Math.max(1, project.duration_seconds);

  const initialStart = useMemo(() => {
    if (typeof project.range_start_ms === 'number' && project.range_start_ms >= 0) {
      return Math.min(videoDuration, project.range_start_ms / 1000);
    }
    if (
      typeof project.suggested_start_seconds === 'number' &&
      project.suggested_start_seconds >= 0
    ) {
      return Math.min(Math.max(0, videoDuration - 1), project.suggested_start_seconds);
    }
    return 0;
  }, [project.range_start_ms, project.suggested_start_seconds, videoDuration]);

  const initialEnd = useMemo(() => {
    if (typeof project.range_end_ms === 'number' && project.range_end_ms > initialStart * 1000) {
      return Math.min(videoDuration, project.range_end_ms / 1000);
    }
    const defaultWindow = Math.min(60, previewConfig.max_range_seconds);
    return Math.min(videoDuration, initialStart + defaultWindow);
  }, [initialStart, previewConfig.max_range_seconds, project.range_end_ms, videoDuration]);

  const [startInput, setStartInput] = useState<string>(() => formatTimecodeInput(initialStart));
  const [endInput, setEndInput] = useState<string>(() => formatTimecodeInput(initialEnd));
  const [currentTime, setCurrentTime] = useState<number>(initialStart);
  const [playerState, setPlayerState] = useState<BridgePreviewState>(
    project.preview_mode === 'embed' ? 'loading' : 'fallback'
  );
  const [isAdPlaying, setIsAdPlaying] = useState<boolean>(false);
  const [loopEnabled, setLoopEnabled] = useState<boolean>(false);
  const [forceTimecodeOnly, setForceTimecodeOnly] = useState<boolean>(false);
  const [fallbackInfo, setFallbackInfo] = useState<BridgeFallbackInfo | null>(() =>
    project.preview_mode === 'embed'
      ? null
      : {
          reason: 'playable_in_embed_false',
          errorCode: null,
          ytErrorCode: null,
          bridgeReadyMs: null,
          counterKind: 'fallback',
        }
  );
  const [saveError, setSaveError] = useState<AppCommandError | null>(null);
  const [savedNotice, setSavedNotice] = useState<boolean>(false);
  const [saving, setSaving] = useState<boolean>(false);

  const playerRef = useRef<BridgePlayerControls | null>(null);
  const timelineRef = useRef<HTMLDivElement | null>(null);
  const draggingHandleRef = useRef<'start' | 'end' | null>(null);
  const recordedInitialProbeModeRef = useRef<string | null>(null);

  useEffect(() => {
    setStartInput(formatTimecodeInput(initialStart));
    setEndInput(formatTimecodeInput(initialEnd));
    setCurrentTime(initialStart);
    setLoopEnabled(false);
    setSavedNotice(false);
    setSaveError(null);
    if (project.preview_mode === 'embed') {
      setPlayerState('loading');
      setFallbackInfo(null);
    } else {
      setPlayerState('fallback');
      setFallbackInfo({
        reason: 'playable_in_embed_false',
        errorCode: null,
        ytErrorCode: null,
        bridgeReadyMs: null,
        counterKind: 'fallback',
      });
    }
  }, [initialEnd, initialStart, project.id, project.preview_mode]);

  useEffect(() => {
    if (project.preview_mode !== 'embed' && recordedInitialProbeModeRef.current !== project.id) {
      recordedInitialProbeModeRef.current = project.id;
      void commands
        .recordPreviewEvent({
          mode: 'timecode_only',
          bridge_ready_ms: null,
          fallback_reason: 'playable_in_embed_false',
          yt_error_code: null,
          counter_kind: 'fallback',
          record_error_code: null,
        })
        .catch(() => undefined);
    }
  }, [project.id, project.preview_mode]);

  const parsedStart = useMemo(() => parseTimecode(startInput), [startInput]);
  const parsedEnd = useMemo(() => parseTimecode(endInput), [endInput]);

  const validation = useMemo(
    () =>
      validateClipRange({
        startSeconds: parsedStart,
        endSeconds: parsedEnd,
        videoDurationSeconds: videoDuration,
        maxRangeSeconds: previewConfig.max_range_seconds,
      }),
    [parsedEnd, parsedStart, previewConfig.max_range_seconds, videoDuration]
  );

  const isTimecodeOnlyActive = forceTimecodeOnly || fallbackInfo !== null;
  const playerControlsReady =
    !isTimecodeOnlyActive &&
    !isAdPlaying &&
    playerState !== 'loading' &&
    playerState !== 'fallback';

  const handleBridgeReady = useCallback((ms: number) => {
    void commands
      .recordPreviewEvent({
        mode: 'embed',
        bridge_ready_ms: ms,
        fallback_reason: null,
        yt_error_code: null,
        counter_kind: null,
        record_error_code: null,
      })
      .catch(() => undefined);
  }, []);

  const handleBridgeFallback = useCallback((info: BridgeFallbackInfo) => {
    setFallbackInfo(info);
    setPlayerState('fallback');
    setLoopEnabled(false);
    void commands
      .recordPreviewEvent({
        mode: 'timecode_only',
        bridge_ready_ms: info.bridgeReadyMs,
        fallback_reason: info.reason,
        yt_error_code: info.ytErrorCode,
        counter_kind: info.counterKind,
        record_error_code: info.errorCode,
      })
      .catch(() => undefined);
  }, []);

  const handleToggleForceTimecodeOnly = (checked: boolean) => {
    setForceTimecodeOnly(checked);
    setLoopEnabled(false);
    if (checked) {
      void commands
        .recordPreviewEvent({
          mode: 'timecode_only',
          bridge_ready_ms: null,
          fallback_reason: 'forced_debug',
          yt_error_code: null,
          counter_kind: 'fallback',
          record_error_code: null,
        })
        .catch(() => undefined);
    }
  };

  const applyStartSeconds = (sec: number) => {
    const clamped = Math.max(0, Math.min(videoDuration, Number(sec.toFixed(1))));
    setStartInput(formatTimecodeInput(clamped));
    setSavedNotice(false);
  };

  const applyEndSeconds = (sec: number) => {
    const clamped = Math.max(0, Math.min(videoDuration, Number(sec.toFixed(1))));
    setEndInput(formatTimecodeInput(clamped));
    setSavedNotice(false);
  };

  const nudgeStart = (deltaSeconds: number) => {
    const base = parsedStart ?? 0;
    applyStartSeconds(base + deltaSeconds);
  };

  const nudgeEnd = (deltaSeconds: number) => {
    const base = parsedEnd ?? initialEnd;
    applyEndSeconds(base + deltaSeconds);
  };

  const handleSetStartHere = () => {
    if (!playerControlsReady) return;
    applyStartSeconds(currentTime);
  };

  const handleSetEndHere = () => {
    if (!playerControlsReady) return;
    applyEndSeconds(currentTime);
  };

  const handleToggleLoopRange = () => {
    if (!validation.valid || isTimecodeOnlyActive) return;
    if (loopEnabled) {
      setLoopEnabled(false);
      playerRef.current?.pause();
    } else {
      playerRef.current?.seek(validation.startSeconds);
      playerRef.current?.play();
      setLoopEnabled(true);
    }
  };

  const handleSaveRange = async () => {
    if (!validation.valid) return;
    setSaving(true);
    setSaveError(null);
    setSavedNotice(false);
    try {
      const startMs = Math.round(validation.startSeconds * 1000);
      const endMs = Math.round(validation.endSeconds * 1000);
      const res = await commands.updateProjectRange(project.id, startMs, endMs);
      if (res.status === 'ok') {
        onProjectUpdated(res.data);
        setSavedNotice(true);
      } else {
        setSaveError(res.error);
      }
    } finally {
      setSaving(false);
    }
  };

  const handleOpenInBrowser = async () => {
    await commands.openVideoInBrowser(project.id, parsedStart ?? 0).catch(() => undefined);
  };

  const updateTimelineFromPointer = (clientX: number) => {
    const bar = timelineRef.current;
    const target = draggingHandleRef.current;
    if (!bar || !target) return;
    const rect = bar.getBoundingClientRect();
    if (rect.width <= 0) return;
    const ratio = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
    const sec = Math.round(ratio * videoDuration);
    if (target === 'start') {
      applyStartSeconds(sec);
    } else {
      applyEndSeconds(sec);
    }
  };

  const handleTimelineMouseDown =
    (handle: 'start' | 'end') => (e: MouseEvent<HTMLButtonElement>) => {
      e.preventDefault();
      draggingHandleRef.current = handle;
      const onMove = (ev: globalThis.MouseEvent) => updateTimelineFromPointer(ev.clientX);
      const onUp = () => {
        draggingHandleRef.current = null;
        window.removeEventListener('mousemove', onMove);
        window.removeEventListener('mouseup', onUp);
      };
      window.addEventListener('mousemove', onMove);
      window.addEventListener('mouseup', onUp);
    };

  const estimateSeconds = validation.valid
    ? estimateDownloadSeconds(
        validation.durationSeconds,
        previewConfig.download_prior_rt_factor,
        previewConfig.download_prior_startup_seconds
      )
    : estimateDownloadSeconds(
        0,
        previewConfig.download_prior_rt_factor,
        previewConfig.download_prior_startup_seconds
      );
  const estimateMinutes = formatEstimateMinutes(estimateSeconds);

  const startPercent =
    parsedStart !== null ? Math.max(0, Math.min(100, (parsedStart / videoDuration) * 100)) : 0;
  const endPercent =
    parsedEnd !== null
      ? Math.max(startPercent, Math.min(100, (parsedEnd / videoDuration) * 100))
      : 0;

  return (
    <section className="space-y-4" data-testid="range-picker-view">
      <div className="bg-slate-900/70 border border-slate-700/70 rounded-xl p-5 space-y-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <h2 className="text-lg font-semibold text-white">
              {t('range.title', undefined, locale)}
            </h2>
            <p className="text-xs text-slate-300 mt-0.5 break-words">
              {project.title} · {project.channel} · {formatDurationHms(project.duration_seconds)}
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-3">
            {import.meta.env.DEV && previewConfig.can_force_timecode_only && (
              <label className="flex items-center gap-1.5 text-xs text-amber-300 cursor-pointer">
                <input
                  type="checkbox"
                  checked={forceTimecodeOnly}
                  onChange={(e) => handleToggleForceTimecodeOnly(e.target.checked)}
                  className="accent-amber-500"
                />
                <span>Force timecode-only</span>
              </label>
            )}
            <button
              type="button"
              onClick={onBackHome}
              className="bg-slate-800 hover:bg-slate-700 border border-slate-600 text-slate-200 text-xs font-medium px-3 py-1.5 rounded-lg transition"
            >
              {t('range.backHome', undefined, locale)}
            </button>
          </div>
        </div>

        {/* Preview Config / Video Unavailable ErrorBanner (only for true errors, never normal mode switches) */}
        {fallbackInfo?.errorCode && (
          <ErrorBanner
            testId="preview-error-banner"
            locale={locale}
            error={toErrorBannerModel(
              {
                code: fallbackInfo.errorCode,
                message_key:
                  fallbackInfo.errorCode === 'E_YT_UNAVAILABLE'
                    ? 'err.E_YT_UNAVAILABLE.deleted'
                    : 'err.E_PREVIEW_CONFIG.body',
                message: fallbackInfo.reason,
                detail: `reason=${fallbackInfo.reason} yt_code=${fallbackInfo.ytErrorCode ?? 'none'}`,
                disk_bounds: null,
              },
              locale
            )}
            onOpenDiagnostics={onOpenDiagnostics}
          />
        )}

        {/* Active Embed Bridge vs Timecode-Only Fallback */}
        {!isTimecodeOnlyActive ? (
          <div className="space-y-3">
            <BridgePlayer
              ref={playerRef}
              bridgeUrl={previewConfig.bridge_url}
              protocolVersion={previewConfig.bridge_protocol_version}
              readyTimeoutMs={previewConfig.ready_timeout_ms}
              controlVideoId={previewConfig.control_video_id}
              videoId={project.source_video_id}
              probeDurationSeconds={project.duration_seconds}
              initialStartSeconds={initialStart}
              loopStartSeconds={validation.valid ? validation.startSeconds : null}
              loopEndSeconds={validation.valid ? validation.endSeconds : null}
              loopEnabled={loopEnabled}
              onStateChange={setPlayerState}
              onTimeChange={setCurrentTime}
              onAdStateChange={setIsAdPlaying}
              onReady={handleBridgeReady}
              onFallback={handleBridgeFallback}
            />

            {isAdPlaying && (
              <div
                data-testid="ad-notice-banner"
                role="status"
                className="bg-amber-950/70 border border-amber-700/80 text-amber-200 rounded-lg px-3 py-2 text-xs"
              >
                {t('range.adNotice', undefined, locale)}
              </div>
            )}

            {/* External transport bar (strictly outside the 16:9 YouTube player box) */}
            <div className="flex flex-wrap items-center justify-between gap-2 bg-slate-950/70 border border-slate-800 rounded-lg px-3 py-2 text-xs">
              <div className="flex flex-wrap items-center gap-2">
                <button
                  type="button"
                  onClick={() =>
                    playerState === 'playing'
                      ? playerRef.current?.pause()
                      : playerRef.current?.play()
                  }
                  disabled={playerState === 'loading'}
                  className="bg-slate-800 hover:bg-slate-700 disabled:opacity-50 text-slate-100 px-3 py-1 rounded border border-slate-600 transition"
                >
                  {playerState === 'playing'
                    ? t('range.pause', undefined, locale)
                    : t('range.play', undefined, locale)}
                </button>
                <button
                  type="button"
                  disabled={!validation.valid}
                  onClick={handleToggleLoopRange}
                  className={`px-3 py-1 rounded font-medium border transition ${
                    loopEnabled
                      ? 'bg-emerald-700 hover:bg-emerald-600 border-emerald-500 text-white'
                      : 'bg-indigo-600 hover:bg-indigo-500 border-indigo-500 text-white disabled:opacity-50'
                  }`}
                >
                  {loopEnabled
                    ? t('range.stopLoop', undefined, locale)
                    : t('range.playRange', undefined, locale)}
                </button>
              </div>
              <div className="font-mono text-slate-300" data-testid="current-playback-time">
                {t('range.currentTime', undefined, locale)}: {formatTimecodeInput(currentTime)}
              </div>
            </div>
          </div>
        ) : (
          <div
            data-testid="timecode-only-panel"
            className="bg-slate-950/80 border border-slate-700 rounded-xl p-4 space-y-2.5 text-xs"
          >
            <div className="font-semibold text-sm text-amber-200">
              {t('range.timecodeOnlyBanner', undefined, locale)}
            </div>
            <p className="text-slate-300">
              {fallbackInfo?.reason === 'ready_timeout'
                ? t('range.timecodeOnlyTimeoutHint', undefined, locale)
                : t('range.timecodeOnlyOwnerHint', undefined, locale)}
            </p>
            <div>
              <button
                type="button"
                onClick={() => void handleOpenInBrowser()}
                className="bg-slate-800 hover:bg-slate-700 border border-slate-600 text-slate-100 font-medium px-3 py-1.5 rounded-lg transition"
              >
                {t('range.openInBrowser', undefined, locale)}
              </button>
            </div>
          </div>
        )}

        {/* Simple custom timeline bar (no thumbnails) */}
        <div className="space-y-1.5 pt-1">
          <div
            ref={timelineRef}
            data-testid="range-timeline-bar"
            className="relative h-4 bg-slate-800 rounded-full border border-slate-700"
          >
            <div
              className="absolute top-0 bottom-0 bg-indigo-500/50 border-x border-indigo-400"
              style={{
                left: `${startPercent}%`,
                width: `${Math.max(1, endPercent - startPercent)}%`,
              }}
            />
            <button
              type="button"
              aria-label="Timeline start handle"
              onMouseDown={handleTimelineMouseDown('start')}
              className="absolute top-1/2 -translate-y-1/2 -translate-x-1/2 w-3.5 h-5 rounded bg-indigo-400 hover:bg-indigo-300 border border-slate-950 cursor-ew-resize"
              style={{ left: `${startPercent}%` }}
            />
            <button
              type="button"
              aria-label="Timeline end handle"
              onMouseDown={handleTimelineMouseDown('end')}
              className="absolute top-1/2 -translate-y-1/2 -translate-x-1/2 w-3.5 h-5 rounded bg-emerald-400 hover:bg-emerald-300 border border-slate-950 cursor-ew-resize"
              style={{ left: `${endPercent}%` }}
            />
          </div>
          <div className="flex justify-between text-[11px] font-mono text-slate-400">
            <span>00:00:00</span>
            <span>{formatTimecodeInput(videoDuration)}</span>
          </div>
        </div>

        {/* Start & End Timecode Controls + Nudges */}
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          <div className="bg-slate-950/70 border border-slate-800 rounded-lg p-3.5 space-y-2.5">
            <label htmlFor="range-start-input" className="block text-xs font-medium text-slate-300">
              {t('range.startLabel', undefined, locale)}
            </label>
            <div className="flex gap-2">
              <input
                id="range-start-input"
                type="text"
                value={startInput}
                onChange={(e) => {
                  setStartInput(e.target.value);
                  setSavedNotice(false);
                }}
                className="flex-1 bg-slate-900 border border-slate-700 rounded px-2.5 py-1.5 text-sm font-mono text-white outline-none focus:border-indigo-500"
              />
              <button
                type="button"
                disabled={!playerControlsReady}
                onClick={handleSetStartHere}
                className="bg-indigo-600 hover:bg-indigo-500 disabled:opacity-45 disabled:cursor-not-allowed text-white text-xs font-medium px-3 py-1.5 rounded transition"
              >
                {t('range.setStartHere', undefined, locale)}
              </button>
            </div>
            <div className="flex flex-wrap gap-1.5 text-xs">
              {[-5, -1, 1, 5].map((d) => (
                <button
                  key={`start-${d}`}
                  type="button"
                  aria-label={`Mulai ${d > 0 ? `+${d}` : d} detik`}
                  onClick={() => nudgeStart(d)}
                  className="bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200 px-2.5 py-1 rounded font-mono"
                >
                  {d > 0 ? `+${d}s` : `${d}s`}
                </button>
              ))}
            </div>
          </div>

          <div className="bg-slate-950/70 border border-slate-800 rounded-lg p-3.5 space-y-2.5">
            <label htmlFor="range-end-input" className="block text-xs font-medium text-slate-300">
              {t('range.endLabel', undefined, locale)}
            </label>
            <div className="flex gap-2">
              <input
                id="range-end-input"
                type="text"
                value={endInput}
                onChange={(e) => {
                  setEndInput(e.target.value);
                  setSavedNotice(false);
                }}
                className="flex-1 bg-slate-900 border border-slate-700 rounded px-2.5 py-1.5 text-sm font-mono text-white outline-none focus:border-indigo-500"
              />
              <button
                type="button"
                disabled={!playerControlsReady}
                onClick={handleSetEndHere}
                className="bg-emerald-600 hover:bg-emerald-500 disabled:opacity-45 disabled:cursor-not-allowed text-white text-xs font-medium px-3 py-1.5 rounded transition"
              >
                {t('range.setEndHere', undefined, locale)}
              </button>
            </div>
            <div className="flex flex-wrap gap-1.5 text-xs">
              {[-5, -1, 1, 5].map((d) => (
                <button
                  key={`end-${d}`}
                  type="button"
                  aria-label={`Akhir ${d > 0 ? `+${d}` : d} detik`}
                  onClick={() => nudgeEnd(d)}
                  className="bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200 px-2.5 py-1 rounded font-mono"
                >
                  {d > 0 ? `+${d}s` : `${d}s`}
                </button>
              ))}
            </div>
          </div>
        </div>

        {/* Duration Summary, Live Download ETA & Validation Errors */}
        <div className="bg-slate-950/60 border border-slate-800 rounded-lg p-3.5 space-y-2 text-xs">
          {validation.valid ? (
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span className="text-slate-200">
                {t('range.clipDurationLabel', undefined, locale)}:{' '}
                <strong className="font-mono text-white">
                  {formatDurationHms(validation.durationSeconds)} ({validation.durationSeconds} dtk)
                </strong>
              </span>
              <span data-testid="download-estimate-text" className="text-cyan-300 font-medium">
                {t(
                  'range.downloadEstimate',
                  {
                    minutes: estimateMinutes,
                    seconds: Math.round(estimateSeconds),
                  },
                  locale
                )}
              </span>
            </div>
          ) : (
            <div
              data-testid="range-validation-error"
              role="alert"
              className="text-rose-300 font-medium"
            >
              [{validation.code}]{' '}
              {t(
                validation.reasonKey,
                {
                  duration: formatTimecodeInput(videoDuration),
                  maxSeconds: previewConfig.max_range_seconds,
                  maxMinutes: Math.round(previewConfig.max_range_seconds / 60),
                },
                locale
              )}
            </div>
          )}
        </div>

        {saveError && (
          <ErrorBanner
            locale={locale}
            error={toErrorBannerModel(saveError, locale)}
            onOpenDiagnostics={onOpenDiagnostics}
          />
        )}

        {savedNotice && (
          <div
            data-testid="range-saved-notice"
            className="bg-emerald-950/60 border border-emerald-700 text-emerald-200 rounded-lg px-3 py-2 text-xs"
          >
            {t('range.savedStatus', undefined, locale)}
          </div>
        )}

        {/* Footer Actions */}
        <div className="flex flex-wrap items-center gap-3 pt-1">
          <button
            type="button"
            disabled={!validation.valid || saving}
            onClick={() => void handleSaveRange()}
            className="bg-indigo-600 hover:bg-indigo-500 disabled:opacity-45 disabled:cursor-not-allowed text-white text-xs font-semibold px-4 py-2 rounded-lg transition"
          >
            {t('range.saveRange', undefined, locale)}
          </button>

          <button
            type="button"
            disabled
            aria-disabled="true"
            className="bg-slate-800 text-slate-400 text-xs font-medium px-4 py-2 rounded-lg cursor-not-allowed"
          >
            {t('range.nextDownload', undefined, locale)}
          </button>
          <span className="text-xs text-slate-400">{t('range.comingSoon', undefined, locale)}</span>
        </div>
      </div>
    </section>
  );
}
