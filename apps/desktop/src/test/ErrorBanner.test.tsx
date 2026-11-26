import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { ErrorBanner } from '../components/ErrorBanner';

describe('ErrorBanner (shared component)', () => {
  it('renders title, code, message, custom actions, copies single error detail, and navigates to Diagnostik', async () => {
    const user = userEvent.setup();
    const onRetry = vi.fn();
    const onOpenDiagnostics = vi.fn();
    const onCopyDetail = vi.fn().mockResolvedValue(undefined);

    render(
      <ErrorBanner
        locale="id"
        error={{
          code: 'E_VERIFY_FAILED',
          title: 'Verifikasi gagal',
          message: 'Komponen ffmpeg gagal lolos verifikasi kapabilitas.',
          redactedDetail: 'Detected forbidden --enable-gpl flag in %USERPROFILE%\\ffmpeg.exe',
          actions: [{ label: 'Coba Lagi', variant: 'primary', onClick: onRetry }],
        }}
        onCopyDetail={onCopyDetail}
        onOpenDiagnostics={onOpenDiagnostics}
      />
    );

    expect(screen.getByText('Verifikasi gagal')).toBeInTheDocument();
    expect(screen.getByText('E_VERIFY_FAILED')).toBeInTheDocument();
    expect(screen.getByText(/gagal lolos verifikasi/)).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Coba Lagi' }));
    expect(onRetry).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole('button', { name: 'Salin detail error' }));
    expect(onCopyDetail).toHaveBeenCalledWith(
      expect.stringContaining('[E_VERIFY_FAILED] Verifikasi gagal')
    );
    expect(onCopyDetail).toHaveBeenCalledWith(expect.stringContaining('--enable-gpl'));

    await user.click(screen.getByRole('button', { name: 'Buka Diagnostik' }));
    expect(onOpenDiagnostics).toHaveBeenCalledTimes(1);
  });
});
