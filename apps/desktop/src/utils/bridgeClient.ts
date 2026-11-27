export const BRIDGE_MAX_MESSAGE_BYTES = 2048;
export const AD_DURATION_TOLERANCE_SECONDS = 2.0;

export type BridgePreviewState =
  | 'loading'
  | 'ready'
  | 'playing'
  | 'paused'
  | 'buffering'
  | 'ended'
  | 'ad'
  | 'fallback';

export type BridgeFallbackReason =
  | 'playable_in_embed_false'
  | 'ready_timeout'
  | 'protocol_mismatch'
  | 'embed_disabled'
  | 'config_error_153'
  | 'config_error_control'
  | 'video_not_found'
  | 'yt_invalid_param'
  | 'yt_html5_error'
  | 'forced_debug';

export interface ExtrapolateTimeInput {
  lastCurrentSeconds: number;
  lastTimestampMs: number;
  rate: number;
  nowMs: number;
  isPlaying: boolean;
  durationSeconds: number;
}

/**
 * Extrapolates current playback position between bridge `time` ticks (4-17 Hz)
 * per S2-preview.md §6.3: `displayTime = lastCurrent + (now - ts) / 1000 * rate`.
 */
export function extrapolatePlaybackTime(input: ExtrapolateTimeInput): number {
  const { lastCurrentSeconds, lastTimestampMs, rate, nowMs, isPlaying, durationSeconds } = input;
  const safeBase = Number.isFinite(lastCurrentSeconds) ? Math.max(0, lastCurrentSeconds) : 0;
  const safeDuration =
    Number.isFinite(durationSeconds) && durationSeconds > 0 ? durationSeconds : Infinity;

  if (!isPlaying) {
    return Math.min(safeDuration, safeBase);
  }

  const safeRate = Number.isFinite(rate) && rate >= 0.25 && rate <= 2.0 ? rate : 1.0;
  const elapsedSecs = Math.max(0, (nowMs - lastTimestampMs) / 1000);
  // Clamp extrapolation gap cleanly so suspended tabs don't jump far ahead
  const boundedElapsed = Math.min(elapsedSecs, 2.0);
  return Math.min(safeDuration, safeBase + boundedElapsed * safeRate);
}

/**
 * Detects pre-roll/mid-roll ads by comparing player-reported duration against probe duration.
 */
export function isAdDurationMismatch(
  playerDurationSeconds: number,
  probeDurationSeconds: number
): boolean {
  if (
    !Number.isFinite(playerDurationSeconds) ||
    !Number.isFinite(probeDurationSeconds) ||
    playerDurationSeconds <= 0 ||
    probeDurationSeconds <= 0
  ) {
    return false;
  }
  return Math.abs(playerDurationSeconds - probeDurationSeconds) > AD_DURATION_TOLERANCE_SECONDS;
}

export type HostOutboundCommand =
  | { type: 'hello'; v: number; nonce: string }
  | { type: 'load'; videoId: string; startSeconds: number }
  | { type: 'play' }
  | { type: 'pause' }
  | { type: 'seek'; seconds: number }
  | { type: 'setRate'; rate: number }
  | { type: 'mute'; bool: boolean };

export interface BridgeFallbackInfo {
  reason: BridgeFallbackReason;
  errorCode: 'E_PREVIEW_CONFIG' | 'E_YT_UNAVAILABLE' | null;
  ytErrorCode: number | null;
  bridgeReadyMs: number | null;
  counterKind: 'embed_disabled' | 'fallback' | 'config_error' | null;
}

export interface BridgeTimeSnapshot {
  currentSeconds: number;
  durationSeconds: number;
  rate: number;
  ts: number;
}

export interface BridgeSessionOptions {
  targetOrigin: string;
  expectedProtocolVersion: number;
  readyTimeoutMs: number;
  videoId: string;
  initialStartSeconds?: number;
  probeDurationSeconds: number;
  controlVideoId: string;
  nonce?: string;
  now?: () => number;
  postToIframe: (cmd: HostOutboundCommand, targetOrigin: string) => void;
  getIframeSource: () => unknown;
  onStateChange?: (state: BridgePreviewState) => void;
  onReady?: (bridgeReadyMs: number) => void;
  onTimeSnapshot?: (snapshot: BridgeTimeSnapshot) => void;
  onAdStateChange?: (isAd: boolean) => void;
  onFallback?: (info: BridgeFallbackInfo) => void;
}

function generateNonce(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID().replace(/-/g, '').slice(0, 24);
  }
  return `n_${Math.random().toString(36).slice(2, 14)}`;
}

/**
 * Protocol v1 host-side bridge session managing nonce handshake, 6 s timeout,
 * 4-17 Hz time snapshots, ad mismatch guard, and control-video differentiation on YT 101/150.
 */
export class BridgeSession {
  private readonly opts: BridgeSessionOptions;
  private readonly nonce: string;
  private readonly now: () => number;

