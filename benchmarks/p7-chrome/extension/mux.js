// Multiplexes any number of captured MediaStreams over one Bridge, inside one document.
//
// Two AudioContexts are shared by all streams:
//   playback (default rate) : source -> destination, so the tab stays audible;
//   capture  (16 kHz)       : source -> worklet -> (muted gain) -> destination,
//                             the browser resamples to 16 kHz before the worklet sees it.

import { FRAME_SAMPLES, SAMPLE_RATE } from "./protocol.js";

export class StreamMux {
  constructor(bridge, { workletUrl, onStopped }) {
    this.bridge = bridge;
    this.workletUrl = workletUrl;
    this.onStopped = onStopped || (() => {});
    this.streams = new Map(); // tabId -> stream record
    this.playbackCtx = null;
    this.captureCtx = null;
    this.ready = null;
  }

  async init() {
    if (!this.ready) {
      this.ready = (async () => {
        this.playbackCtx = new AudioContext({ latencyHint: "playback" });
        this.captureCtx = new AudioContext({ sampleRate: SAMPLE_RATE, latencyHint: "interactive" });
        await this.captureCtx.audioWorklet.addModule(this.workletUrl);
        await Promise.all([this.playbackCtx.resume(), this.captureCtx.resume()]);
      })();
    }
    return this.ready;
  }

  has(tabId) {
    return this.streams.has(tabId);
  }

  /** Adds a stream; `audible` replays it to the default output (false for synthetic tests). */
  async add(tabId, mediaStream, { title = "", audible = true } = {}) {
    if (this.streams.has(tabId)) throw new Error(`tab ${tabId} is already streaming`);
    await this.init();

    const rec = {
      tabId,
      mediaStream,
      seq: 0,
      framesSent: 0,
      samplesSent: 0,
      framesDropped: 0,
      stopping: false,
      nodes: [],
    };
    const track = mediaStream.getAudioTracks()[0];
    if (!track) throw new Error("stream has no audio track");

    if (audible) {
      const play = this.playbackCtx.createMediaStreamSource(mediaStream);
      play.connect(this.playbackCtx.destination);
      rec.nodes.push(play);
    }
    const src = this.captureCtx.createMediaStreamSource(mediaStream);
    const worklet = new AudioWorkletNode(this.captureCtx, "pcm-framer", {
      numberOfInputs: 1,
      numberOfOutputs: 1,
      outputChannelCount: [1],
      processorOptions: { frameSamples: FRAME_SAMPLES },
    });
    const mute = this.captureCtx.createGain();
    mute.gain.value = 0;
    src.connect(worklet);
    worklet.connect(mute);
    mute.connect(this.captureCtx.destination);
    rec.nodes.push(src, worklet, mute);
    rec.worklet = worklet;

    worklet.port.onmessage = (ev) => {
      const data = ev.data;
      if (data.type === "pcm") {
        const pcm = new Int16Array(data.pcm);
        if (this.bridge.sendFrame(tabId, rec.seq, pcm)) {
          rec.framesSent += 1;
          rec.samplesSent += pcm.length;
        } else {
          rec.framesDropped += 1;
        }
        rec.seq += 1; // sequence advances for dropped frames too, so the receiver sees the hole
      } else if (data.type === "flushed" && rec.onFlushed) {
        rec.onFlushed();
      }
    };
    track.addEventListener("ended", () => this.stop(tabId, "track_ended"));

    this.streams.set(tabId, rec);
    this.bridge.sendStart(tabId, title);
    return rec;
  }

  /** Stops one stream cleanly: flush the partial frame, send `stop`, release the tab. */
  async stop(tabId, reason = "user_stop") {
    const rec = this.streams.get(tabId);
    if (!rec || rec.stopping) return;
    rec.stopping = true;

    await new Promise((resolve) => {
      rec.onFlushed = resolve;
      rec.worklet.port.postMessage({ type: "flush" });
      setTimeout(resolve, 200);
    });
    for (const node of rec.nodes) node.disconnect();
    for (const track of rec.mediaStream.getTracks()) track.stop();
    this.bridge.sendStop(tabId, reason, rec);
    this.streams.delete(tabId);
    this.onStopped(tabId, reason);
  }

  async stopAll(reason = "user_stop") {
    await Promise.all([...this.streams.keys()].map((id) => this.stop(id, reason)));
  }

  status() {
    return [...this.streams.values()].map((r) => ({
      tabId: r.tabId,
      framesSent: r.framesSent,
      framesDropped: r.framesDropped,
    }));
  }
}
