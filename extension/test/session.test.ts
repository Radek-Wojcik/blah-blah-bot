// Stream handshake, buffering and resume with a fake link (no audio, no socket).
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { parseFrame } from "../src/protocol.js";
import { MAX_PENDING_FRAMES, StreamSession, type Link } from "../src/session.js";

class FakeLink implements Link {
  isReady = true;
  bufferedAmount = 0;
  texts: Record<string, unknown>[] = [];
  frames: { streamNo: number; seq: number }[] = [];
  sendText(msg: unknown): boolean {
    if (!this.isReady) return false;
    this.texts.push(msg as Record<string, unknown>);
    return true;
  }
  sendBinary(buf: ArrayBuffer): boolean {
    if (!this.isReady) return false;
    const f = parseFrame(buf);
    if (f) this.frames.push({ streamNo: f.streamNo, seq: f.seq });
    return true;
  }
  types(): string[] {
    return this.texts.map((t) => String(t.type));
  }
}

const info = { streamId: "t1-a", tabId: 1, title: "T", host: "tone.test" };
const pcm = () => new Int16Array(320);

function make(max?: number) {
  const link = new FakeLink();
  const s = new StreamSession(info, link, max);
  return { link, s };
}
function accept(s: StreamSession, no = 1) {
  s.onConnected();
  s.onStartOk({ type: "start_ok", stream_id: "t1-a", stream_no: no, track: "tab" });
}

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe("start", () => {
  it("sends start, keeps frames until start_ok, then sends them in order", () => {
    const { link, s } = make();
    s.onConnected();
    expect(link.types()).toEqual(["start"]);
    s.onFrame(pcm(), 1);
    s.onFrame(pcm(), 2);
    expect(link.frames).toHaveLength(0);
    s.onStartOk({ type: "start_ok", stream_id: "t1-a", stream_no: 5, track: "tab" });
    expect(link.frames).toEqual([
      { streamNo: 5, seq: 0 },
      { streamNo: 5, seq: 1 },
    ]);
    s.onFrame(pcm(), 3);
    expect(link.frames.at(-1)).toEqual({ streamNo: 5, seq: 2 });
    expect(s.status).toBe("active");
  });

  it("ignores answers for other streams", () => {
    const { s } = make();
    s.onConnected();
    s.onStartOk({ type: "start_ok", stream_id: "other", stream_no: 9, track: "tab" });
    expect(s.status).toBe("starting");
  });

  it("waits when the application is idle and discards frames without numbering them", () => {
    const { link, s } = make();
    s.onConnected();
    s.onFrame(pcm(), 1);
    s.onStartRefused({ type: "start_refused", stream_id: "t1-a", reason: "not_recording" });
    expect(s.status).toBe("waiting");
    s.onFrame(pcm(), 2);
    expect(s.pendingFrames).toBe(0);
    expect(s.nextSeq).toBe(0);
    expect(link.frames).toHaveLength(0);
  });

  it("starts again when the application reports a recording", () => {
    const { link, s } = make();
    s.onConnected();
    s.onStartRefused({ type: "start_refused", stream_id: "t1-a", reason: "not_recording" });
    link.texts.length = 0;
    expect(s.onAppState({ type: "state", recording: true, streams: [] })).toBe(false);
    expect(link.types()).toEqual(["start"]);
    s.onStartOk({ type: "start_ok", stream_id: "t1-a", stream_no: 2, track: "tab" });
    s.onFrame(pcm(), 1);
    expect(link.frames).toEqual([{ streamNo: 2, seq: 0 }]);
  });

  it("does not start twice when connecting while waiting", () => {
    const { link, s } = make();
    s.onConnected();
    s.onStartRefused({ type: "start_refused", stream_id: "t1-a", reason: "not_recording" });
    link.texts.length = 0;
    s.onConnected();
    expect(link.texts).toHaveLength(0);
  });
});