  private startedAtMs = 0;
  private bridgeReadyMs: number | null = null;
  private handshakeCompleted = false;
  private state: BridgePreviewState = 'loading';
  private checkingControlVideo = false;
  private pendingYtErrorForControl: number | null = null;
  private controlTimer: ReturnType<typeof setTimeout> | null = null;
  private readyTimer: ReturnType<typeof setTimeout> | null = null;
  private disposed = false;

  constructor(opts: BridgeSessionOptions) {
    this.opts = opts;
    this.nonce = opts.nonce ?? generateNonce();
    this.now = opts.now ?? (() => Date.now());
  }

  public getNonce(): string {
    return this.nonce;
  }

  public getState(): BridgePreviewState {
    return this.state;
  }

  public getBridgeReadyMs(): number | null {
    return this.bridgeReadyMs;
  }

  public startHandshake(): void {
    if (this.disposed) return;
    this.startedAtMs = this.now();
    this.handshakeCompleted = false;
    this.clearTimers();
    this.setState('loading');

    this.readyTimer = setTimeout(() => {
      if (!this.handshakeCompleted && !this.disposed) {
        this.triggerFallback({
          reason: 'ready_timeout',
          errorCode: null,
          ytErrorCode: null,
          bridgeReadyMs: null,
          counterKind: 'fallback',
        });
      }
    }, this.opts.readyTimeoutMs);

    this.sendCommand({
      type: 'hello',
      v: this.opts.expectedProtocolVersion,
      nonce: this.nonce,
    });
  }

  public handleMessageEvent(event: { origin: string; source: unknown; data: unknown }): boolean {
    if (this.disposed) return false;
    if (event.origin !== this.opts.targetOrigin) return false;

    const iframeSource = this.opts.getIframeSource();
    if (iframeSource !== null && event.source !== iframeSource) return false;

    if (!event.data || typeof event.data !== 'object' || Array.isArray(event.data)) {
      return false;
    }

    try {
      if (JSON.stringify(event.data).length > BRIDGE_MAX_MESSAGE_BYTES) {
        return false;
      }
    } catch {
      return false;
    }

    const msg = event.data as Record<string, unknown>;
    if (typeof msg.type !== 'string') return false;

    if (msg.type === 'ready') {
      if (msg.nonce !== this.nonce) {
        return false;
      }
      if (msg.v !== this.opts.expectedProtocolVersion) {
        this.clearTimers();
        this.triggerFallback({
          reason: 'protocol_mismatch',
          errorCode: 'E_PREVIEW_CONFIG',
          ytErrorCode: null,
          bridgeReadyMs: null,
          counterKind: 'config_error',
        });
        return true;
      }

      if (!this.handshakeCompleted) {
        this.handshakeCompleted = true;
        if (this.readyTimer !== null) {
          clearTimeout(this.readyTimer);
          this.readyTimer = null;
        }
        this.bridgeReadyMs = Math.max(0, Math.round(this.now() - this.startedAtMs));
        this.opts.onReady?.(this.bridgeReadyMs);
        this.sendCommand({
          type: 'load',
          videoId: this.opts.videoId,
          startSeconds: this.opts.initialStartSeconds ?? 0,
        });
      }
      return true;
    }

    if (!this.handshakeCompleted) {
      return false;
    }

    if (msg.type === 'state' && typeof msg.state === 'string') {
      if (this.checkingControlVideo) {
        if (msg.state === 'ready' || msg.state === 'playing' || msg.state === 'cued') {
          this.clearControlTimer();
          this.checkingControlVideo = false;
          // Control video succeeded => primary video is genuinely owner-restricted (not config error).
          this.triggerFallback({
            reason: 'embed_disabled',
            errorCode: null,
            ytErrorCode: this.pendingYtErrorForControl,
            bridgeReadyMs: this.bridgeReadyMs,
            counterKind: 'embed_disabled',
          });
          return true;
        }
        return true;
      }

      const mapped = mapIncomingState(msg.state);
      if (mapped && this.state !== 'ad' && this.state !== 'fallback') {
        this.setState(mapped);
      }
      return true;
    }

    if (msg.type === 'time') {
      if (this.checkingControlVideo) return true;
      const current = Number(msg.current);
      const duration = Number(msg.duration);
      const rate = Number(msg.rate ?? 1);
      const ts = Number(msg.ts ?? this.now());
      if (!Number.isFinite(current) || !Number.isFinite(duration)) return false;

      const isAd = isAdDurationMismatch(duration, this.opts.probeDurationSeconds);
      this.opts.onAdStateChange?.(isAd);
      if (isAd && this.state !== 'ad') {
        this.setState('ad');
      } else if (!isAd && this.state === 'ad') {
        this.setState('playing');
      }

      this.opts.onTimeSnapshot?.({
        currentSeconds: current,
        durationSeconds: duration,
        rate: Number.isFinite(rate) ? rate : 1,
        ts: Number.isFinite(ts) ? ts : this.now(),
      });
      return true;
    }

    if (msg.type === 'error' && typeof msg.code === 'number') {
      const code = msg.code;
      if (this.checkingControlVideo) {
        this.clearControlTimer();
        this.checkingControlVideo = false;
        // Control video ALSO failed => OUR bridge/origin configuration is broken!
        this.triggerFallback({
          reason: 'config_error_control',
          errorCode: 'E_PREVIEW_CONFIG',
          ytErrorCode: code,
          bridgeReadyMs: this.bridgeReadyMs,
          counterKind: 'config_error',
        });
        return true;
      }

      if (code === 153) {
        this.triggerFallback({
          reason: 'config_error_153',
          errorCode: 'E_PREVIEW_CONFIG',
          ytErrorCode: 153,
          bridgeReadyMs: this.bridgeReadyMs,
          counterKind: 'config_error',
        });
      } else if (code === 101 || code === 150) {
        this.startControlVideoCheck(code);
      } else if (code === 100) {
        this.triggerFallback({
          reason: 'video_not_found',
          errorCode: 'E_YT_UNAVAILABLE',
          ytErrorCode: 100,
          bridgeReadyMs: this.bridgeReadyMs,
          counterKind: 'fallback',
        });
      } else {
        this.triggerFallback({
          reason: code === 2 ? 'yt_invalid_param' : 'yt_html5_error',
          errorCode: null,
          ytErrorCode: code,
          bridgeReadyMs: this.bridgeReadyMs,
          counterKind: 'fallback',
        });
      }
      return true;
    }

    return false;
  }

