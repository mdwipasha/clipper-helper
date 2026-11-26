import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ComponentsView } from '../components/ComponentsView';
import { commands, ComponentsOverview } from '../ipc/bindings';

const { openDialogMock } = vi.hoisted(() => ({
  openDialogMock: vi.fn(),
}));

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: openDialogMock,
  save: vi.fn(),
}));

vi.mock('../ipc/bindings', async () => {
  const actual = await vi.importActual<typeof import('../ipc/bindings')>('../ipc/bindings');
  return {
    ...actual,
    events: {
      componentInstallProgressEvent: {
        listen: vi.fn().mockResolvedValue(() => undefined),
      },
    },
    commands: {
      ...actual.commands,
      fetchComponentsManifest: vi.fn(),
      installAllRequiredComponents: vi.fn(),
      selectCustomDataRoot: vi.fn(),
      verifyComponent: vi.fn(),
      repairComponent: vi.fn(),
      retryQuarantinedComponent: vi.fn(),
    },
  };
});

function buildOverview(overrides?: Partial<ComponentsOverview>): ComponentsOverview {
  return {
    diagnostics: {
      custom_ffmpeg: {
        enabled: false,
        configured_path: null,
        redacted_path: null,
        warning_flag: false,
        warning_message: null,
        warnings: [],
        file_present: false,
        fallback_active: false,
        capability_report: null,
      },
      pending_deferred_deletes: 0,
      components: [
        {
          component: 'ffmpeg',
          kind: 'bin',
          current_version: '9.0.2',
          previous_version: null,
          active_source: 'managed',
          active_path: 'bin/ffmpeg/9.0.2/ffmpeg.exe',
          missing_advice: null,
          versions: [
            {
              version: '9.0.2',
              kind: 'bin',
              status: 'installed',
              is_current: true,
              is_previous: false,
              manifest_sequence: 1,
              installed_at: 1000,
              quarantine_reason: null,
              capability_report: {
                component: 'ffmpeg',
                version: '9.0.2',
                mode: 'strict',
                status: 'pass',
                checked_at: 1000,
                reported_version: '9.0.2',
                reasons: [],
                warnings: [],
                ffmpeg: {
                  ffmpeg_version: '9.0.2',
                  ffprobe_version: '9.0.2',
                  has_gpl: false,
                  has_nonfree: false,
                  has_version3: true,
                  effective_license: 'LGPLv3',
                  forbidden_libs_present: [],
                  forbidden_encoders_present: [],
                  gpl_filters_present: [],
                  hardware_encoders_available: ['h264_nvenc', 'h264_mf'],
                  required_encoders_present: ['h264_mf'],
                  missing_encoders: [],
                  missing_decoders: [],
                  missing_filters: [],
                  missing_protocols: [],
                  missing_formats: [],
                },
              },
            },
          ],
        },
        {
          component: 'ggml-small-q5_1.bin',
          kind: 'model',
          current_version: 'small-q5_1-sha1234',
          previous_version: null,
          active_source: 'managed',
          active_path: 'models/ggml-small-q5_1.bin',
          missing_advice: null,
          versions: [],
        },
      ],
    },
    required_components: [
      'yt-dlp',
      'qjs',
      'ffmpeg',
      'whisper-cli',
      'ggml-silero-v6.2.0.bin',
      'ggml-small-q5_1.bin',
    ],
    manifest: null,
    manifest_url_config: {
      url: 'https://cdn.clipperhelper.com/manifests/windows-x86_64.json',
      sig_url: 'https://cdn.clipperhelper.com/manifests/windows-x86_64.json.sig',
      can_override_in_ui: false,
    },
    disk_space: {
      available_bytes: 20 * 1024 * 1024 * 1024,
      required_bytes: 1.5 * 1024 * 1024 * 1024,
      sufficient: true,
      data_root_redacted: '%USERPROFILE%\\AppData\\Local\\ClipperHelper',
      is_default_data_root: true,
      is_onedrive: false,
    },
    speech_model: {
      active_model: 'small-q5_1',
      recommended_model: 'small-q5_1',
      is_user_override: false,
      downgrade_suggested: false,
      suggested_lighter_model: null,
      consecutive_slow_runs: 0,
      options: [],
    },
    ...overrides,
  };
}

