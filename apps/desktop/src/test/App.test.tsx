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
});