  public sendPlay(): void {
    if (this.handshakeCompleted && !this.disposed) this.sendCommand({ type: 'play' });
  }

  public sendPause(): void {
    if (this.handshakeCompleted && !this.disposed) this.sendCommand({ type: 'pause' });
  }

  public sendSeek(seconds: number): void {
    if (this.handshakeCompleted && !this.disposed && Number.isFinite(seconds) && seconds >= 0) {
      this.sendCommand({ type: 'seek', seconds });
    }
  }

  public sendRate(rate: number): void {
    if (this.handshakeCompleted && !this.disposed && rate >= 0.25 && rate <= 2.0) {
      this.sendCommand({ type: 'setRate', rate });
    }
  }

  public sendMute(bool: boolean): void {
    if (this.handshakeCompleted && !this.disposed) this.sendCommand({ type: 'mute', bool });
  }

  public dispose(): void {
    this.disposed = true;
    this.clearTimers();
  }

  private startControlVideoCheck(primaryYtCode: number): void {
    if (!this.opts.controlVideoId || this.opts.controlVideoId === this.opts.videoId) {
      this.triggerFallback({
        reason: 'embed_disabled',
        errorCode: null,
        ytErrorCode: primaryYtCode,
        bridgeReadyMs: this.bridgeReadyMs,
        counterKind: 'embed_disabled',
      });
      return;
    }

    this.checkingControlVideo = true;
    this.pendingYtErrorForControl = primaryYtCode;
    this.clearControlTimer();
    this.controlTimer = setTimeout(() => {
      if (this.checkingControlVideo && !this.disposed) {
        this.checkingControlVideo = false;
        this.triggerFallback({
          reason: 'config_error_control',
          errorCode: 'E_PREVIEW_CONFIG',
          ytErrorCode: primaryYtCode,
          bridgeReadyMs: this.bridgeReadyMs,
          counterKind: 'config_error',
        });
      }
    }, this.opts.readyTimeoutMs);

    this.sendCommand({
      type: 'load',
      videoId: this.opts.controlVideoId,
      startSeconds: 0,
    });
  }

  private sendCommand(cmd: HostOutboundCommand): void {
    if (!this.opts.targetOrigin || this.opts.targetOrigin === '*') {
      throw new Error('Bridge targetOrigin must never be wildcard "*"');
    }
    this.opts.postToIframe(cmd, this.opts.targetOrigin);
  }

  private setState(next: BridgePreviewState): void {
    this.state = next;
    this.opts.onStateChange?.(next);
  }

  private triggerFallback(info: BridgeFallbackInfo): void {
    this.clearTimers();
    this.setState('fallback');
    this.opts.onFallback?.(info);
  }

  private clearControlTimer(): void {
    if (this.controlTimer !== null) {
      clearTimeout(this.controlTimer);
      this.controlTimer = null;
    }
  }

  private clearTimers(): void {
    if (this.readyTimer !== null) {
      clearTimeout(this.readyTimer);
      this.readyTimer = null;
    }
    this.clearControlTimer();
  }
}

function mapIncomingState(raw: string): BridgePreviewState | null {
  switch (raw) {
    case 'loading':
    case 'cued':
      return 'loading';
    case 'ready':
      return 'ready';
    case 'playing':
      return 'playing';
    case 'paused':
      return 'paused';
    case 'buffering':
      return 'buffering';
    case 'ended':
      return 'ended';
    default:
      return null;
  }
}
