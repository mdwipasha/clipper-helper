import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { DiagnosticsView } from '../components/DiagnosticsView';
import { commands, DiagnosticsReportBundle, ErrorEventRow } from '../ipc/bindings';

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(),
  save: vi.fn(),
}));

vi.mock('../ipc/bindings', async () => {
  const actual = await vi.importActual<typeof import('../ipc/bindings')>('../ipc/bindings');
  return {
    ...actual,
    commands: {
      ...actual.commands,
      listErrorEvents: vi.fn(),
      generateDiagnosticsReport: vi.fn(),
      saveDiagnosticsReportFile: vi.fn(),
      openLogsFolder: vi.fn(),
    },
  };
});

function makeBundle(
  userNote: string,
  includeIds: boolean,
  truncated = false
): DiagnosticsReportBundle {
  const text = [
    '# ClipperHelper Diagnostics Report',
    '## RINGKASAN',
    `- ID Laporan: CH-ABCD-1234`,
    `- Catatan Pengguna: ${userNote || '(tidak diisi)'}`,
    `## DETAIL`,
    includeIds ? '- id=j1 video_id=dQw4w9WgXcQ' : '- id=j1 video_id=-',
    truncated ? '> DIPOTONG OTOMATIS: detail terlama dihapus' : '',
  ]
    .filter(Boolean)
    .join('\n');

  return {
    report_id: 'CH-ABCD-1234',
    ringkasan_bytes: 180,
    preview_text: text,
    preview_bytes: text.length,
    preview_truncated: truncated,
    full_text: text,
    full_bytes: text.length,
    full_truncated: false,
    error_count: 1,
  };
}

describe('DiagnosticsView', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders empty state when there are no errors', async () => {
    vi.mocked(commands.listErrorEvents).mockResolvedValue({ status: 'ok', data: [] });
    vi.mocked(commands.generateDiagnosticsReport).mockResolvedValue({
      status: 'ok',
      data: makeBundle('', true, false),
    });

    render(
      <DiagnosticsView
        locale="id"
        crashStatus={{
          unclean_shutdown_detected: false,
          previous_started_at: null,
          crash_record: null,
          banner_dismissed: false,
        }}
        onDismissCrashBanner={vi.fn().mockResolvedValue(undefined)}
        onNavigateToDiagnostics={vi.fn()}
      />
    );

    expect(await screen.findByTestId('diagnostics-empty-errors')).toBeInTheDocument();
  });

  it('updates preview with free text & video ID toggle, copies report, shows truncation notice & crash banner', async () => {
    const user = userEvent.setup();
    const writeTextSpy = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue(undefined);
    const errorRows: ErrorEventRow[] = [
      {
        id: 1,
        ts: 1732500000,
        code: 'E_YT_EXTRACT_FAILED',
        stage: 'download',
        job_id: 'job-1',
        message_key: 'err.E_YT_EXTRACT_FAILED.body',
        detail_redacted: 'Download gagal pada %USERPROFILE%\\out.mp4',
        stderr_tail_redacted: 'stderr line',
        app_version: '0.1.0',
        component_versions_json: '{}',
      },
    ];

    vi.mocked(commands.listErrorEvents).mockResolvedValue({ status: 'ok', data: errorRows });
    vi.mocked(commands.generateDiagnosticsReport).mockImplementation(async (note, incIds) => ({
      status: 'ok',
      data: makeBundle(note, incIds, true),
    }));

    render(
      <DiagnosticsView
        locale="id"
        crashStatus={{
          unclean_shutdown_detected: true,
          previous_started_at: 1732499000,
          crash_record: null,
          banner_dismissed: false,
        }}
        onDismissCrashBanner={vi.fn().mockResolvedValue(undefined)}
        onNavigateToDiagnostics={vi.fn()}
      />
    );

    // Crash recovery banner visible
    expect(screen.getByTestId('diagnostics-crash-banner')).toHaveTextContent(
      'Aplikasi sebelumnya berhenti tiba-tiba — Salin laporan'
    );

    // Error event rendered & truncation notice shown
    expect(await screen.findByTestId('error-event-row-1')).toBeInTheDocument();
    expect(await screen.findByTestId('diagnostics-truncation-notice')).toBeInTheDocument();

    // Free text box updates preview live
    const noteInput = screen.getByLabelText('Apa yang sedang kamu lakukan?');
    await user.type(noteInput, 'Render clip gagal');
    await waitFor(() =>
      expect(screen.getByTestId('diagnostics-report-preview')).toHaveTextContent(
        'Render clip gagal'
      )
    );

    // Toggling "Sertakan ID video" off removes 11-char ID from live preview
    expect(screen.getByTestId('diagnostics-report-preview')).toHaveTextContent(
      'video_id=dQw4w9WgXcQ'
    );
    await user.click(screen.getByRole('checkbox'));
    await waitFor(() =>
      expect(screen.getByTestId('diagnostics-report-preview')).not.toHaveTextContent('dQw4w9WgXcQ')
    );

    // Copy report copies exact live preview to clipboard (both crash banner and report bar expose 'Salin laporan')
    const copyButtons = screen.getAllByRole('button', { name: 'Salin laporan' });
    await user.click(copyButtons[copyButtons.length - 1]!);
    expect(writeTextSpy).toHaveBeenCalledWith(
      expect.stringContaining('Catatan Pengguna: Render clip gagal')
    );

    // Per-error "Salin" copies the single redacted error snippet
    await user.click(screen.getByRole('button', { name: 'Salin' }));
    expect(writeTextSpy).toHaveBeenCalledWith(
      expect.stringContaining('[E_YT_EXTRACT_FAILED] stage=download')
    );
  });
});
