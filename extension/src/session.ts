// One captured stream (one tab) as seen by the bridge: start / resume handshake, frame
// numbering, the buffer that holds frames while the connection is down, and the counters of
// the `stop` message. No audio code lives here, so tests drive it with a fake link.
//
// Status:
//   starting   `start` sent, waiting for the answer; frames are kept
//   waiting    the application is not recording (or declined); frames are discarded
//   active     start_ok received; frames flow
//   buffering  connection lost; frames are kept (at most 30 s) until `resume` is answered
//   ended      stopped; nothing more happens

import {
  FRAME_MS,
  packFrame,
  startMessage,
  stopMessage,
  type AppState,
  type StartOk,
  type StartRefused,
  type StopReason,
} from "./protocol.js";

/** Frames kept while disconnected: 30 s (about 960 KB of audio). */
export const MAX_PENDING_FRAMES = (30 * 1000) / FRAME_MS;
/** Frames sent per pump while a backlog drains (the application takes 1500 queued frames). */
const DRAIN_BATCH = 25;
const DRAIN_INTERVAL_MS = 5;
/** Backlog length that switches from "send everything now" to throttled draining. */
const BACKLOG = 3;
/** Stop writing to a socket whose own buffer holds more than this. */
const SOCKET_LIMIT_BYTES = 512 * 1024;

export type SessionStatus = "starting" | "waiting" | "active" | "buffering" | "ended";

export interface Link {
  readonly isReady: boolean;
  readonly bufferedAmount: number;
  sendText(msg: unknown): boolean;
  sendBinary(buf: ArrayBuffer): boolean;
}

export interface StreamInfo {
  streamId: string;
  tabId: number;
  groupId?: number;
  title: string;
  host: string;
}

interface Pending {
  seq: number;
  sentMs: number;
  pcm: Int16Array;
}

export class StreamSession {
  status: SessionStatus = "starting";
  streamNo: number | null = null;
  track: string | null = null;
  framesSent = 0;
  samplesSent = 0;
  framesDropped = 0;
  /** Sequence number of the next frame the worklet produces. */
  nextSeq = 0;
  private pending: Pending[] = [];
  private pumpTimer: ReturnType<typeof setTimeout> | null = null;
  /** True once the application accepted this stream on some connection. */
  private accepted = false;

  constructor(
    readonly info: StreamInfo,
    private readonly link: Link,
    private readonly maxPending = MAX_PENDING_FRAMES,
  ) {}

  /** The pending buffer length, for tests and the status report. */
  get pendingFrames(): number {
    return this.pending.length;
  }

  /** The socket is ready (first connection or after a reconnect): start or resume. */
  onConnected(): void {
    if (this.status === "ended") return;
    // A waiting stream offers itself again: the application lists the tabs it waits for, and
    // it forgot them when the connection was lost. It starts recording when the application
    // reports a recording (onAppState).
    if (this.accepted) {
      this.status = "starting";
      const first = this.pending[0]?.seq ?? this.nextSeq;
      this.link.sendText({ type: "resume", stream_id: this.info.streamId, next_seq: first });
    } else {
      this.sendStart();
    }
  }

  /** The socket was lost. */
  onDisconnected(): void {
    if (this.status === "active" || this.status === "starting") {
      this.status = this.accepted ? "buffering" : "starting";
    }
    this.cancelPump();
  }

  private sendStart(): void {
    this.status = "starting";
    this.link.sendText(startMessage(this.info));
  }

  /** One frame from the worklet. */
  onFrame(pcm: Int16Array, sentMs: number): void {
    if (this.status === "ended") return;
    if (this.status === "waiting") return; // nothing is recorded: discard, no sequence number
    this.pending.push({ seq: this.nextSeq, sentMs, pcm });
    this.nextSeq += 1;
    while (this.pending.length > this.maxPending) {
      this.pending.shift();
      this.framesDropped += 1; // the receiver sees the hole and journals it
    }
    if (this.status === "active") this.pump();
  }

  onStartOk(msg: StartOk): void {
    if (msg.stream_id !== this.info.streamId || this.status === "ended") return;
    this.streamNo = msg.stream_no;
    this.track = msg.track;
    this.accepted = true;
    this.status = "active";
    this.pump();
  }

  onStartRefused(msg: StartRefused): void {
    if (msg.stream_id !== this.info.streamId || this.status === "ended") return;
    if (msg.reason === "unknown_stream" && this.accepted) {
      // The application forgot the stream (restart): offer it again; frames stay buffered.
      this.accepted = false;
      this.streamNo = null;
      this.sendStart();
      return;
    }
    // Not recording, declined or duplicate: keep the capture and wait for a recording.
    this.accepted = false;
    this.streamNo = null;
    this.status = "waiting";
    this.pending = [];
    this.nextSeq = 0;
    this.cancelPump();
  }

  /**
   * The state the application reports. Returns true when the recording ended and this
   * stream was part of it, so the owner releases the tab.
   */
  onAppState(state: AppState): boolean {
    if (this.status === "ended") return false;
    if (!state.recording) {
      if (this.accepted) return true;
      return false;
    }
    if (this.status === "waiting") this.sendStart();
    return false;
  }

  /** Stop the stream: flush what can be sent, tell the application. Returns the stats. */
  stop(reason: StopReason): void {
    if (this.status === "ended") return;
    if (this.status === "active") this.pumpAll();
    const wasAccepted = this.accepted;
    // A waiting tab is on offer in the application: tell it that the tab is gone.
    const wasOffered = this.status === "waiting";
    this.status = "ended";
    this.cancelPump();
    if (wasAccepted || wasOffered || this.pending.length > 0) {
      this.link.sendText(
        stopMessage(this.info.streamId, reason, {
          framesSent: this.framesSent,
          samplesSent: this.samplesSent,
          framesDropped: this.framesDropped,
        }),
      );
    }
    this.pending = [];
  }

  /** Mark ended without telling the application (it ended the stream itself). */
  end(): void {
    this.status = "ended";
    this.cancelPump();
    this.pending = [];
  }

  // --- sending ----------------------------------------------------------------------

  private cancelPump(): void {
    if (this.pumpTimer !== null) clearTimeout(this.pumpTimer);
    this.pumpTimer = null;
  }

  private sendOne(f: Pending): boolean {
    if (this.streamNo === null) return false;
    if (!this.link.sendBinary(packFrame(this.streamNo, f.seq, f.sentMs, f.pcm))) return false;
    this.framesSent += 1;
    this.samplesSent += f.pcm.length;
    return true;
  }

  private pumpAll(): void {
    while (this.pending.length > 0 && this.sendOne(this.pending[0])) this.pending.shift();
  }

  private pump(): void {
    if (this.status !== "active" || !this.link.isReady) return;
    if (this.pumpTimer !== null) return; // the timer continues the drain in order
    const backlog = this.pending.length > BACKLOG;
    let budget = backlog ? DRAIN_BATCH : this.pending.length;
    while (budget > 0 && this.pending.length > 0 && this.link.bufferedAmount < SOCKET_LIMIT_BYTES) {
      if (!this.sendOne(this.pending[0])) break;
      this.pending.shift();
      budget -= 1;
    }
    if (this.pending.length > 0) {
      this.pumpTimer = setTimeout(() => {
        this.pumpTimer = null;
        this.pump();
      }, DRAIN_INTERVAL_MS);
    }
  }
}
