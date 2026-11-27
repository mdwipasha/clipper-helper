import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { App } from '../App';
import { commands } from '../ipc/bindings';

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(),
  save: vi.fn(),
}));

vi.mock('../components/DevPanel', () => ({
  DevPanel: () => null,
}));

vi.mock('../ipc/bindings', async () => {
  const actual = await vi.importActual<typeof import('../ipc/bindings')>('../ipc/bindings');
  return {
    ...actual,
    events: {
      ...actual.events,
      probeProgressEvent: {
        listen: vi.fn().mockResolvedValue(() => undefined),
      },
      jobEventPayload: {
        listen: vi.fn().mockResolvedValue(() => undefined),
      },
    },
    commands: {
      ...actual.commands,
      getAppInfo: vi.fn(),
      getWorkspaceStatus: vi.fn(),
      useDefaultDataRoot: vi.fn(),
      getCrashRecoveryStatus: vi.fn(),
      getComponentsOverview: vi.fn(),
      getSettingsOverview: vi.fn(),
      fetchComponentsManifest: vi.fn(),
      listRecentProjects: vi.fn().mockResolvedValue({ status: 'ok', data: [] }),
      listJobs: vi.fn().mockResolvedValue({ status: 'ok', data: [] }),
      getProject: vi.fn(),
      cancelJob: vi.fn(),
      retryJob: vi.fn(),
      startGenerateJob: vi.fn(),
      getPreviewConfig: vi.fn().mockResolvedValue({
        bridge_url: 'https://clipper-bridge.vercel.app',
        bridge_protocol_version: 1,
        control_video_id: 'aqz-KE-bpKQ',
        ready_timeout_ms: 6000,
        max_range_seconds: 300,
        download_prior_rt_factor: 1.66,
        download_prior_startup_seconds: 10,
        can_force_timecode_only: false,
      }),
    },
  };
});

