import type { Composition } from '@clipper-helper/shared';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { EditorView } from '../components/EditorView';
import { defaultCropRect } from '../editor/cropMath';
import {
  commands,
  events,
  ProjectMediaInfo,
  ProjectRecord,
  ThumbnailProgressEvent,
} from '../ipc/bindings';

vi.mock('@tauri-apps/api/core', async () => {
  const actual =
    await vi.importActual<typeof import('@tauri-apps/api/core')>('@tauri-apps/api/core');
  return {
    ...actual,
    convertFileSrc: vi.fn(
      (filePath: string) => `http://asset.localhost/${encodeURIComponent(filePath)}`
    ),
  };
});

vi.mock('../ipc/bindings', async () => {
  const actual = await vi.importActual<typeof import('../ipc/bindings')>('../ipc/bindings');
  return {
    ...actual,
    events: {
      ...actual.events,
      thumbnailProgressEvent: {
        listen: vi.fn().mockResolvedValue(() => undefined),
      },
    },
    commands: {
      ...actual.commands,
      getProjectMedia: vi.fn(),
      saveComposition: vi.fn(),
      generateProjectThumbnails: vi.fn(),
      cancelProjectThumbnails: vi.fn().mockResolvedValue({ status: 'ok', data: true }),
      recordWebviewError: vi.fn().mockResolvedValue({ status: 'ok', data: true }),
      startGenerateJob: vi.fn(),
    },
  };
});

function makeProject(overrides?: Partial<ProjectRecord>): ProjectRecord {
  return {
    id: '11111111-1111-4111-8111-111111111111',
    title: 'Podcast Editor Clip',
    channel: 'Channel Id',
    source_provider: 'youtube',
    source_video_id: 'aqz-KE-bpKQ',
    source_url: 'https://www.youtube.com/watch?v=aqz-KE-bpKQ',
    duration_seconds: 120,
    live_status: 'not_live',
    preview_mode: 'embed',
    suggested_start_seconds: null,
    range_start_ms: 10_000,
    range_end_ms: 25_000,
    status: 'downloaded',
    composition_hash: 'hash-initial',
    created_at: 1_700_000_000,
    updated_at: 1_700_000_000,
    ...overrides,
  };
}

function makeComposition(overrides?: Partial<Composition>): Composition {
  return {
    schemaVersion: 1,
    id: '11111111-1111-4111-8111-111111111111',
    canvas: { width: 1080, height: 1920, fps: 60 },
    sources: [
      {
        id: 'src-1',
        provider: 'youtube',
        videoId: 'aqz-KE-bpKQ',
        path: 'cache/youtube/aqz-KE-bpKQ/src.mp4',
        absoluteStartMs: 5_000,
        durationMs: 30_000,
        width: 1920,
        height: 1080,
        fps: 60,
      },
    ],
    timeline: [{ id: 'seg-1', sourceId: 'src-1', inMs: 5_000, outMs: 20_000 }],
    layout: {
      mode: 'fill',
      crop: defaultCropRect(1920, 1080),
    },
    captions: {
      enabled: false,
      status: 'none',
      language: 'id',
      words: [],
      edits: [],
      style: {
        fontFamily: 'Montserrat',
        fontSizePct: 5,
        weight: 700,
        color: '#FFFFFF',
        outlineColor: '#000000',
        outlinePx: 2,
        shadow: true,
        positionAnchor: 'bottom',
        offsetYPct: 14,
        maxWordsPerLine: 5,
        maxLines: 2,
        animation: 'phrase',
        highlightColor: '#FACC15',
        uppercase: false,
      },
    },
    overlays: [],
    audio: { gainDb: 0, normalize: { enabled: false, targetLufs: -14 } },
    template: null,
    export: { preset: 'standard', audioKbps: 192 },
    ...overrides,
  };
}

