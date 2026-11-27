import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { openUrl } from '@tauri-apps/plugin-opener';
import { RangePickerView } from '../components/RangePickerView';
import { commands, PreviewConfigResponse, ProjectRecord } from '../ipc/bindings';
import {
  BridgeSession,
  extrapolatePlaybackTime,
  HostOutboundCommand,
  isAdDurationMismatch,
} from '../utils/bridgeClient';
import {
  buildAllowedYouTubeWatchUrl,
  estimateDownloadSeconds,
  parseTimecode,
  validateClipRange,
} from '../utils/timecode';

vi.mock('@tauri-apps/plugin-opener', () => ({
  openUrl: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('../ipc/bindings', async () => {
  const actual = await vi.importActual<typeof import('../ipc/bindings')>('../ipc/bindings');
  return {
    ...actual,
    commands: {
      ...actual.commands,
      updateProjectRange: vi.fn(),
      recordPreviewEvent: vi.fn().mockResolvedValue({
        status: 'ok',
        data: { embed_disabled_count: 0, fallback_count: 0, config_error_count: 0 },
      }),
    },
  };
});

function buildProject(overrides?: Partial<ProjectRecord>): ProjectRecord {
  return {
    id: '11111111-1111-4111-8111-111111111111',
    title: 'Podcast Livestream 3 Jam',
    channel: 'Channel Mantap',
    source_provider: 'youtube',
    source_video_id: '9s6imDGjy08',
    source_url: 'https://www.youtube.com/watch?v=9s6imDGjy08',
    duration_seconds: 13_668,
    live_status: 'was_live',
    preview_mode: 'embed',
    suggested_start_seconds: 90,
    range_start_ms: null,
    range_end_ms: null,
    status: 'probed',
    created_at: 1_700_000_000,
    updated_at: 1_700_000_000,
    ...overrides,
  };
}

function buildPreviewConfig(overrides?: Partial<PreviewConfigResponse>): PreviewConfigResponse {
  return {
    bridge_url: 'https://clipper-bridge.vercel.app',
    bridge_protocol_version: 1,
    control_video_id: 'WufkeDaJisQ',
    ready_timeout_ms: 6000,
    max_range_seconds: 300,
    download_prior_rt_factor: 1.66,
    download_prior_startup_seconds: 10,
    can_force_timecode_only: false,
    ...overrides,
  };
}

describe('Timecode parsing, validation & download estimate helpers', () => {
  it('parses table-driven timecode formats cleanly and rejects invalid/negative/overflow', () => {
    const cases: Array<[string, number | null]> = [
      ['1:02:03', 3723],
      ['62:03', 3723],
      ['3723', 3723],
      ['1:02:03.5', 3723.5],
      ['0', 0],
      ['00:05:30', 330],
      ['abc', null],
      ['-5', null],
      ['-00:10', null],
      ['01:70:00', null],
      ['01:02:65', null],
      ['1:2:3:4', null],
      ['999999', null],
      ['', null],
    ];
    for (const [raw, expected] of cases) {
      expect(parseTimecode(raw), `parseTimecode(${raw})`).toBe(expected);
    }
  });

  it('validates clip range bounds and estimates download seconds (range/1.66 + 10)', () => {
    expect(
      validateClipRange({
        startSeconds: 10,
        endSeconds: 130,
        videoDurationSeconds: 600,
        maxRangeSeconds: 300,
      })
    ).toEqual({
      valid: true,
      startSeconds: 10,
      endSeconds: 130,
      durationSeconds: 120,
    });

    expect(
      validateClipRange({
        startSeconds: 100,
        endSeconds: 90,
        videoDurationSeconds: 600,
        maxRangeSeconds: 300,
      })
    ).toMatchObject({ valid: false, code: 'E_RANGE_INVALID' });

    expect(
      validateClipRange({
        startSeconds: 10,
        endSeconds: 10.5,
        videoDurationSeconds: 600,
        maxRangeSeconds: 300,
      })
    ).toMatchObject({ valid: false, code: 'E_RANGE_INVALID' });

    expect(
      validateClipRange({
        startSeconds: 0,
        endSeconds: 350,
        videoDurationSeconds: 600,
        maxRangeSeconds: 300,
      })
    ).toMatchObject({ valid: false, code: 'E_RANGE_TOO_LONG' });

    expect(estimateDownloadSeconds(166, 1.66, 10)).toBe(110);
  });

  it('builds strictly restricted external YouTube watch URLs only', () => {
    expect(buildAllowedYouTubeWatchUrl('dQw4w9WgXcQ', 95)).toBe(
      'https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=95'
    );
    expect(buildAllowedYouTubeWatchUrl('dQw4w9WgXcQ', 0)).toBe(
      'https://www.youtube.com/watch?v=dQw4w9WgXcQ'
    );
    expect(() => buildAllowedYouTubeWatchUrl('../bad')).toThrow();
  });
});

describe('BridgeSession & extrapolation protocol logic', () => {
  it('extrapolates time smoothly between 4-17 Hz updates while playing', () => {
    expect(
      extrapolatePlaybackTime({
        lastCurrentSeconds: 10,
        lastTimestampMs: 1000,
        rate: 1.5,
        nowMs: 1200,
        isPlaying: true,
        durationSeconds: 600,
      })
    ).toBeCloseTo(10.3, 3);

    expect(
      extrapolatePlaybackTime({
        lastCurrentSeconds: 10,
        lastTimestampMs: 1000,
        rate: 1.0,
        nowMs: 1500,
        isPlaying: false,
        durationSeconds: 600,
      })
    ).toBe(10);
  });

  it('detects ad playback by duration mismatch (> 2 s) and differentiates YT 101/150 using control video', () => {
    expect(isAdDurationMismatch(15, 600)).toBe(true);
    expect(isAdDurationMismatch(601.5, 600)).toBe(false);

    vi.useFakeTimers();
    const sent: Array<{ cmd: HostOutboundCommand; targetOrigin: string }> = [];
    const fallbacks: unknown[] = [];
    const fakeSource = { tag: 'iframe' };

    const session = new BridgeSession({
      targetOrigin: 'https://clipper-bridge.vercel.app',
      expectedProtocolVersion: 1,
      readyTimeoutMs: 6000,
      controlVideoId: 'WufkeDaJisQ',
      videoId: '9s6imDGjy08',
      probeDurationSeconds: 600,
      nonce: 'nonce1234',
      getIframeSource: () => fakeSource,
      postToIframe: (cmd, targetOrigin) => sent.push({ cmd, targetOrigin }),
      onFallback: (info) => fallbacks.push(info),
    });

    session.startHandshake();
    expect(sent[0]).toEqual({
      cmd: { type: 'hello', v: 1, nonce: 'nonce1234' },
      targetOrigin: 'https://clipper-bridge.vercel.app',
    });

    // Ready response completes handshake & loads primary video
    session.handleMessageEvent({
      origin: 'https://clipper-bridge.vercel.app',
      source: fakeSource,
      data: { type: 'ready', v: 1, nonce: 'nonce1234' },
    });
    expect(sent[1]?.cmd).toMatchObject({ type: 'load', videoId: '9s6imDGjy08' });

    // Primary YT 150 triggers control-video verification
    session.handleMessageEvent({
      origin: 'https://clipper-bridge.vercel.app',
      source: fakeSource,
      data: { type: 'error', code: 150 },
    });
    expect(sent[2]?.cmd).toMatchObject({ type: 'load', videoId: 'WufkeDaJisQ' });
    expect(fallbacks).toHaveLength(0);

    // Control video fails => OUR config error -> E_PREVIEW_CONFIG
    session.handleMessageEvent({
      origin: 'https://clipper-bridge.vercel.app',
      source: fakeSource,
      data: { type: 'error', code: 150 },
    });
    expect(fallbacks[0]).toMatchObject({
      reason: 'config_error_control',
      errorCode: 'E_PREVIEW_CONFIG',
      counterKind: 'config_error',
    });

    session.dispose();
    vi.useRealTimers();
  });
});

describe('RangePickerView UI (player, fallback, ads, nudges, loop, persistence)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  function setupBridgeMessageResponder(project: ProjectRecord, config: PreviewConfigResponse) {
    const sentCommands: HostOutboundCommand[] = [];
    const origin = new URL(config.bridge_url).origin;
    let lastNonce = '';
    let fakeIframeWindow: Record<string, unknown> | null = null;

    const mountResult = render(
      <RangePickerView
        locale="id"
        project={project}
        previewConfig={config}
        onBackHome={vi.fn()}
        onOpenDiagnostics={vi.fn()}
        onProjectUpdated={vi.fn()}
      />
    );

    const iframe = screen.queryByTitle('YouTube Preview Player') as HTMLIFrameElement | null;
    if (iframe) {
      fakeIframeWindow = {
        postMessage: vi.fn((cmd: HostOutboundCommand, targetOrigin: string) => {
          expect(targetOrigin).toBe(origin);
          sentCommands.push(cmd);
          if (cmd.type === 'hello') {
            lastNonce = cmd.nonce;
          }
        }),
      };
      Object.defineProperty(iframe, 'contentWindow', {
        configurable: true,
        value: fakeIframeWindow,
      });
    }

    const sendFromBridge = (payload: Record<string, unknown>) => {
      act(() => {
        window.dispatchEvent(
          new MessageEvent('message', {
            origin,
            source: fakeIframeWindow as unknown as MessageEventSource,
            data: payload,
          })
        );
      });
    };

    return {
      ...mountResult,
      sentCommands,
      sendFromBridge,
      completeHandshake: (readyMs = 200) => {
        if (iframe) {
          act(() => {
            iframe.dispatchEvent(new Event('load'));
          });
        }
        expect(lastNonce).not.toBe('');
        sendFromBridge({ type: 'ready', v: 1, nonce: lastNonce });
        sendFromBridge({ type: 'state', state: 'ready' });
        void readyMs;
      },
    };
  }

  it('renders available bridge player cleanly, supports nudges/set/loop, and persists saved range', async () => {
    const user = userEvent.setup();
    const proj = buildProject();
    const cfg = buildPreviewConfig();
    vi.mocked(commands.updateProjectRange).mockResolvedValue({
      status: 'ok',
      data: { ...proj, range_start_ms: 119_000, range_end_ms: 155_000, status: 'ranged' },
    });

    const { sentCommands, sendFromBridge, completeHandshake } = setupBridgeMessageResponder(
      proj,
      cfg
    );

    const frameBox = screen.getByTestId('bridge-player-frame-container');
    expect(frameBox.className).toContain('aspect-video');
    expect(frameBox.className).toContain('min-w-[200px]');
    // Policy compliance: zero custom overlay buttons inside player container
    expect(frameBox.querySelectorAll('button')).toHaveLength(0);

    completeHandshake();

    // Bridge emits paused + time at 120.0s for deterministic Set start check
    sendFromBridge({ type: 'state', state: 'paused' });
    sendFromBridge({
      type: 'time',
      current: 120,
      duration: 13_668,
      rate: 1,
      ts: Date.now(),
    });

    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Set mulai di sini' })).toBeEnabled();
    });

    // Click "Set mulai di sini" + nudge "-1s" => 00:01:59
    await user.click(screen.getByRole('button', { name: 'Set mulai di sini' }));
    expect((screen.getByLabelText('Waktu mulai (HH:MM:SS)') as HTMLInputElement).value).toBe(
      '00:02:00'
    );
    await user.click(screen.getByRole('button', { name: 'Mulai -1 detik' }));
    expect((screen.getByLabelText('Waktu mulai (HH:MM:SS)') as HTMLInputElement).value).toBe(
      '00:01:59'
    );

    // Advance player time to 155s and click "Set akhir di sini" => duration 36s, estimate updates
    sendFromBridge({
      type: 'time',
      current: 155,
      duration: 13_668,
      rate: 1,
      ts: Date.now(),
    });
    await user.click(screen.getByRole('button', { name: 'Set akhir di sini' }));
    expect((screen.getByLabelText('Waktu akhir (HH:MM:SS)') as HTMLInputElement).value).toBe(
      '00:02:35'
    );
    expect(screen.getByTestId('download-estimate-text')).toHaveTextContent(/Perkiraan unduh/);

    // Loop range seeks to start (119s) and plays; reaching end loops back to 119s
    await user.click(screen.getByRole('button', { name: 'Putar rentang' }));
    expect(sentCommands.some((c) => c.type === 'seek' && c.seconds === 119)).toBe(true);
    expect(sentCommands.some((c) => c.type === 'play')).toBe(true);

    sendFromBridge({ type: 'state', state: 'playing' });
    sendFromBridge({
      type: 'time',
      current: 156,
      duration: 13_668,
      rate: 1,
      ts: Date.now(),
    });
    await waitFor(
      () => {
        const seeksToStart = sentCommands.filter((c) => c.type === 'seek' && c.seconds === 119);
        expect(seeksToStart.length).toBeGreaterThanOrEqual(2);
      },
      { timeout: 1000 }
    );

    // Save range updates SQLite via typed IPC
    await user.click(screen.getByRole('button', { name: 'Simpan rentang' }));
    await waitFor(() => {
      expect(commands.updateProjectRange).toHaveBeenCalledWith(proj.id, 119_000, 155_000);
      expect(screen.getByTestId('range-saved-notice')).toBeInTheDocument();
    });

    // "Lanjut ke unduh" remains disabled with "Segera hadir"
    const nextDownloadBtn = screen.getByRole('button', { name: 'Lanjut ke unduh' });
    expect(nextDownloadBtn).toBeDisabled();
    expect(screen.getByTestId('range-picker-view')).toHaveTextContent('Segera hadir');
  });

  it('switches to timecode-only on 6s bridge timeout and opens restricted YouTube URL', async () => {
    vi.useFakeTimers();
    const proj = buildProject({ suggested_start_seconds: 120 });
    const cfg = buildPreviewConfig({ ready_timeout_ms: 6000 });

    render(
      <RangePickerView
        locale="id"
        project={proj}
        previewConfig={cfg}
        onBackHome={vi.fn()}
        onOpenDiagnostics={vi.fn()}
        onProjectUpdated={vi.fn()}
      />
    );

    act(() => {
      vi.advanceTimersByTime(6100);
    });

    vi.useRealTimers();
    const user = userEvent.setup();
    const fallbackPanel = await screen.findByTestId('timecode-only-panel');
    expect(fallbackPanel).toHaveTextContent('Pratinjau tidak tersedia untuk video ini');
    expect(screen.queryByTestId('preview-error-banner')).not.toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Buka video di browser' }));
    expect(openUrl).toHaveBeenCalledWith('https://www.youtube.com/watch?v=9s6imDGjy08&t=120');
  });

  it('distinguishes owner-restricted 101/150 (no error banner) from config error 153 (E_PREVIEW_CONFIG banner)', async () => {
    // 1. Owner restricted (control video succeeds -> info fallback only, no E_PREVIEW_CONFIG banner)
    const proj1 = buildProject();
    const cfg = buildPreviewConfig();
    const r1 = setupBridgeMessageResponder(proj1, cfg);
    r1.completeHandshake();

    r1.sendFromBridge({ type: 'error', code: 150 });
    expect(r1.sentCommands.some((c) => c.type === 'load' && c.videoId === 'WufkeDaJisQ')).toBe(
      true
    );

    r1.sendFromBridge({ type: 'state', state: 'ready' });
    await waitFor(() => {
      expect(screen.getByTestId('timecode-only-panel')).toHaveTextContent(
        'Pemilik video membatasi pemutaran embed'
      );
    });
    expect(screen.queryByTestId('preview-error-banner')).not.toBeInTheDocument();
    expect(commands.recordPreviewEvent).toHaveBeenCalledWith(
      expect.objectContaining({
        fallback_reason: 'embed_disabled',
        counter_kind: 'embed_disabled',
        record_error_code: null,
      })
    );

    r1.unmount();

    // 2. YouTube 153 => E_PREVIEW_CONFIG banner + fallback
    const r2 = setupBridgeMessageResponder(proj1, cfg);
    r2.completeHandshake();
    r2.sendFromBridge({ type: 'error', code: 153 });

    const errBanner = await screen.findByTestId('preview-error-banner');
    expect(errBanner).toHaveTextContent('E_PREVIEW_CONFIG');
    expect(commands.recordPreviewEvent).toHaveBeenCalledWith(
      expect.objectContaining({
        fallback_reason: 'config_error_153',
        counter_kind: 'config_error',
        record_error_code: 'E_PREVIEW_CONFIG',
      })
    );
  });

  it('disables Set start/end buttons while an ad duration mismatch (> 2s) is detected', async () => {
    const proj = buildProject({ duration_seconds: 600 });
    const cfg = buildPreviewConfig();
    const { sendFromBridge, completeHandshake } = setupBridgeMessageResponder(proj, cfg);
    completeHandshake();

    sendFromBridge({ type: 'state', state: 'playing' });
    sendFromBridge({ type: 'time', current: 5, duration: 15, rate: 1, ts: Date.now() });

    const adNotice = await screen.findByTestId('ad-notice-banner');
    expect(adNotice).toHaveTextContent('Iklan sedang diputar');
    expect(screen.getByRole('button', { name: 'Set mulai di sini' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Set akhir di sini' })).toBeDisabled();
  });

  it('restores saved range boundaries cleanly when reopening project from recent list', () => {
    const reopened = buildProject({
      status: 'ranged',
      range_start_ms: 400_000,
      range_end_ms: 525_000,
    });
    render(
      <RangePickerView
        locale="id"
        project={reopened}
        previewConfig={buildPreviewConfig()}
        onBackHome={vi.fn()}
        onOpenDiagnostics={vi.fn()}
        onProjectUpdated={vi.fn()}
      />
    );

    expect((screen.getByLabelText('Waktu mulai (HH:MM:SS)') as HTMLInputElement).value).toBe(
      '00:06:40'
    );
    expect((screen.getByLabelText('Waktu akhir (HH:MM:SS)') as HTMLInputElement).value).toBe(
      '00:08:45'
    );
  });
});