describe('App startup DataRoot recovery', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('shows actionable recovery screen when configured DataRoot is unreachable and recovers via default location', async () => {
    const user = userEvent.setup();

    vi.mocked(commands.getAppInfo).mockResolvedValue({
      name: 'ClipperHelper',
      version: '0.1.0',
      platform: 'windows',
    });
    vi.mocked(commands.getCrashRecoveryStatus).mockResolvedValue({
      unclean_shutdown_detected: false,
      previous_started_at: null,
      crash_record: null,
      banner_dismissed: false,
    });
    vi.mocked(commands.getWorkspaceStatus)
      .mockResolvedValueOnce({
        is_initialized: false,
        root: null,
        error_code: 'E_APP_INIT_FAILED',
        error_message: 'Configured data root is unreachable',
        configured_root_unreachable: true,
        unreachable_root_redacted: 'Q:\\MissingDrive\\ClipperHelper',
      })
      .mockResolvedValueOnce({
        is_initialized: true,
        root: 'C:\\Users\\test\\AppData\\Local\\ClipperHelper-Dev',
        error_code: null,
        error_message: null,
        configured_root_unreachable: false,
        unreachable_root_redacted: null,
      });

    vi.mocked(commands.useDefaultDataRoot).mockResolvedValue({
      status: 'ok',
      data: {
        is_initialized: true,
        root: 'C:\\Users\\test\\AppData\\Local\\ClipperHelper-Dev',
        error_code: null,
        error_message: null,
        configured_root_unreachable: false,
        unreachable_root_redacted: null,
      },
    });
    vi.mocked(commands.getComponentsOverview).mockResolvedValue({
      status: 'error',
      error: {
        code: 'E_UNKNOWN',
        message_key: 'err.E_UNKNOWN.body',
        message: 'noop',
        detail: 'noop',
        disk_bounds: null,
      },
    });
    vi.mocked(commands.getSettingsOverview).mockResolvedValue({
      status: 'error',
      error: {
        code: 'E_UNKNOWN',
        message_key: 'err.E_UNKNOWN.body',
        message: 'noop',
        detail: 'noop',
        disk_bounds: null,
      },
    });
    vi.mocked(commands.fetchComponentsManifest).mockResolvedValue({
      status: 'error',
      error: {
        code: 'E_NET_OFFLINE',
        message_key: 'err.E_NET_OFFLINE.body',
        message: 'offline',
        detail: 'offline',
        disk_bounds: null,
      },
    });

    render(<App />);

    const screenBox = await screen.findByTestId('unreachable-dataroot-screen');
    expect(screenBox).toHaveTextContent(
      'Folder data tidak ditemukan — sambungkan drive atau gunakan lokasi bawaan'
    );
    expect(screenBox).toHaveTextContent('Q:\\MissingDrive\\ClipperHelper');

    await user.click(screen.getByRole('button', { name: 'Gunakan lokasi bawaan' }));
    await waitFor(() => expect(commands.useDefaultDataRoot).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(screen.queryByTestId('unreachable-dataroot-screen')).not.toBeInTheDocument()
    );
  });

  it('shows global header activity chip across tabs and navigates back to active project', async () => {
    const user = userEvent.setup();
    vi.mocked(commands.getAppInfo).mockResolvedValue({
      name: 'ClipperHelper',
      version: '0.1.0',
      platform: 'windows',
    });
    vi.mocked(commands.getCrashRecoveryStatus).mockResolvedValue({
      unclean_shutdown_detected: false,
      previous_started_at: null,
      crash_record: null,
      banner_dismissed: false,
    });
    vi.mocked(commands.getWorkspaceStatus).mockResolvedValue({
      is_initialized: true,
      root: 'C:\\Users\\test\\AppData\\Local\\ClipperHelper-Dev',
      error_code: null,
      error_message: null,
      configured_root_unreachable: false,
      unreachable_root_redacted: null,
    });
    vi.mocked(commands.getComponentsOverview).mockResolvedValue({
      status: 'error',
      error: {
        code: 'E_UNKNOWN',
        message_key: 'err.E_UNKNOWN.body',
        message: 'noop',
        detail: 'noop',
        disk_bounds: null,
      },
    });
    vi.mocked(commands.getSettingsOverview).mockResolvedValue({
      status: 'error',
      error: {
        code: 'E_UNKNOWN',
        message_key: 'err.E_UNKNOWN.body',
        message: 'noop',
        detail: 'noop',
        disk_bounds: null,
      },
    });
    vi.mocked(commands.fetchComponentsManifest).mockResolvedValue({
      status: 'error',
      error: {
        code: 'E_NET_OFFLINE',
        message_key: 'err.E_NET_OFFLINE.body',
        message: 'offline',
        detail: 'offline',
        disk_bounds: null,
      },
    });
    vi.mocked(commands.listJobs).mockResolvedValue({
      status: 'ok',
      data: [
        {
          id: 'job-active-1',
          project_id: 'proj-chip-1',
          kind: 'generate',
          status: 'running',
          stage: 'download',
          progress_json: null,
          error_code: null,
          error_detail: null,
          idempotency_key: null,
          attempt: 1,
          created_at: 1_700_000_000,
          started_at: 1_700_000_001,
          finished_at: null,
        },
      ],
    });
    vi.mocked(commands.getProject).mockResolvedValue({
      status: 'ok',
      data: {
        id: 'proj-chip-1',
        title: 'Chip Target Project',
        channel: 'Ch',
        source_provider: 'youtube',
        source_video_id: 'aqz-KE-bpKQ',
        source_url: 'https://www.youtube.com/watch?v=aqz-KE-bpKQ',
        duration_seconds: 120,
        live_status: 'not_live',
        preview_mode: 'timecode_only',
        suggested_start_seconds: null,
        range_start_ms: 10_000,
        range_end_ms: 40_000,
        status: 'ranged',
        composition_hash: null,
        created_at: 1_700_000_000,
        updated_at: 1_700_000_000,
      },
    });

    render(<App />);

    // Navigate away to Pengaturan tab; global activity chip stays visible
    await user.click(screen.getByRole('button', { name: 'Pengaturan' }));
    const chip = await screen.findByTestId('global-job-activity-chip');
    expect(chip).toHaveTextContent('Mengunduh klip');

    await user.click(chip);
    await waitFor(() => {
      expect(commands.getProject).toHaveBeenCalledWith('proj-chip-1');
    });
    expect(await screen.findByTestId('range-picker-view')).toHaveTextContent('Chip Target Project');
  });
});
