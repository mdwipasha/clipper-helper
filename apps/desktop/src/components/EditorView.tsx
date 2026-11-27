import { SupportedLocale, t } from '@clipper-helper/i18n';
import { Composition, CompositionSchema, Rect } from '@clipper-helper/shared';
import { convertFileSrc } from '@tauri-apps/api/core';
import {
  MouseEvent as ReactMouseEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  WheelEvent as ReactWheelEvent,
} from 'react';
import {
  cropRectToCssTransform,
  getZoomFromCropRect,
  MAX_CROP_ZOOM,
  MIN_CROP_ZOOM,
} from '../editor/cropMath';
import {
  createEditorState,
  EditorAction,
  EditorState,
  executeEditorCommand,
  getFillCrop,
  getTrimBounds,
  markEditorSaved,
  redoEditorCommand,
  undoEditorCommand,
} from '../editor/editorState';
import { DEFAULT_MIN_CLIP_MS, snapMsToFrame, stepByFrames } from '../editor/frameMath';
import {
  AppCommandError,
  commands,
  events,
  JobRecord,
  ProjectMediaInfo,
  ProjectRecord,
  ThumbnailProgressEvent,
} from '../ipc/bindings';
import { EditorTimeline, FilmstripStatus } from './EditorTimeline';
import { ErrorBanner, toErrorBannerModel } from './ErrorBanner';

export interface EditorViewProps {
  locale: SupportedLocale;
  project: ProjectRecord;
  workspaceRoot: string | null;
  onBackHome: () => void;
  onChangeRange: (updated?: ProjectRecord) => void;
  onOpenDiagnostics: () => void;
  onProjectUpdated: (project: ProjectRecord) => void;
  onStartGenerate?: (projectId: string) => Promise<JobRecord | null>;
}

type SaveStatus = 'saved' | 'dirty' | 'saving' | 'error';

const ALLOWED_REL_PREFIX = /^(cache|projects)\//;

export function buildWorkspaceAssetUrl(
  workspaceRoot: string | null,
  relPath: string | null
): string | null {
  if (!relPath || !ALLOWED_REL_PREFIX.test(relPath) || relPath.includes('..')) {
    return null;
  }
  if (!workspaceRoot) {
    return convertFileSrc(relPath);
  }
  const cleanRoot = workspaceRoot.replace(/[\\/]+$/, '');
  const sep = cleanRoot.includes('\\') && !cleanRoot.includes('/') ? '\\' : '/';
  const joined = `${cleanRoot}${sep}${relPath.split('/').join(sep)}`;
  return convertFileSrc(joined);
}

type VideoFrameCb = (now: number, meta: { mediaTime?: number }) => void;

type VideoWithFrameCallback = HTMLVideoElement & {
  requestVideoFrameCallback?: (cb: VideoFrameCb) => number;
  cancelVideoFrameCallback?: (handle: number) => void;
};

