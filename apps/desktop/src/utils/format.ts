export function formatComponentVersion(version: string | null | undefined): string {
  if (!version) return '';
  const trimmed = version.trim();
  if (!trimmed) return '';
  return /^[A-Za-z]/.test(trimmed) ? trimmed : `v${trimmed}`;
}

export function formatBytesHuman(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
  if (bytes >= 1024 * 1024 * 1024) {
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
  }
  if (bytes >= 1024 * 1024) {
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }
  if (bytes >= 1024) {
    return `${(bytes / 1024).toFixed(1)} KB`;
  }
  return `${Math.round(bytes)} B`;
}

export function formatEtaText(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds) || seconds < 0) {
    return 'menghitung…';
  }
  const rounded = Math.ceil(seconds);
  if (rounded < 60) {
    return `${rounded} dtk`;
  }
  const mins = Math.floor(rounded / 60);
  const secs = rounded % 60;
  return `${mins} mnt ${secs} dtk`;
}

export async function copyTextToClipboard(text: string): Promise<void> {
  if (typeof navigator !== 'undefined' && navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }
  throw new Error('Clipboard API unavailable');
}
