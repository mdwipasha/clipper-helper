import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { HomeView } from '../components/HomeView';
import { AppCommandError, commands, ProbeProjectResponse, ProjectRecord } from '../ipc/bindings';

vi.mock('../ipc/bindings', async () => {
  const actual = await vi.importActual<typeof import('../ipc/bindings')>('../ipc/bindings');
  return {
    ...actual,
    events: {
      probeProgressEvent: {
        listen: vi.fn().mockResolvedValue(() => undefined),
      },
    },
    commands: {
      ...actual.commands,
      probeYoutubeUrl: vi.fn(),
      cancelYoutubeProbe: vi.fn(),
      listRecentProjects: vi.fn(),
      deleteProject: vi.fn(),
    },
  };
});

function buildSampleProject(overrides?: Partial<ProjectRecord>): ProjectRecord {
  return {
    id: '11111111-1111-4111-8111-111111111111',
    title: 'Podcast Episode 42',
    channel: 'Channel Mantap',
    source_provider: 'youtube',
    source_video_id: 'dQw4w9WgXcQ',
    source_url: 'https://www.youtube.com/watch?v=dQw4w9WgXcQ',
    duration_seconds: 3723,
    live_status: 'not_live',
    preview_mode: 'embed',
    suggested_start_seconds: null,
    range_start_ms: null,
    range_end_ms: null,
    status: 'probed',
    created_at: 1_700_000_000,
    updated_at: 1_700_000_000,
    ...overrides,
  };
}

function buildProbeResponse(overrides?: Partial<ProbeProjectResponse>): ProbeProjectResponse {
  const project = buildSampleProject(overrides?.project);
  return {
    probe_info: {
      id: project.source_video_id,
      title: project.title,
      channel: project.channel,
      duration_seconds: project.duration_seconds,
      live_status: project.live_status,
      availability: 'public',
      playable_in_embed: project.preview_mode === 'embed',
      was_live: project.live_status === 'was_live',
      is_live: false,
      estimated_bytes_per_second: 150_000,
      has_avc1_1080p: true,
      preview_mode: project.preview_mode === 'embed' ? 'embed' : 'timecode_only',
      ...(overrides?.probe_info ?? {}),
    },
    project,
    suggested_start_seconds: overrides?.suggested_start_seconds ?? null,
  };
}