describe('ComponentsView', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders plain-language capability + model hash verified + neutral offline Retry', async () => {
    const user = userEvent.setup();
    const onRefresh = vi.fn().mockResolvedValue(undefined);
    vi.mocked(commands.fetchComponentsManifest).mockResolvedValue({
      status: 'ok',
      data: {
        sequence: 1,
        channel: 'stable',
        generated_at: '2026-11-25T00:00:00Z',
        components: [],
      },
    });

    render(
      <ComponentsView
        locale="id"
        overview={buildOverview()}
        onRefreshOverview={onRefresh}
        onOpenDiagnostics={vi.fn()}
      />
    );

    // Offline state is neutral and provides Retry
    const offlineBanner = screen.getByTestId('components-offline-banner');
    expect(offlineBanner).toHaveTextContent('Server komponen belum terjangkau');

    // Clean single 'v' prefix and model says "Hash terverifikasi"
    const ffmpegRow = screen.getByTestId('component-row-ffmpeg');
    expect(ffmpegRow).toHaveTextContent('v9.0.2');
    expect(ffmpegRow).not.toHaveTextContent('vv9.0.2');
    expect(ffmpegRow).toHaveTextContent('Siap dipakai');

    const modelRow = screen.getByTestId('component-row-ggml-small-q5_1.bin');
    expect(modelRow).toHaveTextContent('Hash terverifikasi');

    // Detail drawer toggles raw capability info ("Encoders in build")
    await user.click(screen.getByRole('button', { name: 'Detail' }));
    expect(ffmpegRow).toHaveTextContent('Encoders in build: h264_nvenc, h264_mf');

    await user.click(screen.getAllByRole('button', { name: 'Coba Lagi' })[0]!);
    expect(commands.fetchComponentsManifest).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(onRefresh).toHaveBeenCalledTimes(1));
  });

  it('shows blocking insufficient disk banner and allows choosing custom DataRoot with restart warning', async () => {
    const user = userEvent.setup();
    const onRefresh = vi.fn().mockResolvedValue(undefined);
    openDialogMock.mockResolvedValue('D:\\ClipperData');
    vi.mocked(commands.selectCustomDataRoot).mockResolvedValue({
      status: 'ok',
      data: {
        new_root_redacted: 'D:\\ClipperData',
        is_onedrive: false,
        requires_restart: true,
      },
    });

    const lowDiskOverview = buildOverview({
      disk_space: {
        available_bytes: 0.5 * 1024 * 1024 * 1024,
        required_bytes: 2.0 * 1024 * 1024 * 1024,
        sufficient: false,
        data_root_redacted: '%USERPROFILE%\\AppData\\Local\\ClipperHelper',
        is_default_data_root: true,
        is_onedrive: false,
      },
    });

    render(
      <ComponentsView
        locale="id"
        overview={lowDiskOverview}
        onRefreshOverview={onRefresh}
        onOpenDiagnostics={vi.fn()}
      />
    );

    const blocking = screen.getByTestId('disk-insufficient-blocking');
    expect(blocking).toHaveTextContent('2.00 GB');
    expect(blocking).toHaveTextContent('512.0 MB');
    expect(screen.getByRole('button', { name: 'Pasang semua' })).toBeDisabled();

    await user.click(screen.getAllByRole('button', { name: 'Pilih lokasi data lain' })[0]!);
    expect(openDialogMock).toHaveBeenCalledWith(
      expect.objectContaining({ directory: true, multiple: false })
    );
    await waitFor(() =>
      expect(commands.selectCustomDataRoot).toHaveBeenCalledWith('D:\\ClipperData')
    );

    const notice = await screen.findByTestId('dataroot-restart-notice');
    expect(notice).toHaveTextContent('bootstrap.json');
    expect(notice).toHaveTextContent('dimulai ulang');
    expect(notice).toHaveTextContent('tidak dipindahkan otomatis');
  });

  it('keeps neutral offline banner (no error banner) when manifest fetch returns E_NET_OFFLINE and recovers on Retry', async () => {
    const user = userEvent.setup();
    const onRefresh = vi.fn().mockResolvedValue(undefined);
    vi.mocked(commands.fetchComponentsManifest)
      .mockResolvedValueOnce({
        status: 'error',
        error: {
          code: 'E_NET_OFFLINE',
          message_key: 'err.E_NET_OFFLINE.body',
          message: 'Connection refused',
          detail: 'tcp connect error',
          disk_bounds: null,
        },
      })
      .mockResolvedValueOnce({
        status: 'ok',
        data: {
          sequence: 2,
          channel: 'stable',
          generated_at: '2026-11-25T00:00:00Z',
          components: [],
        },
      });

    render(
      <ComponentsView
        locale="id"
        overview={buildOverview()}
        onRefreshOverview={onRefresh}
        onOpenDiagnostics={vi.fn()}
      />
    );

    await user.click(screen.getAllByRole('button', { name: 'Coba Lagi' })[0]!);
    expect(screen.getByTestId('components-offline-banner')).toBeInTheDocument();
    expect(screen.queryByTestId('shared-error-banner')).not.toBeInTheDocument();

    await user.click(screen.getAllByRole('button', { name: 'Coba Lagi' })[0]!);
    await waitFor(() => expect(onRefresh).toHaveBeenCalledTimes(1));
  });
});
