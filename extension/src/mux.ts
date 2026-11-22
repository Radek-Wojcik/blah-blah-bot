// Several captured MediaStreams over one bridge connection, inside the offscreen document.
//
// Two AudioContexts are shared by all streams:
//   playback (default rate) : source -> destination, so the tab stays audible;
//   capture  (16 kHz)       : source -> worklet -> (muted gain) -> destination;
//                             the browser resamples to 16 kHz before the worklet sees it.
// Each stream is a StreamSession (handshake, buffer, counters); this class only connects the
// audio graph to it.

import {
  FRAME_SAMPLES,
  SAMPLE_RATE,
  type AppState,
  type StartOk,
  type StartRefused,
  type StopReason,
} from "./protocol.js";
import { StreamSession, type Link, type StreamInfo } from "./session.js";

interface Rec {
  session: StreamSession;
  mediaStream: MediaStream;
  nodes: AudioNode[];
  worklet: AudioWorkletNode;
  onFlushed?: () => void;
}

export interface StreamStatusInfo {
  tabId: number;
  streamId: string;
  status: string;
  track: string | null;
  framesSent: number;
  framesDropped: number;
  pendingFrames: number;
}

export class StreamMux {
  private readonly streams = new Map<number, Rec>();
  private playbackCtx: AudioContext | null = null;
  private captureCtx: AudioContext | null = null;
  private ready: Promise<void> | null = null;

  constructor(
    private readonly link: Link,
    private readonly opts: {
      workletUrl: string;
      /** A stream ended on its own or by the application (the tab is released already). */
      onEnded: (tabId: number, streamId: string, reason: StopReason | "recording_ended") => void;
    },
  ) {}

  private init(): Promise<void> {
    this.ready ??= (async () => {
      this.playbackCtx = new AudioContext({ latencyHint: "playback" });
      this.captureCtx = new AudioContext({ sampleRate: SAMPLE_RATE, latencyHint: "interactive" });
      await this.captureCtx.audioWorklet.addModule(this.opts.workletUrl);
      await Promise.all([this.playbackCtx.resume(), this.captureCtx.resume()]);
    })();
    return this.ready;
  }

  has(tabId: number): boolean {
    return this.streams.has(tabId);
  }

  /** Adds a stream; `audible` replays it to the default output (false for synthetic tests). */
  async add(info: StreamInfo, mediaStream: MediaStream, audible = true): Promise<void> {
    if (this.streams.has(info.tabId)) throw new Error(`tab ${info.tabId} is already streaming`);
    const track = mediaStream.getAudioTracks()[0];
    if (!track) throw new Error("stream has no audio track");
    await this.init();
    const playback = this.playbackCtx;
    const capture = this.captureCtx;
    if (!playback || !capture) throw new Error("audio contexts are not ready");

    const session = new StreamSession(info, this.link);
    const nodes: AudioNode[] = [];
    if (audible) {
      const play = playback.createMediaStreamSource(mediaStream);
      play.connect(playback.destination);
      nodes.push(play);
    }
    const src = capture.createMediaStreamSource(mediaStream);
    const worklet = new AudioWorkletNode(capture, "pcm-framer", {
      numberOfInputs: 1,
      numberOfOutputs: 1,
      outputChannelCount: [1],
      processorOptions: { frameSamples: FRAME_SAMPLES },
    });
    const mute = capture.createGain();
    mute.gain.value = 0;
    src.connect(worklet);
    worklet.connect(mute);
    mute.connect(capture.destination);
    nodes.push(src, worklet, mute);

    const rec: Rec = { session, mediaStream, nodes, worklet };
    worklet.port.onmessage = (ev: MessageEvent) => {
      const data = ev.data as { type: string; pcm?: ArrayBuffer };
      if (data.type === "pcm" && data.pcm) {
        session.onFrame(new Int16Array(data.pcm), performance.timeOrigin + performance.now());
      } else if (data.type === "flushed") {
        rec.onFlushed?.();
      }
    };
    track.addEventListener("ended", () => void this.stop(info.tabId, "track_ended"));

    this.streams.set(info.tabId, rec);
    if (this.link.isReady) session.onConnected();
  }

  /** Stops one stream cleanly: flush the partial frame, send `stop`, release the tab. */
  async stop(tabId: number, reason: StopReason = "user_stop"): Promise<void> {
    const rec = this.streams.get(tabId);
    if (!rec || rec.session.status === "ended") return;
    // Mark first so a second stop (the ended event of the track) does nothing.
    this.streams.delete(tabId);
    await new Promise<void>((resolve) => {
      rec.onFlushed = resolve;
      rec.worklet.port.postMessage({ type: "flush" });
      setTimeout(resolve, 200);
    });
    rec.session.stop(reason);
    this.release(rec);
    this.opts.onEnded(tabId, rec.session.info.streamId, reason);
  }

  async stopAll(reason: StopReason = "user_stop"): Promise<void> {
    await Promise.all([...this.streams.keys()].map((id) => this.stop(id, reason)));
  }

  private release(rec: Rec): void {
    for (const node of rec.nodes) node.disconnect();
    for (const t of rec.mediaStream.getTracks()) t.stop();
  }

  // --- events from the bridge client --------------------------------------------------

  onConnected(): void {
    for (const r of this.streams.values()) r.session.onConnected();
  }

  onDisconnected(): void {
    for (const r of this.streams.values()) r.session.onDisconnected();
  }

  onStartOk(msg: StartOk): void {
    for (const r of this.streams.values()) r.session.onStartOk(msg);
  }

  onStartRefused(msg: StartRefused): void {
    for (const r of this.streams.values()) r.session.onStartRefused(msg);
  }

  /** The application reports its state; streams of a finished recording are released. */
  onAppState(state: AppState): void {
    for (const [tabId, r] of [...this.streams]) {
      if (r.session.onAppState(state)) {
        this.streams.delete(tabId);
        r.session.end();
        this.release(r);
        this.opts.onEnded(tabId, r.session.info.streamId, "recording_ended");
      }
    }
  }

  status(): StreamStatusInfo[] {
    return [...this.streams.values()].map((r) => ({
      tabId: r.session.info.tabId,
      streamId: r.session.info.streamId,
      status: r.session.status,
      track: r.session.track,
      framesSent: r.session.framesSent,
      framesDropped: r.session.framesDropped,
      pendingFrames: r.session.pendingFrames,
    }));
  }
}
