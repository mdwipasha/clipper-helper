import { SupportedLocale, t } from '@clipper-helper/i18n';
import { MouseEvent as ReactMouseEvent, useEffect, useRef, useState } from 'react';
import { formatFrameTimecodeMs } from '../editor/frameMath';
import { ThumbnailProgressEvent } from '../ipc/bindings';
import { formatEtaDisplay } from '../utils/etaFormat';

export type FilmstripStatus = 'idle' | 'generating' | 'ready' | 'cancelled' | 'error';

export interface EditorTimelineProps {
  locale: SupportedLocale;
  sourceDurationMs: number;
  handleBeforeMs: number;
  handleAfterMs: number;
  inMs: number;
  outMs: number;
  playheadMs: number;
  maxClipSeconds: number;
  filmstripFrames: string[];
  filmstripStatus: FilmstripStatus;
  filmstripProgress: ThumbnailProgressEvent | null;
  onSeek: (ms: number) => void;
  onDragTrim: (nextInMs: number, nextOutMs: number, coalesceKey?: string) => void;
  onSetInAtPlayhead: () => void;
  onSetOutAtPlayhead: () => void;
  onNudgeFrame: (edge: 'in' | 'out', deltaFrames: number) => void;
  onNudgeSecond: (edge: 'in' | 'out', deltaSeconds: number) => void;
  onCancelFilmstrip: () => void;
  onRetryFilmstrip: () => void;
}

type DragTarget = 'playhead' | 'in' | 'out' | null;