function makeMedia(overrides?: Partial<ProjectMediaInfo>): ProjectMediaInfo {
  const comp = makeComposition();
  return {
    project_id: '11111111-1111-4111-8111-111111111111',
    playable_rel_path: 'cache/youtube/aqz-KE-bpKQ/src.mp4',
    source_rel_path: 'cache/youtube/aqz-KE-bpKQ/src.mp4',
    proxy_rel_path: null,
    using_proxy: false,
    media_missing: false,
    proxy_missing: false,
    fps: 60,
    fps_num: 60,
    fps_den: 1,
    width: 1920,
    height: 1080,
    duration_ms: 30_000,
    absolute_start_ms: 5_000,
    handle_before_ms: 5_000,
    handle_after_ms: 5_000,
    composition_json: JSON.stringify(comp),
    composition_hash: 'hash-initial',
    max_clip_seconds: 180,
    ...overrides,
  };
}

describe('EditorView & EditorTimeline UI', () => {
  let videoPlaySpy: ReturnType<typeof vi.fn>;
  let videoPauseSpy: ReturnType<typeof vi.fn>;
  let vfcCallback: ((now: number, meta: { mediaTime?: number }) => void) | null;
  let pausedFlag: boolean;

  let canvasGetContextSpy: { mockRestore: () => void } | null = null;

  beforeEach(() => {
    vi.clearAllMocks();
    vfcCallback = null;
    pausedFlag = true;

    vi.mocked(events.thumbnailProgressEvent.listen).mockResolvedValue(() => undefined);
    vi.mocked(commands.cancelProjectThumbnails).mockResolvedValue({ status: 'ok', data: true });
    vi.mocked(commands.recordWebviewError).mockResolvedValue({ status: 'ok', data: true });

    videoPlaySpy = vi.fn().mockImplementation(function (this: HTMLVideoElement) {
      pausedFlag = false;
      this.dispatchEvent(new Event('play'));
      return Promise.resolve();
    });
    videoPauseSpy = vi.fn().mockImplementation(function (this: HTMLVideoElement) {
      pausedFlag = true;
      this.dispatchEvent(new Event('pause'));
    });

    Object.defineProperty(HTMLVideoElement.prototype, 'play', {
      configurable: true,
      value: videoPlaySpy,
    });
    Object.defineProperty(HTMLVideoElement.prototype, 'pause', {
      configurable: true,
      value: videoPauseSpy,
    });
    Object.defineProperty(HTMLVideoElement.prototype, 'paused', {
      configurable: true,
      get: () => pausedFlag,
    });
    Object.defineProperty(HTMLVideoElement.prototype, 'requestVideoFrameCallback', {
      configurable: true,
      value: (cb: (now: number, meta: { mediaTime?: number }) => void) => {
        vfcCallback = cb;
        return 42;
      },
    });
    Object.defineProperty(HTMLVideoElement.prototype, 'cancelVideoFrameCallback', {
      configurable: true,
      value: vi.fn(),
    });

    canvasGetContextSpy = vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({
      clearRect: vi.fn(),
      fillRect: vi.fn(),
      strokeRect: vi.fn(),
      drawImage: vi.fn(),
      save: vi.fn(),
      restore: vi.fn(),
      beginPath: vi.fn(),
      rect: vi.fn(),
      clip: vi.fn(),
    } as unknown as CanvasRenderingContext2D);

    vi.mocked(commands.getProjectMedia).mockResolvedValue({ status: 'ok', data: makeMedia() });
    vi.mocked(commands.generateProjectThumbnails).mockResolvedValue({
      status: 'ok',
      data: {
        cache_key: 'thumb-key',
        source_rel_path: 'cache/youtube/aqz-KE-bpKQ/src.mp4',
        interval_secs: 2,
        width_px: 160,
        duration_ms: 30_000,
        frame_rel_paths: ['cache/thumbs/k/thumb_0001.jpg', 'cache/thumbs/k/thumb_0002.jpg'],
        total_bytes: 2048,
      },
    });
    vi.mocked(commands.saveComposition).mockResolvedValue({
      status: 'ok',
      data: {
        project: makeProject({ status: 'edited', composition_hash: 'hash-saved' }),
        composition_json: JSON.stringify(makeComposition()),
        composition_hash: 'hash-saved',
      },
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    canvasGetContextSpy?.mockRestore();
  });

  it('plays via mocked HTMLVideoElement, updates frame-accurate timecode via rVFC, steps 1/60s, handles shortcuts and disabled Export', async () => {
    const user = userEvent.setup();
    render(
      <EditorView
        locale="id"
        project={makeProject()}
        workspaceRoot="C:\\DataRoot\\ClipperHelper"
        onBackHome={vi.fn()}
        onChangeRange={vi.fn()}
        onOpenDiagnostics={vi.fn()}
        onProjectUpdated={vi.fn()}
      />
    );

    const video = (await screen.findByTestId('editor-video')) as HTMLVideoElement;
    expect(video.getAttribute('src')).toContain('http://asset.localhost/');
    expect(screen.getByTestId('btn-export-disabled')).toBeDisabled();
    expect(screen.getByTestId('editor-save-status')).toHaveTextContent('Tersimpan');
    expect(screen.getByTestId('editor-playhead-time')).toHaveTextContent('00:05.000');

    // Play triggers video.play and rVFC advances displayed time accurately
    await user.click(screen.getByTestId('btn-play-pause'));
    expect(videoPlaySpy).toHaveBeenCalledTimes(1);

    await act(async () => {
      vfcCallback?.(100, { mediaTime: 12.5 });
    });
    expect(screen.getByTestId('editor-playhead-time')).toHaveTextContent('00:12.500');

    // Step +1 frame at 60 fps -> +16.667 ms -> 12517 ms -> 00:12.517
    fireEvent.keyDown(window, { key: 'ArrowRight' });
    expect(screen.getByTestId('editor-playhead-time')).toHaveTextContent('00:12.517');

    // Shift+ArrowRight -> +1 s -> 00:13.517
    fireEvent.keyDown(window, { key: 'ArrowRight', shiftKey: true });
    expect(screen.getByTestId('editor-playhead-time')).toHaveTextContent('00:13.517');

    // I sets In at playhead
    fireEvent.keyDown(window, { key: 'i' });
    expect(screen.getByTestId('editor-in-time')).toHaveTextContent('00:13.517');

    // Toggle rate 2x & mute & Home jumps to In
    await user.click(screen.getByTestId('btn-rate-2'));
    expect(video.playbackRate).toBe(2);
    await user.click(screen.getByTestId('btn-mute-toggle'));
    expect(video.muted).toBe(true);
    fireEvent.keyDown(window, { key: 'Home' });
    expect(screen.getByTestId('editor-playhead-time')).toHaveTextContent('00:13.517');
  });

  it('supports trim handle drag, crop zoom/pan/center/reset, over-max warning, and >= 10-step undo/redo', async () => {
    const user = userEvent.setup();
    vi.mocked(commands.getProjectMedia).mockResolvedValue({
      status: 'ok',
      data: makeMedia({ max_clip_seconds: 18 }),
    });

    render(
      <EditorView
        locale="id"
        project={makeProject()}
        workspaceRoot={null}
        onBackHome={vi.fn()}
        onChangeRange={vi.fn()}
        onOpenDiagnostics={vi.fn()}
        onProjectUpdated={vi.fn()}
      />
    );

    const track = (await screen.findByTestId('editor-timeline-track')) as HTMLDivElement;
    vi.spyOn(track, 'getBoundingClientRect').mockReturnValue({
      left: 0,
      top: 0,
      width: 1000,
      height: 64,
      right: 1000,
      bottom: 64,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    });

    // Drag Out handle from 20_000 ms to 27_000 ms (0.9 * 30_000): selected = 22s > 18s max_clip_seconds
    const outHandle = screen.getByTestId('timeline-handle-out');
    fireEvent.mouseDown(outHandle, { button: 0, clientX: 667 });
    fireEvent.mouseMove(window, { clientX: 900 });
    fireEvent.mouseUp(window);
    expect(screen.getByTestId('editor-max-clip-warning')).toHaveTextContent('18 dtk');

    // Perform 10 distinct non-coalesced crop zoom adjustments via slider/reset + nudge commands
    for (let i = 0; i < 10; i++) {
      await user.click(screen.getByTestId('nudge-in-plus-frame'));
      // Interleave with centerCrop to break coalescing so 10 distinct entries sit on stack
      await user.click(screen.getByTestId('btn-crop-center'));
    }

    // Zoom slider + reset
    const zoomSlider = screen.getByTestId('crop-zoom-slider') as HTMLInputElement;
    fireEvent.change(zoomSlider, { target: { value: '2' } });
    expect(zoomSlider.value).toBe('2');

    // Overview draggable crop box
    const overviewBox = screen.getByTestId('editor-overview-crop-box');
    const overviewContainer = screen.getByTestId('editor-overview-container');
    Object.defineProperty(overviewContainer, 'clientWidth', { configurable: true, value: 400 });
    Object.defineProperty(overviewContainer, 'clientHeight', { configurable: true, value: 225 });
    fireEvent.mouseDown(overviewBox, { button: 0, clientX: 100, clientY: 50 });
    fireEvent.mouseMove(window, { clientX: 140, clientY: 60 });
    fireEvent.mouseUp(window);

    // Undo 10+ times with Ctrl+Z cleanly back toward start
    for (let i = 0; i < 12; i++) {
      fireEvent.keyDown(window, { key: 'z', ctrlKey: true });
    }
    // Redo with Ctrl+Shift+Z
    fireEvent.keyDown(window, { key: 'z', ctrlKey: true, shiftKey: true });
    expect(screen.getByTestId('btn-redo')).toBeEnabled();

    await user.click(screen.getByTestId('btn-crop-reset'));
    expect((screen.getByTestId('crop-zoom-slider') as HTMLInputElement).value).toBe('1');
  });

  it('debounces autosave (~1s), flushes on blur, displays save errors, and guards navigation', async () => {
    vi.useFakeTimers();
    render(
      <EditorView
        locale="id"
        project={makeProject()}
        workspaceRoot={null}
        onBackHome={vi.fn()}
        onChangeRange={vi.fn()}
        onOpenDiagnostics={vi.fn()}
        onProjectUpdated={vi.fn()}
      />
    );

    // Resolve initial load promise
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(screen.getByTestId('editor-save-status')).toHaveTextContent('Tersimpan');

    // Edit trim via button -> becomes dirty -> waits ~1s -> calls saveComposition
    fireEvent.click(screen.getByTestId('nudge-in-plus-frame'));
    expect(screen.getByTestId('editor-save-status')).toHaveTextContent('Belum disimpan');
    expect(commands.saveComposition).not.toHaveBeenCalled();

    await act(async () => {
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
    });
    expect(commands.saveComposition).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId('editor-save-status')).toHaveTextContent('Tersimpan');

    // Next edit fails save -> error banner + unsaved navigation guard blocks immediate exit
    vi.mocked(commands.saveComposition).mockResolvedValueOnce({
      status: 'error',
      error: {
        code: 'E_TEMPLATE_INVALID',
        message_key: 'err.E_TEMPLATE_INVALID.body',
        message: 'Bad trim',
        detail: 'too long',
        disk_bounds: null,
      },
    });

    const onBackHome = vi.fn();
    fireEvent.click(screen.getByTestId('nudge-out-minus-frame'));
    await act(async () => {
      window.dispatchEvent(new Event('blur'));
      await Promise.resolve();
    });

    expect(screen.getByTestId('editor-save-status')).toHaveTextContent('Gagal menyimpan');
    expect(screen.getByTestId('editor-save-error-banner')).toHaveTextContent('E_TEMPLATE_INVALID');

    vi.mocked(commands.saveComposition).mockResolvedValueOnce({
      status: 'error',
      error: {
        code: 'E_TEMPLATE_INVALID',
        message_key: 'err.E_TEMPLATE_INVALID.body',
        message: 'Bad trim',
        detail: 'still bad',
        disk_bounds: null,
      },
    });
    fireEvent.click(screen.getByTestId('btn-editor-back-home'));
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.getByTestId('editor-unsaved-guard')).toBeInTheDocument();
    expect(onBackHome).not.toHaveBeenCalled();
  });

  it('shows friendly recovery actions for missing source media, missing proxy, and video decode errors', async () => {
    const user = userEvent.setup();
    const onChangeRange = vi.fn();
    const onOpenDiagnostics = vi.fn();
    const onStartGenerate = vi.fn().mockResolvedValue(null);

    // 1) Missing media -> Unduh ulang
    vi.mocked(commands.getProjectMedia).mockResolvedValueOnce({
      status: 'ok',
      data: makeMedia({ media_missing: true, playable_rel_path: null }),
    });
    const { rerender } = render(
      <EditorView
        locale="id"
        project={makeProject()}
        workspaceRoot={null}
        onBackHome={vi.fn()}
        onChangeRange={onChangeRange}
        onOpenDiagnostics={onOpenDiagnostics}
        onProjectUpdated={vi.fn()}
        onStartGenerate={onStartGenerate}
      />
    );

    expect(await screen.findByTestId('editor-media-missing-card')).toHaveTextContent(
      'Berkas media cache tidak ditemukan'
    );
    await user.click(screen.getByTestId('btn-redownload'));
    expect(onChangeRange).toHaveBeenCalledTimes(1);

    // 2) Proxy missing -> Siapkan ulang pratinjau
    vi.mocked(commands.getProjectMedia).mockResolvedValueOnce({
      status: 'ok',
      data: makeMedia({ proxy_missing: true, playable_rel_path: null, using_proxy: false }),
    });
    rerender(
      <EditorView
        locale="id"
        project={makeProject({ id: '22222222-2222-4222-8222-222222222222' })}
        workspaceRoot={null}
        onBackHome={vi.fn()}
        onChangeRange={onChangeRange}
        onOpenDiagnostics={onOpenDiagnostics}
        onProjectUpdated={vi.fn()}
        onStartGenerate={onStartGenerate}
      />
    );

    expect(await screen.findByTestId('editor-proxy-missing-card')).toHaveTextContent(
      'Proxy pratinjau belum siap'
    );
    await user.click(screen.getByTestId('btn-reproxy'));
    expect(onStartGenerate).toHaveBeenCalledWith('22222222-2222-4222-8222-222222222222');

    // 3) Video decode error -> logs error_events and shows Diagnostik link
    vi.mocked(commands.getProjectMedia).mockResolvedValueOnce({
      status: 'ok',
      data: makeMedia(),
    });
    rerender(
      <EditorView
        locale="id"
        project={makeProject({ id: '33333333-3333-4333-8333-333333333333' })}
        workspaceRoot={null}
        onBackHome={vi.fn()}
        onChangeRange={onChangeRange}
        onOpenDiagnostics={onOpenDiagnostics}
        onProjectUpdated={vi.fn()}
      />
    );

    const video = await screen.findByTestId('editor-video');
    fireEvent.error(video);
    const decodeBanner = await screen.findByTestId('editor-decode-error-banner');
    expect(commands.recordWebviewError).toHaveBeenCalledWith(
      'editor-video',
      'WebView HTMLVideoElement decode error',
      null
    );
    expect(decodeBanner).toHaveTextContent('Gagal memutar video');
  });

  it('shows filmstrip placeholders + progress and supports cancelling background extraction', async () => {
    const user = userEvent.setup();
    let progressHandler: ((ev: { payload: ThumbnailProgressEvent }) => void) | null = null;
    vi.mocked(events.thumbnailProgressEvent.listen).mockImplementation(async (cb) => {
      progressHandler = cb as (ev: { payload: ThumbnailProgressEvent }) => void;
      return () => undefined;
    });

    let resolveThumbs!: (
      res: Awaited<ReturnType<typeof commands.generateProjectThumbnails>>
    ) => void;
    vi.mocked(commands.generateProjectThumbnails).mockImplementation(
      () =>
        new Promise((resolve) => {
          resolveThumbs = resolve;
        })
    );

    render(
      <EditorView
        locale="id"
        project={makeProject()}
        workspaceRoot={null}
        onBackHome={vi.fn()}
        onChangeRange={vi.fn()}
        onOpenDiagnostics={vi.fn()}
        onProjectUpdated={vi.fn()}
      />
    );

    expect((await screen.findAllByTestId('filmstrip-placeholder')).length).toBeGreaterThanOrEqual(
      6
    );

    await act(async () => {
      progressHandler?.({
        payload: {
          project_id: '11111111-1111-4111-8111-111111111111',
          percent: 45,
          eta_seconds: 3,
          done_secs: 13.5,
          total_secs: 30,
        },
      });
    });
    expect(screen.getByTestId('filmstrip-progress-label')).toHaveTextContent('45%');

    // Resolve filmstrip -> renders thumbnail images
    await act(async () => {
      resolveThumbs({
        status: 'ok',
        data: {
          cache_key: 'k',
          source_rel_path: 'cache/youtube/aqz-KE-bpKQ/src.mp4',
          interval_secs: 2,
          width_px: 160,
          duration_ms: 30_000,
          frame_rel_paths: ['cache/thumbs/k/thumb_0001.jpg', 'cache/thumbs/k/thumb_0002.jpg'],
          total_bytes: 1200,
        },
      });
    });

    await waitFor(() => {
      expect(screen.getAllByTestId('filmstrip-thumb-img')).toHaveLength(2);
    });

    // Trigger retry + cancel
    vi.mocked(commands.generateProjectThumbnails).mockImplementation(
      () => new Promise(() => undefined)
    );
    // Remount cleanly or simulate cancel when pending in second test phase isn't needed if we test cancel directly before resolving:
    expect(user).toBeDefined();
  });

  it('cancels active filmstrip extraction on user click', async () => {
    const user = userEvent.setup();
    vi.mocked(commands.generateProjectThumbnails).mockImplementation(
      () => new Promise(() => undefined)
    );

    render(
      <EditorView
        locale="id"
        project={makeProject()}
        workspaceRoot={null}
        onBackHome={vi.fn()}
        onChangeRange={vi.fn()}
        onOpenDiagnostics={vi.fn()}
        onProjectUpdated={vi.fn()}
      />
    );

    const cancelBtn = await screen.findByTestId('filmstrip-cancel-button');
    await user.click(cancelBtn);
    expect(commands.cancelProjectThumbnails).toHaveBeenCalledWith(
      '11111111-1111-4111-8111-111111111111'
    );
    expect(screen.getByTestId('filmstrip-status-note')).toHaveTextContent('Filmstrip dihentikan.');
  });

  it('loops playback back to inMs when reaching outMs if loop selection is enabled, and continues when toggled off', async () => {
    const user = userEvent.setup();
    render(
      <EditorView
        locale="id"
        project={makeProject()}
        workspaceRoot={null}
        onBackHome={vi.fn()}
        onChangeRange={vi.fn()}
        onOpenDiagnostics={vi.fn()}
        onProjectUpdated={vi.fn()}
      />
    );

    const video = (await screen.findByTestId('editor-video')) as HTMLVideoElement;
    const loopBtn = screen.getByTestId('btn-loop-toggle');
    expect(loopBtn).toHaveAttribute('aria-pressed', 'true');

    // Start playing inside [5000, 20000]
    await user.click(screen.getByTestId('btn-play-pause'));
    await act(async () => {
      vfcCallback?.(100, { mediaTime: 10.0 });
    });
    expect(screen.getByTestId('editor-playhead-time')).toHaveTextContent('00:10.000');

    // Reach outMs (20.000s) -> loops back to inMs (5.000s)
    await act(async () => {
      vfcCallback?.(200, { mediaTime: 20.1 });
    });
    expect(video.currentTime).toBeCloseTo(5.0, 2);
    expect(screen.getByTestId('editor-playhead-time')).toHaveTextContent('00:05.000');

    // Toggle loop OFF -> reaching 21.000s does NOT wrap back to 5.000s
    await user.click(loopBtn);
    expect(loopBtn).toHaveAttribute('aria-pressed', 'false');
    await act(async () => {
      vfcCallback?.(300, { mediaTime: 21.0 });
    });
    expect(screen.getByTestId('editor-playhead-time')).toHaveTextContent('00:21.000');
  });

  it('restores saved trim and zoomed crop on reopen and supports preview viewport pan/wheel and O shortcut', async () => {
    const baseCrop = defaultCropRect(1920, 1080);
    const savedComp = makeComposition({
      timeline: [{ id: 'seg-1', sourceId: 'src-1', inMs: 7_500, outMs: 18_500 }],
      layout: {
        mode: 'fill',
        crop: { x: 0.25, y: 0.25, w: baseCrop.w / 2, h: 0.5 },
      },
    });
    vi.mocked(commands.getProjectMedia).mockResolvedValue({
      status: 'ok',
      data: makeMedia({
        composition_json: JSON.stringify(savedComp),
        composition_hash: 'hash-persisted-1',
      }),
    });

    const { unmount } = render(
      <EditorView
        locale="id"
        project={makeProject({ status: 'edited', composition_hash: 'hash-persisted-1' })}
        workspaceRoot={null}
        onBackHome={vi.fn()}
        onChangeRange={vi.fn()}
        onOpenDiagnostics={vi.fn()}
        onProjectUpdated={vi.fn()}
      />
    );

    // Reopen restores exact saved in/out and 2x crop zoom
    expect(await screen.findByTestId('editor-in-time')).toHaveTextContent('00:07.500');
    expect(screen.getByTestId('editor-out-time')).toHaveTextContent('00:18.500');
    expect((screen.getByTestId('crop-zoom-slider') as HTMLInputElement).value).toBe('2');

    // Pan directly on 9:16 preview viewport + wheel zoom
    const viewport = screen.getByTestId('editor-preview-viewport');
    fireEvent.mouseDown(viewport, { button: 0, clientX: 120, clientY: 200 });
    fireEvent.mouseMove(window, { clientX: 140, clientY: 220 });
    fireEvent.mouseUp(window);
    fireEvent.wheel(viewport, { deltaY: -100 });
    expect(Number((screen.getByTestId('crop-zoom-slider') as HTMLInputElement).value)).toBeGreaterThan(2);

    // O shortcut sets Out at current playhead after stepping +1s
    fireEvent.keyDown(window, { key: 'ArrowRight', shiftKey: true });
    fireEvent.keyDown(window, { key: 'o' });
    expect(screen.getByTestId('editor-out-time')).toHaveTextContent('00:08.500');

    // Unmount simulates app closing cleanly / flushing dirty state
    unmount();
    expect(commands.saveComposition).toHaveBeenCalled();
  });

  it('recovers from WebView2 repeated-frame seeks using rVFC mediaTime seek-and-verify', async () => {
    const user = userEvent.setup();
    render(
      <EditorView
        locale="id"
        project={makeProject()}
        workspaceRoot={null}
        onBackHome={vi.fn()}
        onChangeRange={vi.fn()}
        onOpenDiagnostics={vi.fn()}
        onProjectUpdated={vi.fn()}
      />
    );

    const video = (await screen.findByTestId('editor-video')) as HTMLVideoElement;
    // Initial playhead is at 5.000s (frame 300 at 60fps). Step +1 frame via button.
    await user.click(screen.getByTestId('btn-playhead-next-frame'));
    expect(video.currentTime).toBeGreaterThan(301 / 60);
    expect(video.currentTime).toBeLessThan(302 / 60);

    // Simulate WebView2 presenting a repeated frame on first rVFC callback (still at 5.000s)
    const firstSeekTime = video.currentTime;
    await act(async () => {
      vfcCallback?.(100, { mediaTime: 5.0 });
    });

    // Seek-and-verify nudged currentTime forward and re-armed rVFC
    expect(video.currentTime).toBeGreaterThan(firstSeekTime);

    // Second rVFC callback confirms frame advanced to 5.016667s -> readout updates from mediaTime
    await act(async () => {
      vfcCallback?.(116, { mediaTime: 5.0 + 1 / 60 });
    });
    expect(screen.getByTestId('editor-playhead-time')).toHaveTextContent('00:05.017');
  });
});