describe("reconnect and resume", () => {
  it("buffers while disconnected and resumes with the first unsent sequence number", () => {
    const { link, s } = make();
    accept(s);
    s.onFrame(pcm(), 1);
    s.onFrame(pcm(), 2);
    expect(link.frames).toHaveLength(2);

    link.isReady = false;
    s.onDisconnected();
    expect(s.status).toBe("buffering");
    for (let i = 0; i < 5; i++) s.onFrame(pcm(), 3 + i);
    expect(link.frames).toHaveLength(2);
    expect(s.pendingFrames).toBe(5);

    link.isReady = true;
    link.texts.length = 0;
    s.onConnected();
    expect(link.texts).toEqual([{ type: "resume", stream_id: "t1-a", next_seq: 2 }]);
    s.onStartOk({ type: "start_ok", stream_id: "t1-a", stream_no: 1, track: "tab" });
    expect(link.frames.map((f) => f.seq)).toEqual([0, 1, 2, 3, 4, 5, 6]);
  });

  it("keeps at most 30 seconds and counts what it drops", () => {
    const { link, s } = make();
    accept(s);
    link.isReady = false;
    s.onDisconnected();
    for (let i = 0; i < MAX_PENDING_FRAMES + 100; i++) s.onFrame(pcm(), i);
    expect(s.pendingFrames).toBe(MAX_PENDING_FRAMES);
    expect(s.framesDropped).toBe(100);
    link.isReady = true;
    link.texts.length = 0;
    s.onConnected();
    expect((link.texts[0] as { next_seq: number }).next_seq).toBe(100);
  });

  it("drains a backlog in batches, not in one burst", () => {
    const { link, s } = make();
    accept(s);
    link.isReady = false;
    s.onDisconnected();
    for (let i = 0; i < 200; i++) s.onFrame(pcm(), i);
    link.isReady = true;
    s.onConnected();
    s.onStartOk({ type: "start_ok", stream_id: "t1-a", stream_no: 1, track: "tab" });
    expect(link.frames.length).toBe(25);
    vi.advanceTimersByTime(5 * 20);
    expect(link.frames.length).toBeGreaterThan(25);
    vi.advanceTimersByTime(1000);
    expect(link.frames.map((f) => f.seq)).toEqual(Array.from({ length: 200 }, (_, i) => i));
  });

  it("offers the stream again when the application forgot it", () => {
    const { link, s } = make();
    accept(s);
    s.onFrame(pcm(), 1);
    link.isReady = false;
    s.onDisconnected();
    s.onFrame(pcm(), 2);
    link.isReady = true;
    link.texts.length = 0;
    s.onConnected();
    s.onStartRefused({ type: "start_refused", stream_id: "t1-a", reason: "unknown_stream" });
    expect(link.types()).toEqual(["resume", "start"]);
    expect(s.status).toBe("starting");
    s.onStartOk({ type: "start_ok", stream_id: "t1-a", stream_no: 8, track: "tab" });
    expect(link.frames.at(-1)).toEqual({ streamNo: 8, seq: 1 });
  });
});

describe("recording state and stop", () => {
  it("a finished recording ends an accepted stream", () => {
    const { s } = make();
    accept(s);
    expect(s.onAppState({ type: "state", recording: false, streams: [] })).toBe(true);
  });

  it("a finished recording leaves a waiting stream alone", () => {
    const { s } = make();
    s.onConnected();
    s.onStartRefused({ type: "start_refused", stream_id: "t1-a", reason: "not_recording" });
    expect(s.onAppState({ type: "state", recording: false, streams: [] })).toBe(false);
  });

  it("stop sends the counters", () => {
    const { link, s } = make();
    accept(s);
    s.onFrame(pcm(), 1);
    s.onFrame(new Int16Array(100), 2);
    s.stop("user_stop");
    expect(link.texts.at(-1)).toEqual({
      type: "stop",
      stream_id: "t1-a",
      reason: "user_stop",
      frames_sent: 2,
      samples_sent: 420,
      frames_dropped: 0,
    });
    expect(s.status).toBe("ended");
    s.onFrame(pcm(), 3);
    expect(link.frames).toHaveLength(2);
  });

  it("stop of a stream that was never accepted sends nothing", () => {
    const { link, s } = make();
    s.onConnected();
    s.onStartRefused({ type: "start_refused", stream_id: "t1-a", reason: "not_recording" });
    link.texts.length = 0;
    s.stop("user_stop");
    expect(link.texts).toHaveLength(0);
  });
});
