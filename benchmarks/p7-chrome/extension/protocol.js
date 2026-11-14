// Wire protocol shared by the offscreen document and the self-test page.
//
// One WebSocket carries many streams (one per captured tab).
//   Text frames  : JSON control messages (hello, start, stop).
//   Binary frames: 16-byte little-endian header + PCM payload (signed 16-bit, mono).
//       u32 tab_id | u32 seq | f64 sent_ms (epoch ms, sender clock) | s16le samples...

export const PROTOCOL_VERSION = 1;
export const SAMPLE_RATE = 16000;
export const FRAME_SAMPLES = 320; // 20 ms at 16 kHz
export const HEADER_BYTES = 16;

/** Builds one binary frame: header followed by the PCM bytes in `pcm` (an Int16Array). */
export function packFrame(tabId, seq, sentMs, pcm) {
  const out = new ArrayBuffer(HEADER_BYTES + pcm.byteLength);
  const view = new DataView(out);
  view.setUint32(0, tabId >>> 0, true);
  view.setUint32(4, seq >>> 0, true);
  view.setFloat64(8, sentMs, true);
  new Int16Array(out, HEADER_BYTES).set(pcm);
  return out;
}

export function helloMessage({ token, session }) {
  const msg = {
    type: "hello",
    version: PROTOCOL_VERSION,
    client: "bbb-p7-extension",
    user_agent: navigator.userAgent,
  };
  if (session) msg.session = session;
  else msg.token = token;
  return JSON.stringify(msg);
}

export function startMessage(tabId, title) {
  return JSON.stringify({
    type: "start",
    tab_id: tabId,
    sample_rate: SAMPLE_RATE,
    channels: 1,
    format: "s16le",
    frame_samples: FRAME_SAMPLES,
    title: title || "",
  });
}

export function stopMessage(tabId, reason, stats) {
  return JSON.stringify({
    type: "stop",
    tab_id: tabId,
    reason,
    frames_sent: stats.framesSent,
    samples_sent: stats.samplesSent,
    frames_dropped: stats.framesDropped,
  });
}