describe('HomeView (M1.2 intake + probe + recent projects)', () => {
  const onNavigate = vi.fn();

  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listRecentProjects).mockResolvedValue({ status: 'ok', data: [] });
    vi.mocked(commands.cancelYoutubeProbe).mockResolvedValue({ status: 'ok', data: true });
    vi.mocked(commands.deleteProject).mockResolvedValue({ status: 'ok', data: true });
  });

  it('trims whitespace on paste and submits trimmed URL', async () => {
    const user = userEvent.setup();
    vi.mocked(commands.probeYoutubeUrl).mockResolvedValue({
      status: 'ok',
      data: buildProbeResponse(),
    });

    render(<HomeView locale="id" overview={null} settings={null} onNavigate={onNavigate} />);

    const input = screen.getByLabelText('Link Video YouTube') as HTMLInputElement;
    await user.click(input);
    await user.paste('   https://youtu.be/dQw4w9WgXcQ   ');

    expect(input.value).toBe('https://youtu.be/dQw4w9WgXcQ');

    await user.click(screen.getByRole('button', { name: 'Cek link' }));
    await waitFor(() => {
      expect(commands.probeYoutubeUrl).toHaveBeenCalledWith(
        'https://youtu.be/dQw4w9WgXcQ',
        expect.any(String)
      );
    });
  });

  it('shows loading state with elapsed timer and cancels active probe', async () => {
    const user = userEvent.setup();
    let resolveProbe: (val: { status: 'error'; error: AppCommandError }) => void = () => {};
    vi.mocked(commands.probeYoutubeUrl).mockImplementation(
      () =>
        new Promise((resolve) => {
          resolveProbe = resolve;
        })
    );

    render(<HomeView locale="id" overview={null} settings={null} onNavigate={onNavigate} />);

    const input = screen.getByLabelText('Link Video YouTube');
    await user.type(input, 'https://www.youtube.com/watch?v=dQw4w9WgXcQ');
    await user.click(screen.getByRole('button', { name: 'Cek link' }));

    expect(screen.getByTestId('probe-loading')).toBeInTheDocument();
    expect(screen.getByTestId('probe-elapsed')).toHaveTextContent('0 dtk');

    await user.click(screen.getByRole('button', { name: 'Batal' }));
    await waitFor(() => {
      expect(commands.cancelYoutubeProbe).toHaveBeenCalledWith(expect.any(String));
    });

    resolveProbe({
      status: 'error',
      error: {
        code: 'E_UNKNOWN',
        message_key: 'home.probeCancelled',
        message: 'Probe cancelled',
        detail: 'Probe cancelled by user',
        disk_bounds: null,
      },
    });

    await waitFor(() => {
      expect(screen.getByText('Pemeriksaan link dibatalkan.')).toBeInTheDocument();
    });
  });

  it('renders success card with metadata, embed/timecode preview mode, and disabled Lanjut hint', async () => {
    const user = userEvent.setup();
    const resEmbed = buildProbeResponse({
      suggested_start_seconds: 95,
      probe_info: {
        id: 'dQw4w9WgXcQ',
        title: 'PODCAST BESAR',
        channel: 'Studio Indo',
        duration_seconds: 3723,
        live_status: 'was_live',
        availability: 'public',
        playable_in_embed: true,
        was_live: true,
        is_live: false,
        estimated_bytes_per_second: 150_000,
        has_avc1_1080p: true,
        preview_mode: 'embed',
      },
    });
    vi.mocked(commands.probeYoutubeUrl).mockResolvedValueOnce({ status: 'ok', data: resEmbed });
    vi.mocked(commands.listRecentProjects).mockResolvedValueOnce({ status: 'ok', data: [] });
    vi.mocked(commands.listRecentProjects).mockResolvedValueOnce({
      status: 'ok',
      data: [resEmbed.project],
    });

    const onOpenProject = vi.fn();
    render(
      <HomeView
        locale="id"
        overview={null}
        settings={null}
        onNavigate={onNavigate}
        onOpenProject={onOpenProject}
      />
    );

    await user.type(
      screen.getByLabelText('Link Video YouTube'),
      'https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=95'
    );
    await user.click(screen.getByRole('button', { name: 'Cek link' }));

    const card = await screen.findByTestId('probe-result-card');
    expect(card).toHaveTextContent('PODCAST BESAR');
    expect(card).toHaveTextContent('Studio Indo');
    expect(card).toHaveTextContent('01:02:03');
    expect(card).toHaveTextContent('Rekaman livestream');
    expect(card).toHaveTextContent('Pratinjau: tersedia');
    expect(card).toHaveTextContent('Mulai disarankan: 01:35');

    const lanjutBtn = screen.getByRole('button', { name: 'Lanjut' });
    expect(lanjutBtn).toBeEnabled();
    await user.click(lanjutBtn);
    expect(onOpenProject).toHaveBeenCalledWith(resEmbed.project);

    // Now test timecode_only preview label
    vi.mocked(commands.probeYoutubeUrl).mockResolvedValueOnce({
      status: 'ok',
      data: buildProbeResponse({
        project: buildSampleProject({ preview_mode: 'timecode_only' }),
        probe_info: {
          id: 'dQw4w9WgXcQ',
          title: 'Embed Disabled Video',
          channel: 'Creator B',
          duration_seconds: 185,
          live_status: 'not_live',
          availability: 'public',
          playable_in_embed: false,
          was_live: false,
          is_live: false,
          estimated_bytes_per_second: 100_000,
          has_avc1_1080p: true,
          preview_mode: 'timecode_only',
        },
      }),
    });
    await user.click(screen.getByRole('button', { name: 'Cek link' }));
    await waitFor(() => {
      expect(screen.getByTestId('probe-result-card')).toHaveTextContent(
        'Pratinjau: hanya timecode'
      );
    });
  });

  it('displays distinct Indonesian messages for each YouTube error reason', async () => {
    const user = userEvent.setup();
    const cases: Array<{ code: string; message_key: string; expectedSubstring: string }> = [
      {
        code: 'E_YT_UNAVAILABLE',
        message_key: 'err.E_YT_UNAVAILABLE.members_only',
        expectedSubstring: 'khusus anggota channel',
      },
      {
        code: 'E_YT_UNAVAILABLE',
        message_key: 'err.E_YT_UNAVAILABLE.age_restricted',
        expectedSubstring: 'dibatasi usia',
      },
      {
        code: 'E_YT_UNAVAILABLE',
        message_key: 'err.E_YT_UNAVAILABLE.region_blocked',
        expectedSubstring: 'wilayah (negara)',
      },
      {
        code: 'E_YT_UNAVAILABLE',
        message_key: 'err.E_YT_UNAVAILABLE.private',
        expectedSubstring: 'bersifat pribadi',
      },
      {
        code: 'E_YT_UNAVAILABLE',
        message_key: 'err.E_YT_UNAVAILABLE.deleted',
        expectedSubstring: 'sudah dihapus',
      },
      {
        code: 'E_YT_LIVE_NOT_ENDED',
        message_key: 'err.E_YT_LIVE_NOT_ENDED.ongoing_live',
        expectedSubstring: 'Siaran langsung sedang berjalan',
      },
      {
        code: 'E_YT_LIVE_NOT_ENDED',
        message_key: 'err.E_YT_LIVE_NOT_ENDED.premiere_not_started',
        expectedSubstring: 'premiere ini belum dimulai',
      },
      {
        code: 'E_YT_BOT_CHECK',
        message_key: 'err.E_YT_BOT_CHECK.body',
        expectedSubstring: "verifikasi 'bukan bot'",
      },
      {
        code: 'E_YT_RATE_LIMITED',
        message_key: 'err.E_YT_RATE_LIMITED.body',
        expectedSubstring: 'membatasi permintaan sementara',
      },
      {
        code: 'E_YT_URL_INVALID',
        message_key: 'err.E_YT_URL_INVALID.collection',
        expectedSubstring: 'Link playlist, channel, atau pencarian tidak didukung',
      },
    ];

    render(<HomeView locale="id" overview={null} settings={null} onNavigate={onNavigate} />);

    const input = screen.getByLabelText('Link Video YouTube');
    await user.type(input, 'https://www.youtube.com/watch?v=dQw4w9WgXcQ');

    for (const item of cases) {
      vi.mocked(commands.probeYoutubeUrl).mockResolvedValueOnce({
        status: 'error',
        error: {
          code: item.code,
          message_key: item.message_key,
          message: 'raw internal error',
          detail: 'redacted detail',
          disk_bounds: null,
        },
      });
      await user.click(screen.getByRole('button', { name: 'Cek link' }));
      const banner = await screen.findByTestId('home-probe-error-banner');
      expect(banner).toHaveTextContent(item.expectedSubstring);
    }
  });

  it('offers Retry on offline error and recovers on second attempt', async () => {
    const user = userEvent.setup();
    vi.mocked(commands.probeYoutubeUrl)
      .mockResolvedValueOnce({
        status: 'error',
        error: {
          code: 'E_NET_OFFLINE',
          message_key: 'err.E_NET_OFFLINE.body',
          message: 'Unable to resolve host',
          detail: 'Unable to resolve host',
          disk_bounds: null,
        },
      })
      .mockResolvedValueOnce({
        status: 'ok',
        data: buildProbeResponse(),
      });

    render(<HomeView locale="id" overview={null} settings={null} onNavigate={onNavigate} />);

    await user.type(
      screen.getByLabelText('Link Video YouTube'),
      'https://www.youtube.com/watch?v=dQw4w9WgXcQ'
    );
    await user.click(screen.getByRole('button', { name: 'Cek link' }));

    const banner = await screen.findByTestId('home-probe-error-banner');
    expect(banner).toHaveTextContent('E_NET_OFFLINE');

    await user.click(screen.getByRole('button', { name: 'Coba Lagi' }));
    await waitFor(() => {
      expect(screen.getByTestId('probe-result-card')).toHaveTextContent('Podcast Episode 42');
    });
  });

  it('lists recent projects and deletes one cleanly', async () => {
    const user = userEvent.setup();
    const p1 = buildSampleProject({
      id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
      title: 'Proyek A',
    });
    const p2 = buildSampleProject({
      id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
      title: 'Proyek B',
    });
    vi.mocked(commands.listRecentProjects).mockResolvedValue({ status: 'ok', data: [p1, p2] });

    render(<HomeView locale="id" overview={null} settings={null} onNavigate={onNavigate} />);

    await waitFor(() => {
      expect(screen.getByText('Proyek A')).toBeInTheDocument();
      expect(screen.getByText('Proyek B')).toBeInTheDocument();
    });

    const rowA = screen.getByTestId(`recent-project-${p1.id}`);
    await user.click(within(rowA).getByRole('button', { name: 'Hapus' }));

    await waitFor(() => {
      expect(commands.deleteProject).toHaveBeenCalledWith(p1.id);
      expect(screen.queryByText('Proyek A')).not.toBeInTheDocument();
      expect(screen.getByText('Proyek B')).toBeInTheDocument();
    });
  });

  it('renders malicious title/channel strictly as inert text with zero HTML injection', async () => {
    const user = userEvent.setup();
    const xssTitle = '<script>window.__xss=1</script><img src=x onerror=alert(1)>';
    const xssChannel = '<b onclick="alert(2)">EvilCh</b>';

    vi.mocked(commands.probeYoutubeUrl).mockResolvedValueOnce({
      status: 'ok',
      data: buildProbeResponse({
        project: buildSampleProject({ title: xssTitle, channel: xssChannel }),
        probe_info: {
          id: 'dQw4w9WgXcQ',
          title: xssTitle,
          channel: xssChannel,
          duration_seconds: 120,
          live_status: 'not_live',
          availability: 'public',
          playable_in_embed: true,
          was_live: false,
          is_live: false,
          estimated_bytes_per_second: 100_000,
          has_avc1_1080p: true,
          preview_mode: 'embed',
        },
      }),
    });

    const { container } = render(
      <HomeView locale="id" overview={null} settings={null} onNavigate={onNavigate} />
    );

    await user.type(
      screen.getByLabelText('Link Video YouTube'),
      'https://www.youtube.com/watch?v=dQw4w9WgXcQ'
    );
    await user.click(screen.getByRole('button', { name: 'Cek link' }));

    const card = await screen.findByTestId('probe-result-card');
    expect(card).toHaveTextContent(xssTitle);
    expect(card).toHaveTextContent(xssChannel);
    expect(container.querySelector('script')).toBeNull();
    expect(container.querySelector('img')).toBeNull();
    expect(container.querySelector('b')).toBeNull();
  });
});