export function EditorTimeline({
  locale,
  sourceDurationMs,
  handleBeforeMs,
  handleAfterMs,
  inMs,
  outMs,
  playheadMs,
  maxClipSeconds,
  filmstripFrames,
  filmstripStatus,
  filmstripProgress,
  onSeek,
  onDragTrim,
  onSetInAtPlayhead,
  onSetOutAtPlayhead,
  onNudgeFrame,
  onNudgeSecond,
  onCancelFilmstrip,
  onRetryFilmstrip,
}: EditorTimelineProps) {
  const trackRef = useRef<HTMLDivElement | null>(null);
  const [dragTarget, setDragTarget] = useState<DragTarget>(null);
  const inMsRef = useRef(inMs);
  const outMsRef = useRef(outMs);
  inMsRef.current = inMs;
  outMsRef.current = outMs;

  const totalMs = Math.max(1, sourceDurationMs);
  const selectedMs = Math.max(0, outMs - inMs);
  const exceedsMaxClip = selectedMs > maxClipSeconds * 1000;

  const toPct = (ms: number): number => Math.min(100, Math.max(0, (ms / totalMs) * 100));
  const inPct = toPct(inMs);
  const outPct = toPct(outMs);
  const playheadPct = toPct(playheadMs);
  const handleBeforePct = toPct(handleBeforeMs);
  const handleAfterStartPct = toPct(Math.max(0, totalMs - handleAfterMs));
  const placeholderCount = Math.min(24, Math.max(6, Math.round(totalMs / 2000)));

  const clientXToMs = (clientX: number): number => {
    const el = trackRef.current;
    if (!el) return playheadMs;
    const rect = el.getBoundingClientRect();
    const width = Math.max(1, rect.width);
    const ratio = Math.min(1, Math.max(0, (clientX - rect.left) / width));
    return Math.round(ratio * totalMs);
  };

  useEffect(() => {
    if (!dragTarget) return undefined;

    const handleMove = (ev: MouseEvent) => {
      const targetMs = clientXToMs(ev.clientX);
      if (dragTarget === 'playhead') {
        onSeek(targetMs);
      } else if (dragTarget === 'in') {
        onDragTrim(targetMs, outMsRef.current, 'timeline-drag-in');
      } else if (dragTarget === 'out') {
        onDragTrim(inMsRef.current, targetMs, 'timeline-drag-out');
      }
    };

    const handleUp = () => {
      setDragTarget(null);
    };

    window.addEventListener('mousemove', handleMove);
    window.addEventListener('mouseup', handleUp);
    return () => {
      window.removeEventListener('mousemove', handleMove);
      window.removeEventListener('mouseup', handleUp);
    };
  });

  const handleTrackMouseDown = (ev: ReactMouseEvent<HTMLDivElement>) => {
    if (ev.button !== 0) return;
    ev.preventDefault();
    setDragTarget('playhead');
    onSeek(clientXToMs(ev.clientX));
  };

  const etaLabel =
    filmstripProgress && filmstripProgress.eta_seconds !== null
      ? formatEtaDisplay(
          {
            seconds: filmstripProgress.eta_seconds,
            lowSeconds: filmstripProgress.eta_seconds,
            highSeconds: filmstripProgress.eta_seconds,
            confidence: 'medium',
          },
          locale,
          false
        )
      : t('job.etaCalculating', undefined, locale);

  return (
    <section className="bg-slate-900/90 border border-slate-700/80 rounded-xl p-4 space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2 text-xs">
        <span className="font-semibold text-slate-200">
          {t('editor.trim.title', undefined, locale)}
        </span>
        <div className="flex flex-wrap items-center gap-3 font-mono text-[11px] text-slate-300">
          <span>
            {t('editor.trim.playheadLabel', undefined, locale)}:{' '}
            <strong data-testid="editor-playhead-time" className="text-cyan-300">
              {formatFrameTimecodeMs(playheadMs)}
            </strong>
          </span>
          <span>
            {t('editor.trim.inLabel', undefined, locale)}:{' '}
            <strong data-testid="editor-in-time" className="text-emerald-300">
              {formatFrameTimecodeMs(inMs)}
            </strong>
          </span>
          <span>
            {t('editor.trim.outLabel', undefined, locale)}:{' '}
            <strong data-testid="editor-out-time" className="text-emerald-300">
              {formatFrameTimecodeMs(outMs)}
            </strong>
          </span>
          <span>
            {t('editor.trim.selectedLabel', undefined, locale)}:{' '}
            <strong
              data-testid="editor-selected-duration"
              className={exceedsMaxClip ? 'text-amber-300' : 'text-white'}
            >
              {formatFrameTimecodeMs(selectedMs)}
            </strong>
          </span>
          <span>
            {t('editor.trim.totalLabel', undefined, locale)}:{' '}
            <strong data-testid="editor-total-duration" className="text-slate-400">
              {formatFrameTimecodeMs(sourceDurationMs)}
            </strong>
          </span>
        </div>
      </div>

      {exceedsMaxClip && (
        <div
          data-testid="editor-max-clip-warning"
          role="alert"
          className="bg-amber-950/60 border border-amber-700 text-amber-200 rounded-lg px-3 py-1.5 text-xs"
        >
          {t('editor.trim.maxWarning', { maxSeconds: maxClipSeconds }, locale)}
        </div>
      )}

      {/* Filmstrip status bar */}
      <div className="flex flex-wrap items-center justify-between gap-2 text-[11px] text-slate-400">
        <span>{t('editor.trim.handleLegend', undefined, locale)}</span>
        <div className="flex items-center gap-2">
          {filmstripStatus === 'generating' && (
            <>
              <span data-testid="filmstrip-progress-label" className="text-indigo-300">
                {filmstripProgress
                  ? t(
                      'editor.filmstrip.generating',
                      { percent: Math.round(filmstripProgress.percent), eta: etaLabel },
                      locale
                    )
                  : t('editor.filmstrip.preparing', undefined, locale)}
              </span>
              <button
                type="button"
                data-testid="filmstrip-cancel-button"
                onClick={onCancelFilmstrip}
                className="px-2 py-0.5 rounded bg-slate-800 hover:bg-slate-700 border border-slate-600 text-slate-200 text-[11px]"
              >
                {t('editor.filmstrip.cancel', undefined, locale)}
              </button>
            </>
          )}
          {(filmstripStatus === 'cancelled' || filmstripStatus === 'error') && (
            <>
              <span data-testid="filmstrip-status-note">
                {filmstripStatus === 'cancelled'
                  ? t('editor.filmstrip.cancelled', undefined, locale)
                  : t('editor.filmstrip.unavailable', undefined, locale)}
              </span>
              <button
                type="button"
                data-testid="filmstrip-retry-button"
                onClick={onRetryFilmstrip}
                className="px-2 py-0.5 rounded bg-slate-800 hover:bg-slate-700 border border-slate-600 text-slate-200 text-[11px]"
              >
                {t('editor.filmstrip.retry', undefined, locale)}
              </button>
            </>
          )}
        </div>
      </div>

      {/* Interactive Timeline Track + Filmstrip + Handles + Playhead */}
      <div
        ref={trackRef}
        data-testid="editor-timeline-track"
        role="slider"
        aria-label={t('editor.trim.scrubAria', undefined, locale)}
        aria-valuemin={0}
        aria-valuemax={sourceDurationMs}
        aria-valuenow={Math.round(playheadMs)}
        tabIndex={0}
        onMouseDown={handleTrackMouseDown}
        className="relative h-16 rounded-lg border border-slate-700 bg-slate-950 overflow-hidden select-none cursor-pointer"
      >
        {/* Thumbnail filmstrip background or placeholders */}
        <div
          data-testid="editor-filmstrip"
          className="absolute inset-0 flex items-stretch pointer-events-none"
        >
          {filmstripFrames.length > 0
            ? filmstripFrames.map((src, idx) => (
                <img
                  key={`${src}-${idx}`}
                  src={src}
                  alt=""
                  data-testid="filmstrip-thumb-img"
                  className="h-full flex-1 min-w-0 object-cover opacity-80"
                />
              ))
            : Array.from({ length: placeholderCount }).map((_, idx) => (
                <div
                  key={idx}
                  data-testid="filmstrip-placeholder"
                  className={`flex-1 border-r border-slate-900 bg-slate-900/70 ${
                    filmstripStatus === 'generating' ? 'animate-pulse' : ''
                  }`}
                />
              ))}
        </div>

        {/* Handle margin shading before & after */}
        {handleBeforePct > 0 && (
          <div
            data-testid="timeline-handle-before"
            style={{ width: `${handleBeforePct}%` }}
            className="absolute inset-y-0 left-0 bg-amber-950/45 border-r border-dashed border-amber-500/60 pointer-events-none"
          />
        )}
        {handleAfterStartPct < 100 && (
          <div
            data-testid="timeline-handle-after"
            style={{ left: `${handleAfterStartPct}%`, right: 0 }}
            className="absolute inset-y-0 bg-amber-950/45 border-l border-dashed border-amber-500/60 pointer-events-none"
          />
        )}

        {/* Selected active clip range */}
        <div
          data-testid="timeline-selection-bar"
          style={{
            left: `${inPct}%`,
            width: `${Math.max(0.5, outPct - inPct)}%`,
          }}
          className="absolute inset-y-0 border-y-2 border-emerald-400 bg-emerald-500/15 pointer-events-none"
        />

        {/* In Trim Handle */}
        <div
          data-testid="timeline-handle-in"
          role="slider"
          aria-label={t('editor.trim.handleInAria', undefined, locale)}
          aria-valuemin={0}
          aria-valuemax={outMs}
          aria-valuenow={Math.round(inMs)}
          tabIndex={0}
          style={{ left: `${inPct}%` }}
          onMouseDown={(ev) => {
            ev.stopPropagation();
            ev.preventDefault();
            setDragTarget('in');
          }}
          className="absolute inset-y-0 -ml-1.5 w-3 bg-emerald-400 hover:bg-emerald-300 cursor-ew-resize z-10 flex items-center justify-center rounded-l shadow"
        >
          <span className="w-0.5 h-6 bg-slate-950 rounded" />
        </div>

        {/* Out Trim Handle */}
        <div
          data-testid="timeline-handle-out"
          role="slider"
          aria-label={t('editor.trim.handleOutAria', undefined, locale)}
          aria-valuemin={inMs}
          aria-valuemax={sourceDurationMs}
          aria-valuenow={Math.round(outMs)}
          tabIndex={0}
          style={{ left: `${outPct}%` }}
          onMouseDown={(ev) => {
            ev.stopPropagation();
            ev.preventDefault();
            setDragTarget('out');
          }}
          className="absolute inset-y-0 -ml-1.5 w-3 bg-emerald-400 hover:bg-emerald-300 cursor-ew-resize z-10 flex items-center justify-center rounded-r shadow"
        >
          <span className="w-0.5 h-6 bg-slate-950 rounded" />
        </div>

        {/* Playhead Needle */}
        <div
          data-testid="timeline-playhead"
          style={{ left: `${playheadPct}%` }}
          className="absolute inset-y-0 -ml-0.5 w-0.5 bg-cyan-300 z-20 pointer-events-none shadow-[0_0_8px_rgba(34,211,238,0.8)]"
        >
          <div className="w-2.5 h-2.5 -ml-1 bg-cyan-300 rotate-45 -mt-1" />
        </div>
      </div>

      {/* Trim buttons & frame nudge controls */}
      <div className="flex flex-wrap items-center justify-between gap-2 text-xs">
        <div className="flex flex-wrap items-center gap-1.5">
          <button
            type="button"
            data-testid="btn-set-in"
            onClick={onSetInAtPlayhead}
            className="px-2.5 py-1 rounded bg-slate-800 hover:bg-slate-700 border border-slate-600 text-emerald-300 font-medium"
          >
            {t('editor.trim.setIn', undefined, locale)}
          </button>
          <button
            type="button"
            data-testid="nudge-in-minus-sec"
            onClick={() => onNudgeSecond('in', -1)}
            className="px-2 py-1 rounded bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200 font-mono"
          >
            In −1s
          </button>
          <button
            type="button"
            data-testid="nudge-in-minus-frame"
            onClick={() => onNudgeFrame('in', -1)}
            className="px-2 py-1 rounded bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200 font-mono"
          >
            In −1f
          </button>
          <button
            type="button"
            data-testid="nudge-in-plus-frame"
            onClick={() => onNudgeFrame('in', 1)}
            className="px-2 py-1 rounded bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200 font-mono"
          >
            In +1f
          </button>
          <button
            type="button"
            data-testid="nudge-in-plus-sec"
            onClick={() => onNudgeSecond('in', 1)}
            className="px-2 py-1 rounded bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200 font-mono"
          >
            In +1s
          </button>
        </div>

        <div className="flex flex-wrap items-center gap-1.5">
          <button
            type="button"
            data-testid="nudge-out-minus-sec"
            onClick={() => onNudgeSecond('out', -1)}
            className="px-2 py-1 rounded bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200 font-mono"
          >
            Out −1s
          </button>
          <button
            type="button"
            data-testid="nudge-out-minus-frame"
            onClick={() => onNudgeFrame('out', -1)}
            className="px-2 py-1 rounded bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200 font-mono"
          >
            Out −1f
          </button>
          <button
            type="button"
            data-testid="nudge-out-plus-frame"
            onClick={() => onNudgeFrame('out', 1)}
            className="px-2 py-1 rounded bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200 font-mono"
          >
            Out +1f
          </button>
          <button
            type="button"
            data-testid="nudge-out-plus-sec"
            onClick={() => onNudgeSecond('out', 1)}
            className="px-2 py-1 rounded bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200 font-mono"
          >
            Out +1s
          </button>
          <button
            type="button"
            data-testid="btn-set-out"
            onClick={onSetOutAtPlayhead}
            className="px-2.5 py-1 rounded bg-slate-800 hover:bg-slate-700 border border-slate-600 text-emerald-300 font-medium"
          >
            {t('editor.trim.setOut', undefined, locale)}
          </button>
        </div>
      </div>
    </section>
  );
}
