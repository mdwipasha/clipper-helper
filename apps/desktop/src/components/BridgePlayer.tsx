import { forwardRef, useEffect, useImperativeHandle, useMemo, useRef, useState } from 'react';
import {
  BridgeFallbackInfo,
  BridgePreviewState,
  BridgeSession,
  BridgeTimeSnapshot,
  extrapolatePlaybackTime,
  HostOutboundCommand,
} from '../utils/bridgeClient';

export interface BridgePlayerControls {
  play: () => void;
  pause: () => void;
  seek: (seconds: number) => void;
  setRate: (rate: number) => void;
  mute: (muted: boolean) => void;
}

interface BridgePlayerProps {
  bridgeUrl: string;
  protocolVersion: number;
  readyTimeoutMs: number;
  controlVideoId: string;
  videoId: string;
  probeDurationSeconds: number;
  initialStartSeconds?: number;
  loopStartSeconds?: number | null;
  loopEndSeconds?: number | null;
  loopEnabled?: boolean;
  onStateChange?: (state: BridgePreviewState) => void;
  onTimeChange?: (extrapolatedSeconds: number) => void;
  onAdStateChange?: (isAd: boolean) => void;
  onReady?: (bridgeReadyMs: number) => void;
  onFallback?: (info: BridgeFallbackInfo) => void;
}

export const BridgePlayer = forwardRef<BridgePlayerControls, BridgePlayerProps>(
  function BridgePlayer(props, ref) {
    const {
      bridgeUrl,
      protocolVersion,
      readyTimeoutMs,
      controlVideoId,
      videoId,
      probeDurationSeconds,
      initialStartSeconds = 0,
      loopStartSeconds,
      loopEndSeconds,
      loopEnabled = false,
      onStateChange,
      onTimeChange,
      onAdStateChange,
      onReady,
      onFallback,
    } = props;

    const iframeRef = useRef<HTMLIFrameElement | null>(null);
    const sessionRef = useRef<BridgeSession | null>(null);
    const lastSnapshotRef = useRef<BridgeTimeSnapshot>({
      currentSeconds: initialStartSeconds,
      durationSeconds: probeDurationSeconds,
      rate: 1,
      ts: Date.now(),
    });
    const playerStateRef = useRef<BridgePreviewState>('loading');
    const [iframeLoaded, setIframeLoaded] = useState(false);

    const bridgeOrigin = useMemo(() => {
      try {
        return new URL(bridgeUrl).origin;
      } catch {
        return 'https://clipper-bridge.vercel.app';
      }
    }, [bridgeUrl]);

    const callbacksRef = useRef({
      onStateChange,
      onTimeChange,
      onAdStateChange,
      onReady,
      onFallback,
    });
    callbacksRef.current = {
      onStateChange,
      onTimeChange,
      onAdStateChange,
      onReady,
      onFallback,
    };

    const loopRef = useRef({
      loopEnabled,
      loopStartSeconds,
      loopEndSeconds,
    });
    loopRef.current = {
      loopEnabled,
      loopStartSeconds,
      loopEndSeconds,
    };

    useImperativeHandle(
      ref,
      () => ({
        play: () => sessionRef.current?.sendPlay(),
        pause: () => sessionRef.current?.sendPause(),
        seek: (seconds: number) => {
          sessionRef.current?.sendSeek(seconds);
          lastSnapshotRef.current = {
            ...lastSnapshotRef.current,
            currentSeconds: seconds,
            ts: Date.now(),
          };
          callbacksRef.current.onTimeChange?.(seconds);
        },
        setRate: (rate: number) => sessionRef.current?.sendRate(rate),
        mute: (muted: boolean) => sessionRef.current?.sendMute(muted),
      }),
      []
    );

    useEffect(() => {
      const session = new BridgeSession({
        targetOrigin: bridgeOrigin,
        expectedProtocolVersion: protocolVersion,
        readyTimeoutMs,
        controlVideoId,
        videoId,
        initialStartSeconds,
        probeDurationSeconds,
        getIframeSource: () => iframeRef.current?.contentWindow ?? null,
        postToIframe: (cmd: HostOutboundCommand, targetOrigin: string) => {
          iframeRef.current?.contentWindow?.postMessage(cmd, targetOrigin);
        },
        onStateChange: (next) => {
          playerStateRef.current = next;
          callbacksRef.current.onStateChange?.(next);
        },
        onReady: (ms) => {
          callbacksRef.current.onReady?.(ms);
        },
        onAdStateChange: (isAd) => {
          callbacksRef.current.onAdStateChange?.(isAd);
        },
        onTimeSnapshot: (snap) => {
          lastSnapshotRef.current = snap;
          callbacksRef.current.onTimeChange?.(snap.currentSeconds);
        },
        onFallback: (info) => {
          callbacksRef.current.onFallback?.(info);
        },
      });

      sessionRef.current = session;
      setIframeLoaded(false);

      const handleMessage = (ev: MessageEvent) => {
        session.handleMessageEvent(ev);
      };
      window.addEventListener('message', handleMessage);

      // Start handshake timer immediately (cover case where iframe never fires onLoad when offline)
      session.startHandshake();

      const clockHandle = window.setInterval(() => {
        const current = extrapolatePlaybackTime({
          lastCurrentSeconds: lastSnapshotRef.current.currentSeconds,
          lastTimestampMs: lastSnapshotRef.current.ts,
          rate: lastSnapshotRef.current.rate,
          nowMs: Date.now(),
          isPlaying: playerStateRef.current === 'playing',
          durationSeconds: lastSnapshotRef.current.durationSeconds || probeDurationSeconds,
        });
        callbacksRef.current.onTimeChange?.(current);

        const lp = loopRef.current;
        if (
          lp.loopEnabled &&
          playerStateRef.current === 'playing' &&
          typeof lp.loopStartSeconds === 'number' &&
          typeof lp.loopEndSeconds === 'number' &&
          lp.loopEndSeconds > lp.loopStartSeconds &&
          current >= lp.loopEndSeconds
        ) {
          session.sendSeek(lp.loopStartSeconds);
          lastSnapshotRef.current = {
            ...lastSnapshotRef.current,
            currentSeconds: lp.loopStartSeconds,
            ts: Date.now(),
          };
        }
      }, 60);

      return () => {
        window.clearInterval(clockHandle);
        window.removeEventListener('message', handleMessage);
        session.dispose();
        sessionRef.current = null;
      };
    }, [
      bridgeOrigin,
      controlVideoId,
      initialStartSeconds,
      probeDurationSeconds,
      protocolVersion,
      readyTimeoutMs,
      videoId,
    ]);

    const handleIframeLoad = () => {
      setIframeLoaded(true);
      sessionRef.current?.startHandshake();
    };

    return (
      <div
        data-testid="bridge-player-frame-container"
        className="w-full aspect-video min-w-[200px] min-h-[200px] rounded-xl overflow-hidden bg-slate-950 border border-slate-800"
      >
        <iframe
          ref={iframeRef}
          title="YouTube Preview Player"
          src={bridgeUrl}
          data-loaded={iframeLoaded ? 'true' : 'false'}
          onLoad={handleIframeLoad}
          referrerPolicy="strict-origin-when-cross-origin"
          sandbox="allow-scripts allow-same-origin allow-presentation"
          allow="autoplay; encrypted-media"
          className="w-full h-full border-0 block"
        />
      </div>
    );
  }
);