export function EditorView({
  locale,
  project,
  workspaceRoot,
  onBackHome,
  onChangeRange,
  onOpenDiagnostics,
  onProjectUpdated,
  onStartGenerate,
}: EditorViewProps) {
  const [media, setMedia] = useState<ProjectMediaInfo | null>(null);
  const [loadError, setLoadError] = useState<AppCommandError | null>(null);
  const [loading, setLoading] = useState<boolean>(true);
  const [editorState, setEditorState] = useState<EditorState | null>(null);

  const [saveStatus, setSaveStatus] = useState<SaveStatus>('saved');
  const [saveError, setSaveError] = useState<AppCommandError | null>(null);
  const [decodeError, setDecodeError] = useState<boolean>(false);
  const [reproxyLoading, setReproxyLoading] = useState<boolean>(false);

  const [pendingNav, setPendingNav] = useState<'home' | 'range' | null>(null);

  const [isPlaying, setIsPlaying] = useState<boolean>(false);
  const [loopSelection, setLoopSelection] = useState<boolean>(true);
  const [muted, setMuted] = useState<boolean>(false);
  const [playbackRate, setPlaybackRate] = useState<0.5 | 1 | 2>(1);
  const [playheadMs, setPlayheadMs] = useState<number>(0);

  const [filmstripFrames, setFilmstripFrames] = useState<string[]>([]);
  const [filmstripStatus, setFilmstripStatus] = useState<FilmstripStatus>('idle');
  const [filmstripProgress, setFilmstripProgress] = useState<ThumbnailProgressEvent | null>(null);

  const videoRef = useRef<HTMLVideoElement | null>(null);
  const overviewCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const overviewContainerRef = useRef<HTMLDivElement | null>(null);
  const previewContainerRef = useRef<HTMLDivElement | null>(null);
  const lastOverviewDrawRef = useRef<number>(0);
  const editorStateRef = useRef<EditorState | null>(null);
  const savingInFlightRef = useRef<boolean>(false);
  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const loopSelectionRef = useRef<boolean>(true);

  editorStateRef.current = editorState;
  loopSelectionRef.current = loopSelection;

  // Load media metadata & initial/saved composition
  const loadMedia = useCallback(async () => {
    setLoading(true);
    setLoadError(null);
    setDecodeError(false);
    const res = await commands.getProjectMedia(project.id);
    if (res.status === 'error') {
      setLoadError(res.error);
      setLoading(false);
      return;
    }
    const info = res.data;
    setMedia(info);
    try {
      const parsedJson = JSON.parse(info.composition_json) as unknown;
      const parsedComp = CompositionSchema.safeParse(parsedJson);
      if (!parsedComp.success) {
        setLoadError({
          code: 'E_TEMPLATE_INVALID',
          message_key: 'err.E_TEMPLATE_INVALID.body',
          message: 'Saved composition failed validation',
          detail: parsedComp.error.issues.map((issue) => issue.message).join('; '),
          disk_bounds: null,
        });
        setLoading(false);
        return;
      }
      const initial = createEditorState(parsedComp.data, {
        fps: { num: info.fps_num, den: info.fps_den },
        sourceWidth: info.width,
        sourceHeight: info.height,
        sourceDurationMs: info.duration_ms,
        minClipMs: DEFAULT_MIN_CLIP_MS,
        maxClipMs: Math.max(1000, info.max_clip_seconds * 1000),
      });
      setEditorState(initial);
      const trim = getTrimBounds(initial.composition);
      setPlayheadMs(trim.inMs);
      setSaveStatus('saved');
      setSaveError(null);
    } catch (err: unknown) {
      setLoadError({
        code: 'E_TEMPLATE_INVALID',
        message_key: 'err.E_TEMPLATE_INVALID.body',
        message: 'Failed to parse composition JSON',
        detail: err instanceof Error ? err.message : String(err),
        disk_bounds: null,
      });
    } finally {
      setLoading(false);
    }
  }, [project.id]);

  useEffect(() => {
    void loadMedia();
  }, [loadMedia]);

  // Listen for thumbnail progress events + start filmstrip job when playable
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    if (typeof events?.thumbnailProgressEvent?.listen === 'function') {
      events.thumbnailProgressEvent
        .listen((ev) => {
          if (disposed || ev.payload.project_id !== project.id) return;
          setFilmstripProgress(ev.payload);
        })
        .then((fn) => {
          if (disposed) fn();
          else unlisten = fn;
        })
        .catch(() => undefined);
    }
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, [project.id]);

  const startFilmstrip = useCallback(async () => {
    setFilmstripStatus('generating');
    setFilmstripProgress(null);
    const res = await commands.generateProjectThumbnails(project.id);
    if (res.status === 'ok') {
      const urls = res.data.frame_rel_paths
        .map((rel) => buildWorkspaceAssetUrl(workspaceRoot, rel))
        .filter((u): u is string => u !== null);
      setFilmstripFrames(urls);
      setFilmstripStatus('ready');
    } else if (res.error.code === 'E_JOB_CANCELLED') {
      setFilmstripStatus('cancelled');
    } else {
      setFilmstripStatus('error');
    }
  }, [project.id, workspaceRoot]);

  useEffect(() => {
    if (!media || media.media_missing || media.proxy_missing || !media.playable_rel_path) {
      return undefined;
    }
    void startFilmstrip();
    return () => {
      void Promise.resolve(commands.cancelProjectThumbnails(project.id)).catch(() => undefined);
    };
  }, [media, project.id, startFilmstrip]);

  // Autosave logic (debounced ~1s + blur + unmount flush)
  const flushSave = useCallback(async (): Promise<boolean> => {
    const current = editorStateRef.current;
    if (!current || !current.dirty || savingInFlightRef.current) {
      return current ? !current.dirty : true;
    }
    savingInFlightRef.current = true;
    setSaveStatus('saving');
    setSaveError(null);
    const payload = JSON.stringify(current.composition);
    try {
      const res = await commands.saveComposition(project.id, payload);
      if (res.status === 'ok') {
        let parsedPersisted: Composition | undefined;
        try {
          const parsed = CompositionSchema.safeParse(JSON.parse(res.data.composition_json));
          if (parsed.success) parsedPersisted = parsed.data;
        } catch {
          parsedPersisted = undefined;
        }
        setEditorState((prev) => {
          if (!prev) return prev;
          // Keep dirty=true if another edit happened while awaiting IPC
          if (JSON.stringify(prev.composition) !== payload) {
            return prev;
          }
          return markEditorSaved(prev, parsedPersisted);
        });
        setSaveStatus('saved');
        onProjectUpdated(res.data.project);
        return true;
      }
      setSaveStatus('error');
      setSaveError(res.error);
      return false;
    } catch (err: unknown) {
      setSaveStatus('error');
      setSaveError({
        code: 'E_UNKNOWN',
        message_key: 'err.E_UNKNOWN.body',
        message: err instanceof Error ? err.message : String(err),
        detail: err instanceof Error ? err.message : String(err),
        disk_bounds: null,
      });
      return false;
    } finally {
      savingInFlightRef.current = false;
    }
  }, [onProjectUpdated, project.id]);

  useEffect(() => {
    if (!editorState?.dirty) return undefined;
    setSaveStatus((prev) => (prev === 'saving' ? prev : 'dirty'));
    if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
    saveTimerRef.current = setTimeout(() => {
      void flushSave();
    }, 1000);
    return () => {
      if (saveTimerRef.current) {
        clearTimeout(saveTimerRef.current);
        saveTimerRef.current = null;
      }
    };
  }, [editorState?.composition, editorState?.dirty, flushSave]);

  useEffect(() => {
    const onBlur = () => {
      if (editorStateRef.current?.dirty) {
        void flushSave();
      }
    };
    window.addEventListener('blur', onBlur);
    return () => {
      window.removeEventListener('blur', onBlur);
      if (editorStateRef.current?.dirty) {
        void Promise.resolve(
          commands.saveComposition(project.id, JSON.stringify(editorStateRef.current.composition))
        ).catch(() => undefined);
      }
    };
  }, [flushSave, project.id]);

  const dispatchCommand = useCallback((action: EditorAction) => {
    setEditorState((prev) => (prev ? executeEditorCommand(prev, action) : prev));
  }, []);

  const handleUndo = useCallback(() => {
    setEditorState((prev) => (prev ? undoEditorCommand(prev) : prev));
  }, []);

  const handleRedo = useCallback(() => {
    setEditorState((prev) => (prev ? redoEditorCommand(prev) : prev));
  }, []);

  const currentTrim = useMemo(
    () => (editorState ? getTrimBounds(editorState.composition) : { inMs: 0, outMs: 1000 }),
    [editorState]
  );

  const currentCrop: Rect = useMemo(() => {
    if (!editorState || !media) {
      return { x: 0.34, y: 0, w: 0.32, h: 1 };
    }
    return getFillCrop(editorState.composition, media.width, media.height);
  }, [editorState, media]);

  const cropZoom = useMemo(() => {
    if (!media) return 1;
    return getZoomFromCropRect(currentCrop, media.width, media.height);
  }, [currentCrop, media]);

  // Throttled ~5 fps overview canvas redraw
  const drawOverviewFrame = useCallback(
    (force = false) => {
      const canvas = overviewCanvasRef.current;
      const video = videoRef.current;
      if (!canvas || !video) return;
      const now = typeof performance !== 'undefined' ? performance.now() : Date.now();
      if (!force && now - lastOverviewDrawRef.current < 200) {
        return;
      }
      lastOverviewDrawRef.current = now;
      try {
        const ctx = canvas.getContext('2d');
        if (!ctx) return;
        const cw = canvas.width;
        const ch = canvas.height;
        ctx.clearRect(0, 0, cw, ch);
        if (video.readyState >= 2) {
          ctx.drawImage(video, 0, 0, cw, ch);
        } else {
          ctx.fillStyle = '#0f172a';
          ctx.fillRect(0, 0, cw, ch);
        }
        // Subtle vignette outside crop box
        ctx.fillStyle = 'rgba(2, 6, 23, 0.55)';
        ctx.fillRect(0, 0, cw, ch);
        const rx = currentCrop.x * cw;
        const ry = currentCrop.y * ch;
        const rw = currentCrop.w * cw;
        const rh = currentCrop.h * ch;
        if (video.readyState >= 2) {
          ctx.save();
          ctx.beginPath();
          ctx.rect(rx, ry, rw, rh);
          ctx.clip();
          ctx.drawImage(video, 0, 0, cw, ch);
          ctx.restore();
        }
        ctx.strokeStyle = '#34d399';
        ctx.lineWidth = 2;
        ctx.strokeRect(rx, ry, rw, rh);
      } catch {
        // Ignore JSDOM / headless canvas errors cleanly
      }
    },
    [currentCrop]
  );

  useEffect(() => {
    drawOverviewFrame(true);
  }, [drawOverviewFrame]);

  // Seek helper
  const seekToMs = useCallback(
    (rawMs: number) => {
      if (!editorState) return;
      const snapped = snapMsToFrame(
        rawMs,
        editorState.bounds.fps,
        editorState.bounds.sourceDurationMs
      );
      setPlayheadMs(snapped);
      const vid = videoRef.current;
      if (vid) {
        vid.currentTime = snapped / 1000;
      }
      drawOverviewFrame(true);
    },
    [drawOverviewFrame, editorState]
  );

  const togglePlayPause = useCallback(async () => {
    const vid = videoRef.current;
    if (!vid) return;
    if (!vid.paused) {
      vid.pause();
      setIsPlaying(false);
      return;
    }
    if (
      loopSelectionRef.current &&
      (playheadMs < currentTrim.inMs || playheadMs >= currentTrim.outMs)
    ) {
      vid.currentTime = currentTrim.inMs / 1000;
      setPlayheadMs(currentTrim.inMs);
    }
    try {
      await vid.play();
      setIsPlaying(true);
    } catch {
      setIsPlaying(false);
    }
  }, [currentTrim.inMs, currentTrim.outMs, playheadMs]);

  // Frame-accurate time synchronization (requestVideoFrameCallback + rAF fallback)
  useEffect(() => {
    const vid = videoRef.current as VideoWithFrameCallback | null;
    if (!vid || !media) return undefined;

    let stopped = false;
    let vfcHandle: number | null = null;
    let rafHandle: number | null = null;

    const handleFrameTime = (seconds: number) => {
      const rawMs = Math.round(seconds * 1000);
      if (loopSelectionRef.current && !vid.paused && rawMs >= currentTrim.outMs) {
        vid.currentTime = currentTrim.inMs / 1000;
        setPlayheadMs(currentTrim.inMs);
      } else {
        setPlayheadMs(rawMs);
      }
      drawOverviewFrame(false);
    };

    const scheduleVfc = () => {
      if (stopped || typeof vid.requestVideoFrameCallback !== 'function') return;
      vfcHandle = vid.requestVideoFrameCallback((_now, metadata) => {
        if (stopped) return;
        const sec =
          typeof metadata?.mediaTime === 'number' && Number.isFinite(metadata.mediaTime)
            ? metadata.mediaTime
            : vid.currentTime;
        handleFrameTime(sec);
        if (!vid.paused) {
          scheduleVfc();
        }
      });
    };

    const onPlay = () => {
      setIsPlaying(true);
      if (typeof vid.requestVideoFrameCallback === 'function') {
        scheduleVfc();
      } else {
        const tick = () => {
          if (stopped || vid.paused) return;
          handleFrameTime(vid.currentTime);
          rafHandle = window.requestAnimationFrame(tick);
        };
        rafHandle = window.requestAnimationFrame(tick);
      }
    };

    const onPause = () => {
      setIsPlaying(false);
      drawOverviewFrame(true);
    };

    const onTimeUpdate = () => {
      handleFrameTime(vid.currentTime);
    };

    const onSeeked = () => {
      setPlayheadMs(Math.round(vid.currentTime * 1000));
      drawOverviewFrame(true);
    };

    const onLoadedData = () => {
      if (vid.currentTime === 0 && currentTrim.inMs > 0) {
        vid.currentTime = currentTrim.inMs / 1000;
      }
      drawOverviewFrame(true);
    };

    vid.addEventListener('play', onPlay);
    vid.addEventListener('pause', onPause);
    vid.addEventListener('timeupdate', onTimeUpdate);
    vid.addEventListener('seeked', onSeeked);
    vid.addEventListener('loadeddata', onLoadedData);
    return () => {
      stopped = true;
      vid.removeEventListener('play', onPlay);
      vid.removeEventListener('pause', onPause);
      vid.removeEventListener('timeupdate', onTimeUpdate);
      vid.removeEventListener('seeked', onSeeked);
      vid.removeEventListener('loadeddata', onLoadedData);
      if (vfcHandle !== null && typeof vid.cancelVideoFrameCallback === 'function') {
        vid.cancelVideoFrameCallback(vfcHandle);
      }
      if (rafHandle !== null) {
        window.cancelAnimationFrame(rafHandle);
      }
    };
  }, [currentTrim.inMs, currentTrim.outMs, drawOverviewFrame, media]);

  // Sync mute and playback rate to `<video>`
  useEffect(() => {
    const vid = videoRef.current;
    if (!vid) return;
    vid.muted = muted;
    vid.playbackRate = playbackRate;
  }, [muted, playbackRate, media]);

  // Keyboard shortcuts
  useEffect(() => {
    const onKeyDown = (ev: KeyboardEvent) => {
      const target = ev.target as HTMLElement | null;
      if (
        target &&
        (target.tagName === 'INPUT' ||
          target.tagName === 'TEXTAREA' ||
          target.tagName === 'SELECT' ||
          target.isContentEditable)
      ) {
        return;
      }
      const mod = ev.ctrlKey || ev.metaKey;
      const lower = ev.key.toLowerCase();

      if (mod && lower === 'z') {
        ev.preventDefault();
        if (ev.shiftKey) handleRedo();
        else handleUndo();
        return;
      }
      if (mod && lower === 'y') {
        ev.preventDefault();
        handleRedo();
        return;
      }

      if (!editorStateRef.current) return;

      if (ev.code === 'Space') {
        ev.preventDefault();
        void togglePlayPause();
      } else if (ev.key === 'ArrowLeft') {
        ev.preventDefault();
        const st = editorStateRef.current;
        const next = ev.shiftKey
          ? snapMsToFrame(playheadMs - 1000, st.bounds.fps, st.bounds.sourceDurationMs)
          : stepByFrames(playheadMs, -1, st.bounds.fps, st.bounds.sourceDurationMs);
        seekToMs(next);
      } else if (ev.key === 'ArrowRight') {
        ev.preventDefault();
        const st = editorStateRef.current;
        const next = ev.shiftKey
          ? snapMsToFrame(playheadMs + 1000, st.bounds.fps, st.bounds.sourceDurationMs)
          : stepByFrames(playheadMs, 1, st.bounds.fps, st.bounds.sourceDurationMs);
        seekToMs(next);
      } else if (lower === 'i' && !mod) {
        ev.preventDefault();
        dispatchCommand({
          type: 'setTrim',
          inMs: playheadMs,
          outMs: Math.max(playheadMs + 1, getTrimBounds(editorStateRef.current.composition).outMs),
        });
      } else if (lower === 'o' && !mod) {
        ev.preventDefault();
        dispatchCommand({
          type: 'setTrim',
          inMs: Math.min(getTrimBounds(editorStateRef.current.composition).inMs, playheadMs - 1),
          outMs: playheadMs,
        });
      } else if (ev.key === 'Home') {
        ev.preventDefault();
        seekToMs(getTrimBounds(editorStateRef.current.composition).inMs);
      } else if (ev.key === 'End') {
        ev.preventDefault();
        seekToMs(getTrimBounds(editorStateRef.current.composition).outMs);
      }
    };

    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [dispatchCommand, handleRedo, handleUndo, playheadMs, seekToMs, togglePlayPause]);

  // Interactive crop drag on 9:16 preview or overview box
  const cropCss = useMemo(() => {
    if (!media) {
      return cropRectToCssTransform(currentCrop, 270, 480, 1920, 1080);
    }
    return cropRectToCssTransform(currentCrop, 270, 480, media.width, media.height);
  }, [currentCrop, media]);

  const handlePreviewMouseDown = (ev: ReactMouseEvent<HTMLDivElement>) => {
    if (ev.button !== 0 || !media) return;
    ev.preventDefault();
    let lastX = ev.clientX;
    let lastY = ev.clientY;
    const containerW = Math.max(1, previewContainerRef.current?.clientWidth || 270);
    const containerH = Math.max(1, previewContainerRef.current?.clientHeight || 480);
    const transformScaleX = containerW / Math.max(1e-4, currentCrop.w);
    const transformScaleY = containerH / Math.max(1e-4, currentCrop.h);

    const onMove = (moveEv: MouseEvent) => {
      const dxPx = moveEv.clientX - lastX;
      const dyPx = moveEv.clientY - lastY;
      lastX = moveEv.clientX;
      lastY = moveEv.clientY;
      dispatchCommand({
        type: 'pan',
        dx: -dxPx / transformScaleX,
        dy: -dyPx / transformScaleY,
        coalesceKey: 'preview-pan',
      });
    };

    const onUp = () => {
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
    };

    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  };

  const handleOverviewMouseDown = (ev: ReactMouseEvent<HTMLDivElement>) => {
    if (ev.button !== 0 || !media) return;
    ev.preventDefault();
    let lastX = ev.clientX;
    let lastY = ev.clientY;
    const boxW = Math.max(1, overviewContainerRef.current?.clientWidth || 320);
    const boxH = Math.max(1, overviewContainerRef.current?.clientHeight || 180);

    const onMove = (moveEv: MouseEvent) => {
      const dxPx = moveEv.clientX - lastX;
      const dyPx = moveEv.clientY - lastY;
      lastX = moveEv.clientX;
      lastY = moveEv.clientY;
      dispatchCommand({
        type: 'pan',
        dx: dxPx / boxW,
        dy: dyPx / boxH,
        coalesceKey: 'overview-pan',
      });
    };

    const onUp = () => {
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
    };

    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  };

  const handlePreviewWheel = (ev: ReactWheelEvent<HTMLDivElement>) => {
    ev.preventDefault();
    const step = ev.deltaY < 0 ? 0.1 : -0.1;
    const nextZoom = Math.min(MAX_CROP_ZOOM, Math.max(MIN_CROP_ZOOM, cropZoom + step));
    dispatchCommand({ type: 'zoom', factor: nextZoom, coalesceKey: 'preview-wheel-zoom' });
  };

  const handleVideoDecodeError = () => {
    setDecodeError(true);
    setIsPlaying(false);
    void Promise.resolve(
      commands.recordWebviewError('editor-video', 'WebView HTMLVideoElement decode error', null)
    ).catch(() => undefined);
  };

  const handleReproxy = async () => {
    setReproxyLoading(true);
    try {
      if (onStartGenerate) {
        await onStartGenerate(project.id);
      } else {
        await commands.startGenerateJob(project.id);
      }
      onChangeRange(project);
    } finally {
      setReproxyLoading(false);
    }
  };

  const handleNavigateRequest = async (dest: 'home' | 'range') => {
    if (editorState?.dirty || saveStatus === 'error') {
      const ok = await flushSave();
      if (!ok) {
        setPendingNav(dest);
        return;
      }
    }
    if (dest === 'home') onBackHome();
    else onChangeRange(project);
  };

  const playableAssetUrl = useMemo(() => {
    if (!media || !media.playable_rel_path) return null;
    return buildWorkspaceAssetUrl(workspaceRoot, media.playable_rel_path);
  }, [media, workspaceRoot]);

  const saveLabelKey =
    saveStatus === 'saving'
      ? 'editor.saveStatus.saving'
      : saveStatus === 'dirty'
        ? 'editor.saveStatus.dirty'
        : saveStatus === 'error'
          ? 'editor.saveStatus.error'
          : 'editor.saveStatus.saved';

  const saveBadgeTone =
    saveStatus === 'error'
      ? 'bg-rose-950/70 border-rose-700 text-rose-200'
      : saveStatus === 'saving' || saveStatus === 'dirty'
        ? 'bg-amber-950/70 border-amber-700 text-amber-200'
        : 'bg-emerald-950/70 border-emerald-700 text-emerald-200';

  return (
    <section data-testid="editor-view" className="space-y-4">
      {/* Header Toolbar */}
      <div className="bg-slate-900/90 border border-slate-700/80 rounded-xl px-4 py-3 flex flex-wrap items-center justify-between gap-3">
        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            data-testid="btn-editor-back-home"
            onClick={() => void handleNavigateRequest('home')}
            className="text-xs px-3 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 border border-slate-600 text-slate-200"
          >
            ← {t('editor.backHome', undefined, locale)}
          </button>
          <button
            type="button"
            data-testid="btn-editor-change-range"
            onClick={() => void handleNavigateRequest('range')}
            className="text-xs px-3 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-300"
          >
            {t('editor.changeRange', undefined, locale)}
          </button>
          <div className="min-w-0 pl-1">
            <h2 className="text-sm font-semibold text-white truncate max-w-xs sm:max-w-md">
              {project.title}
            </h2>
            <p className="text-[11px] text-slate-400">
              {t('editor.title', undefined, locale)}
              {media && (
                <>
                  {' '}
                  ·{' '}
                  {media.using_proxy
                    ? t('editor.proxyBadge', undefined, locale)
                    : t('editor.originalBadge', undefined, locale)}
                </>
              )}
            </p>
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            data-testid="btn-undo"
            disabled={!editorState || editorState.undoStack.length === 0}
            onClick={handleUndo}
            className="text-xs px-2.5 py-1.5 rounded bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200 disabled:opacity-40"
          >
            ↶ {t('editor.undo', undefined, locale)}
          </button>
          <button
            type="button"
            data-testid="btn-redo"
            disabled={!editorState || editorState.redoStack.length === 0}
            onClick={handleRedo}
            className="text-xs px-2.5 py-1.5 rounded bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200 disabled:opacity-40"
          >
            ↷ {t('editor.redo', undefined, locale)}
          </button>

          <span
            data-testid="editor-save-status"
            className={`text-[11px] font-medium px-2.5 py-1 rounded-full border ${saveBadgeTone}`}
          >
            {t(saveLabelKey, undefined, locale)}
          </span>

          <div className="flex items-center gap-1.5">
            <button
              type="button"
              data-testid="btn-export-disabled"
              disabled
              aria-disabled="true"
              className="text-xs px-3 py-1.5 rounded-lg bg-slate-800 border border-slate-700 text-slate-400 cursor-not-allowed"
            >
              {t('editor.exportButton', undefined, locale)}
            </button>
            <span className="text-[11px] text-slate-400">
              {t('editor.exportComingSoon', undefined, locale)}
            </span>
          </div>
        </div>
      </div>

      {/* Unsaved navigation guard */}
      {pendingNav && (
        <div
          data-testid="editor-unsaved-guard"
          role="alertdialog"
          className="bg-amber-950/70 border border-amber-700 rounded-xl p-3 flex flex-wrap items-center justify-between gap-3 text-xs text-amber-100"
        >
          <div>
            <p className="font-semibold">{t('editor.unsavedGuard.title', undefined, locale)}</p>
            <p className="text-amber-200/90">{t('editor.unsavedGuard.body', undefined, locale)}</p>
          </div>
          <div className="flex gap-2">
            <button
              type="button"
              data-testid="btn-guard-stay"
              onClick={() => setPendingNav(null)}
              className="px-3 py-1.5 rounded bg-slate-800 hover:bg-slate-700 border border-slate-600 text-white"
            >
              {t('editor.unsavedGuard.stay', undefined, locale)}
            </button>
            <button
              type="button"
              data-testid="btn-guard-leave"
              onClick={() => {
                const dest = pendingNav;
                setPendingNav(null);
                if (dest === 'home') onBackHome();
                else onChangeRange(project);
              }}
              className="px-3 py-1.5 rounded bg-amber-600 hover:bg-amber-500 text-slate-950 font-semibold"
            >
              {t('editor.unsavedGuard.leaveAnyway', undefined, locale)}
            </button>
          </div>
        </div>
      )}

      {/* Save error banner */}
      {saveError && (
        <ErrorBanner
          testId="editor-save-error-banner"
          locale={locale}
          error={toErrorBannerModel(saveError, locale, [
            {
              label: t('job.actionRetry', undefined, locale),
              variant: 'primary',
              onClick: () => void flushSave(),
            },
          ])}
          onOpenDiagnostics={onOpenDiagnostics}
        />
      )}

      {loading && (
        <div
          data-testid="editor-loading"
          className="bg-slate-900/70 border border-slate-800 rounded-xl p-6 text-xs text-slate-300"
        >
          {t('editor.loadingMedia', undefined, locale)}
        </div>
      )}

      {loadError && !loading && (
        <ErrorBanner
          testId="editor-load-error-banner"
          locale={locale}
          error={toErrorBannerModel(loadError, locale, [
            {
              label: t('job.actionRetry', undefined, locale),
              variant: 'primary',
              onClick: () => void loadMedia(),
            },
          ])}
          onOpenDiagnostics={onOpenDiagnostics}
        />
      )}

      {/* Media Missing State */}
      {!loading && media?.media_missing && (
        <div
          data-testid="editor-media-missing-card"
          role="alert"
          className="bg-amber-950/50 border border-amber-700 rounded-xl p-5 space-y-3"
        >
          <h3 className="text-sm font-semibold text-amber-200">
            {t('editor.errors.mediaMissingTitle', undefined, locale)}
          </h3>
          <p className="text-xs text-amber-100/90">
            {t('editor.errors.mediaMissingBody', undefined, locale)}
          </p>
          <button
            type="button"
            data-testid="btn-redownload"
            onClick={() => onChangeRange(project)}
            className="px-4 py-2 rounded-lg bg-amber-600 hover:bg-amber-500 text-slate-950 text-xs font-semibold"
          >
            {t('editor.errors.redownloadAction', undefined, locale)}
          </button>
        </div>
      )}

      {/* Proxy Missing State */}
      {!loading && media && !media.media_missing && media.proxy_missing && (
        <div
          data-testid="editor-proxy-missing-card"
          role="alert"
          className="bg-amber-950/50 border border-amber-700 rounded-xl p-5 space-y-3"
        >
          <h3 className="text-sm font-semibold text-amber-200">
            {t('editor.errors.proxyMissingTitle', undefined, locale)}
          </h3>
          <p className="text-xs text-amber-100/90">
            {t('editor.errors.proxyMissingBody', undefined, locale)}
          </p>
          <button
            type="button"
            data-testid="btn-reproxy"
            disabled={reproxyLoading}
            onClick={() => void handleReproxy()}
            className="px-4 py-2 rounded-lg bg-indigo-600 hover:bg-indigo-500 text-white text-xs font-semibold disabled:opacity-50"
          >
            {t('editor.errors.reproxyAction', undefined, locale)}
          </button>
        </div>
      )}

      {/* Main Editor Workspace */}
      {!loading && media && !media.media_missing && !media.proxy_missing && editorState && (
        <>
          {decodeError && (
            <ErrorBanner
              testId="editor-decode-error-banner"
              locale={locale}
              error={{
                code: 'E_VERIFY_FAILED',
                title: t('editor.errors.decodeErrorTitle', undefined, locale),
                message: t('editor.errors.decodeErrorBody', undefined, locale),
                actions: [
                  {
                    label: t('editor.errors.redownloadAction', undefined, locale),
                    variant: 'primary',
                    onClick: () => onChangeRange(project),
                  },
                ],
              }}
              onOpenDiagnostics={onOpenDiagnostics}
            />
          )}

          <div className="grid grid-cols-1 lg:grid-cols-12 gap-4 items-start">
            {/* Left: 9:16 Interactive Portrait Preview */}
            <div className="lg:col-span-5 bg-slate-900/80 border border-slate-700/80 rounded-xl p-4 space-y-3 flex flex-col items-center">
              <div className="w-full flex items-center justify-between text-xs">
                <span className="font-semibold text-slate-200">
                  {t('editor.preview.title', undefined, locale)}
                </span>
                <span className="font-mono text-[11px] text-slate-400">{cropZoom.toFixed(2)}×</span>
              </div>

              <div
                ref={previewContainerRef}
                data-testid="editor-preview-viewport"
                onMouseDown={handlePreviewMouseDown}
                onWheel={handlePreviewWheel}
                className="relative w-64 max-w-full aspect-[9/16] bg-black rounded-lg border border-slate-700 overflow-hidden cursor-grab active:cursor-grabbing select-none"
              >
                <video
                  ref={videoRef}
                  data-testid="editor-video"
                  src={playableAssetUrl ?? undefined}
                  playsInline
                  preload="metadata"
                  onError={handleVideoDecodeError}
                  style={{
                    width: `${cropCss.widthPct}%`,
                    height: `${cropCss.heightPct}%`,
                    maxWidth: 'none',
                    maxHeight: 'none',
                    transform: cropCss.cssTransform,
                  }}
                  className="block object-fill pointer-events-none"
                />
              </div>

              <p className="text-[11px] text-slate-400 text-center">
                {t('editor.preview.panHint', undefined, locale)}
              </p>

              {/* Crop zoom & align controls */}
              <div className="w-full flex flex-wrap items-center gap-2 text-xs">
                <label className="flex items-center gap-2 flex-1 min-w-[160px] text-slate-300">
                  <span>{t('editor.preview.zoom', undefined, locale)}</span>
                  <input
                    type="range"
                    data-testid="crop-zoom-slider"
                    aria-label={t('editor.preview.zoom', undefined, locale)}
                    min={MIN_CROP_ZOOM}
                    max={MAX_CROP_ZOOM}
                    step={0.05}
                    value={cropZoom}
                    onChange={(ev) =>
                      dispatchCommand({
                        type: 'zoom',
                        factor: Number(ev.target.value),
                        coalesceKey: 'slider-zoom',
                      })
                    }
                    className="flex-1 accent-emerald-400"
                  />
                </label>
                <button
                  type="button"
                  data-testid="btn-crop-center"
                  onClick={() => dispatchCommand({ type: 'centerCrop' })}
                  className="px-2.5 py-1 rounded bg-slate-800 hover:bg-slate-700 border border-slate-600 text-slate-200"
                >
                  {t('editor.preview.center', undefined, locale)}
                </button>
                <button
                  type="button"
                  data-testid="btn-crop-reset"
                  onClick={() => dispatchCommand({ type: 'resetCrop' })}
                  className="px-2.5 py-1 rounded bg-slate-800 hover:bg-slate-700 border border-slate-600 text-slate-200"
                >
                  {t('editor.preview.reset', undefined, locale)}
                </button>
              </div>
            </div>

            {/* Right: Mini Full-Frame Overview Canvas + Draggable Crop Overlay + Transport */}
            <div className="lg:col-span-7 space-y-4">
              <div className="bg-slate-900/80 border border-slate-700/80 rounded-xl p-4 space-y-3">
                <div className="flex items-center justify-between text-xs">
                  <span className="font-semibold text-slate-200">
                    {t('editor.preview.overviewTitle', undefined, locale)}
                  </span>
                  <span className="font-mono text-[11px] text-slate-400">
                    {media.width}×{media.height} · {media.fps.toFixed(2)} fps
                  </span>
                </div>

                <div
                  ref={overviewContainerRef}
                  data-testid="editor-overview-container"
                  className="relative w-full aspect-video bg-slate-950 rounded-lg border border-slate-700 overflow-hidden"
                >
                  <canvas
                    ref={overviewCanvasRef}
                    data-testid="editor-overview-canvas"
                    width={480}
                    height={270}
                    className="w-full h-full block"
                  />
                  {/* Draggable crop rect overlay */}
                  <div
                    data-testid="editor-overview-crop-box"
                    onMouseDown={handleOverviewMouseDown}
                    style={{
                      left: `${currentCrop.x * 100}%`,
                      top: `${currentCrop.y * 100}%`,
                      width: `${currentCrop.w * 100}%`,
                      height: `${currentCrop.h * 100}%`,
                    }}
                    className="absolute border-2 border-emerald-400 bg-emerald-400/10 cursor-move"
                  />
                </div>
                <p className="text-[11px] text-slate-400">
                  {t('editor.preview.overviewHint', undefined, locale)}
                </p>
              </div>

              {/* Transport Bar */}
              <div className="bg-slate-900/80 border border-slate-700/80 rounded-xl p-4 space-y-3">
                <div className="flex flex-wrap items-center gap-2 text-xs">
                  <button
                    type="button"
                    data-testid="btn-play-pause"
                    onClick={() => void togglePlayPause()}
                    className="px-3.5 py-1.5 rounded-lg bg-indigo-600 hover:bg-indigo-500 text-white font-semibold"
                  >
                    {isPlaying
                      ? t('editor.transport.pause', undefined, locale)
                      : t('editor.transport.play', undefined, locale)}
                  </button>

                  <button
                    type="button"
                    data-testid="btn-loop-toggle"
                    aria-pressed={loopSelection}
                    onClick={() => setLoopSelection((v) => !v)}
                    className={`px-3 py-1.5 rounded-lg border font-medium ${
                      loopSelection
                        ? 'bg-emerald-950/70 border-emerald-600 text-emerald-200'
                        : 'bg-slate-800 border-slate-700 text-slate-300'
                    }`}
                  >
                    {t('editor.transport.loopSelection', undefined, locale)}
                  </button>

                  <button
                    type="button"
                    data-testid="btn-mute-toggle"
                    aria-pressed={muted}
                    onClick={() => setMuted((v) => !v)}
                    className="px-3 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200"
                  >
                    {muted
                      ? t('editor.transport.unmute', undefined, locale)
                      : t('editor.transport.mute', undefined, locale)}
                  </button>

                  <div className="flex items-center gap-1 ml-auto">
                    <span className="text-slate-400 mr-1">
                      {t('editor.transport.rate', undefined, locale)}:
                    </span>
                    {([0.5, 1, 2] as const).map((r) => (
                      <button
                        key={r}
                        type="button"
                        data-testid={`btn-rate-${r}`}
                        onClick={() => setPlaybackRate(r)}
                        className={`px-2 py-1 rounded border font-mono ${
                          playbackRate === r
                            ? 'bg-indigo-600 border-indigo-500 text-white'
                            : 'bg-slate-800 border-slate-700 text-slate-300'
                        }`}
                      >
                        {r}×
                      </button>
                    ))}
                  </div>
                </div>

                <div className="flex flex-wrap items-center gap-2 text-xs">
                  <button
                    type="button"
                    data-testid="btn-jump-in"
                    onClick={() => seekToMs(currentTrim.inMs)}
                    className="px-2.5 py-1 rounded bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200"
                  >
                    ⏮ {t('editor.transport.jumpIn', undefined, locale)}
                  </button>
                  <button
                    type="button"
                    data-testid="btn-playhead-prev-sec"
                    onClick={() =>
                      seekToMs(
                        snapMsToFrame(
                          playheadMs - 1000,
                          editorState.bounds.fps,
                          editorState.bounds.sourceDurationMs
                        )
                      )
                    }
                    className="px-2 py-1 rounded bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200 font-mono"
                  >
                    {t('editor.transport.prevSecond', undefined, locale)}
                  </button>
                  <button
                    type="button"
                    data-testid="btn-playhead-prev-frame"
                    onClick={() =>
                      seekToMs(
                        stepByFrames(
                          playheadMs,
                          -1,
                          editorState.bounds.fps,
                          editorState.bounds.sourceDurationMs
                        )
                      )
                    }
                    className="px-2 py-1 rounded bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200 font-mono"
                  >
                    {t('editor.transport.prevFrame', undefined, locale)}
                  </button>
                  <button
                    type="button"
                    data-testid="btn-playhead-next-frame"
                    onClick={() =>
                      seekToMs(
                        stepByFrames(
                          playheadMs,
                          1,
                          editorState.bounds.fps,
                          editorState.bounds.sourceDurationMs
                        )
                      )
                    }
                    className="px-2 py-1 rounded bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200 font-mono"
                  >
                    {t('editor.transport.nextFrame', undefined, locale)}
                  </button>
                  <button
                    type="button"
                    data-testid="btn-playhead-next-sec"
                    onClick={() =>
                      seekToMs(
                        snapMsToFrame(
                          playheadMs + 1000,
                          editorState.bounds.fps,
                          editorState.bounds.sourceDurationMs
                        )
                      )
                    }
                    className="px-2 py-1 rounded bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200 font-mono"
                  >
                    {t('editor.transport.nextSecond', undefined, locale)}
                  </button>
                  <button
                    type="button"
                    data-testid="btn-jump-out"
                    onClick={() => seekToMs(currentTrim.outMs)}
                    className="px-2.5 py-1 rounded bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200"
                  >
                    {t('editor.transport.jumpOut', undefined, locale)} ⏭
                  </button>
                </div>

                <p className="text-[11px] text-slate-400">
                  {t('editor.transport.shortcutsHint', undefined, locale)}
                </p>
              </div>
            </div>
          </div>

          {/* Bottom: Timeline, Handles, Filmstrip, Trim Controls */}
          <EditorTimeline
            locale={locale}
            sourceDurationMs={media.duration_ms}
            handleBeforeMs={media.handle_before_ms}
            handleAfterMs={media.handle_after_ms}
            inMs={currentTrim.inMs}
            outMs={currentTrim.outMs}
            playheadMs={playheadMs}
            maxClipSeconds={media.max_clip_seconds}
            filmstripFrames={filmstripFrames}
            filmstripStatus={filmstripStatus}
            filmstripProgress={filmstripProgress}
            onSeek={seekToMs}
            onDragTrim={(nextIn, nextOut, coalesceKey) =>
              dispatchCommand({
                type: 'setTrim',
                inMs: nextIn,
                outMs: nextOut,
                coalesceKey,
              })
            }
            onSetInAtPlayhead={() =>
              dispatchCommand({
                type: 'setTrim',
                inMs: playheadMs,
                outMs: Math.max(playheadMs + 1, currentTrim.outMs),
              })
            }
            onSetOutAtPlayhead={() =>
              dispatchCommand({
                type: 'setTrim',
                inMs: Math.min(currentTrim.inMs, playheadMs - 1),
                outMs: playheadMs,
              })
            }
            onNudgeFrame={(edge, deltaFrames) =>
              dispatchCommand({
                type: edge === 'in' ? 'nudgeIn' : 'nudgeOut',
                deltaFrames,
                coalesceKey: `btn-${edge}-frame`,
              })
            }
            onNudgeSecond={(edge, deltaSeconds) =>
              dispatchCommand({
                type: edge === 'in' ? 'nudgeIn' : 'nudgeOut',
                deltaMs: deltaSeconds * 1000,
                coalesceKey: `btn-${edge}-sec`,
              })
            }
            onCancelFilmstrip={() => {
              setFilmstripStatus('cancelled');
              void Promise.resolve(commands.cancelProjectThumbnails(project.id)).catch(
                () => undefined
              );
            }}
            onRetryFilmstrip={() => void startFilmstrip()}
          />
        </>
      )}
    </section>
  );
}
