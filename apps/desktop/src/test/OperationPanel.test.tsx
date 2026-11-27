import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { OperationPanel } from '../components/OperationPanel';
import { JobEventPayload, JobRecord } from '../ipc/bindings';
import { formatEtaDisplay, formatRateDisplay, roundDisplaySeconds } from '../utils/etaFormat';

function makeJob(overrides?: Partial<JobRecord>): JobRecord {
  return {
    id: 'job-1111-2222',
    project_id: 'proj-1',
    kind: 'generate',
    status: 'running',
    stage: 'download',
    progress_json: null,
    error_code: null,
    error_detail: null,
    idempotency_key: 'k'.repeat(64),
    attempt: 1,
    created_at: 1_700_000_000,
    started_at: 1_700_000_001,
    finished_at: null,
    ...overrides,
  };
}

function makeEvent(overrides?: Partial<JobEventPayload>): JobEventPayload {
  return {
    jobId: 'job-1111-2222',
    projectId: 'proj-1',
    kind: 'generate',
    state: 'running',
    stage: {
      id: 'download',
      index: 1,
      count: 3,
      labelKey: 'job.stage.download',
      labelArgs: null,
    },
    stageProgress: {
      done: 20,
      total: 70,
      unit: 'media_seconds',
    },
    overall: { percent: 30 },
    rate: { value: 1.66, unit: 'media_seconds_per_sec', smoothed: true },
    eta: { seconds: 32, lowSeconds: 28, highSeconds: 38, confidence: 'medium' },
    detail: { startup: 'false', stallStatus: 'normal' },
    error: null,
    ts: 1_700_000_005_000,
    ...overrides,
  };
}

describe('etaFormat rounding & display rules (PROGRESS_AND_ETA §5)', () => {
  it('applies exact bucket rounding (<10s, 10-60s -> 5s, 1-10m -> 15s, >10m -> 1m)', () => {
    expect(roundDisplaySeconds(4)).toBe(4);
    expect(roundDisplaySeconds(32)).toBe(30);
    expect(roundDisplaySeconds(34)).toBe(35);
    expect(roundDisplaySeconds(138)).toBe(135); // 2m15s
    expect(roundDisplaySeconds(730)).toBe(720); // 12m
  });

  it('formats low-confidence ranges, startup fallback, and localized realtime speed', () => {
    expect(formatEtaDisplay(null, 'id', true)).toBe('Menghitung estimasi…');
    expect(
      formatEtaDisplay({ seconds: 6, lowSeconds: 4, highSeconds: 8, confidence: 'high' }, 'id')
    ).toBe('beberapa detik lagi');
    expect(
      formatEtaDisplay({ seconds: 180, lowSeconds: 120, highSeconds: 240, confidence: 'low' }, 'id')
    ).toBe('sekitar 2–4 menit lagi');
    expect(
      formatRateDisplay({ value: 1.66, unit: 'media_seconds_per_sec', smoothed: true }, 'id')
    ).toBe('1,66× kecepatan normal');
  });
});

describe('OperationPanel (M1.4 progress, stall, cancel confirm, errors, cache hit)', () => {
  it('shows indeterminate startup message then switches to determinate progress, ETA, and 15s stall notice', () => {
    const job = makeJob();
    const { rerender } = render(
      <OperationPanel
        locale="id"
        job={job}
        liveEvent={makeEvent({
          overall: { percent: 5 },
          eta: null,
          rate: null,
          detail: { startup: 'true', stallStatus: 'normal' },
        })}
        onCancelJob={vi.fn()}
        onRetryJob={vi.fn()}
      />
    );

    expect(screen.getByTestId('operation-indeterminate-bar')).toBeInTheDocument();
    expect(screen.getByTestId('operation-status-line')).toHaveTextContent('Menyiapkan unduhan…');
    expect(screen.getByTestId('operation-status-line')).toHaveTextContent('Menghitung estimasi…');

    rerender(
      <OperationPanel
        locale="id"
        job={job}
        liveEvent={makeEvent({
          detail: { startup: 'false', stallStatus: 'slow_warning' },
        })}
        onCancelJob={vi.fn()}
        onRetryJob={vi.fn()}
      />
    );

    expect(screen.getByTestId('operation-progress-bar')).toHaveStyle({ width: '30%' });
    expect(screen.getByTestId('operation-rate-text')).toHaveTextContent('1,66× kecepatan normal');
    expect(screen.getByTestId('operation-stall-warning')).toHaveTextContent(
      'Koneksi lambat / proses belum bergerak'
    );
  });

  it('requires non-resumable confirmation before cancelling download', async () => {
    const user = userEvent.setup();
    const onCancelJob = vi.fn().mockResolvedValue(undefined);
    render(
      <OperationPanel
        locale="id"
        job={makeJob()}
        liveEvent={makeEvent()}
        onCancelJob={onCancelJob}
        onRetryJob={vi.fn()}
      />
    );

    await user.click(screen.getByTestId('operation-cancel-button'));
    const dlg = screen.getByTestId('cancel-confirm-dialog');
    expect(dlg).toHaveTextContent(
      'Proses unduh akan dihentikan dan harus diulang dari awal jika dilanjutkan.'
    );
    expect(onCancelJob).not.toHaveBeenCalled();

    await user.click(screen.getByRole('button', { name: 'Ya, batalkan' }));
    await waitFor(() => expect(onCancelJob).toHaveBeenCalledWith('job-1111-2222'));
  });

  it('renders repair action on E_COMPONENT_MISSING and cache-hit + proxy notes on success', async () => {
    const user = userEvent.setup();
    const onRepair = vi.fn();
    const { rerender } = render(
      <OperationPanel
        locale="id"
        job={makeJob({ status: 'failed', error_code: 'E_COMPONENT_MISSING' })}
        liveEvent={makeEvent({
          state: 'failed',
          error: {
            code: 'E_COMPONENT_MISSING',
            messageKey: 'err.E_COMPONENT_MISSING.body',
            retryable: true,
            actionKeys: ['repair', 'retry'],
          },
        })}
        onCancelJob={vi.fn()}
        onRetryJob={vi.fn()}
        onNavigateRepair={onRepair}
      />
    );

    await user.click(screen.getByRole('button', { name: 'Perbaiki' }));
    expect(onRepair).toHaveBeenCalledTimes(1);

    rerender(
      <OperationPanel
        locale="id"
        job={makeJob({ status: 'succeeded', stage: 'build' })}
        liveEvent={makeEvent({
          state: 'succeeded',
          stage: {
            id: 'build',
            index: 3,
            count: 4,
            labelKey: 'job.stage.build',
            labelArgs: null,
          },
          overall: { percent: 100 },
          detail: {
            cacheHit: 'true',
            needsProxy: 'true',
            width: '1920',
            height: '1080',
            fps: '60',
            videoCodec: 'vp9',
            sizeBytes: '7340032',
          },
        })}
        onCancelJob={vi.fn()}
        onRetryJob={vi.fn()}
      />
    );

    expect(screen.getByTestId('operation-cache-hit-notice')).toBeInTheDocument();
    expect(screen.getByTestId('operation-proxy-note')).toBeInTheDocument();
    expect(screen.getByTestId('operation-success-card')).toHaveTextContent('1920×1080');
    expect(screen.getByRole('button', { name: 'Buka editor' })).toBeDisabled();
  });
});
