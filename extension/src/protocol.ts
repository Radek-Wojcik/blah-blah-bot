// Bridge protocol version 2 (ADR 0020): message types, builders and the binary frame layout.
//
// Text frames are JSON with a `type`. Binary frames carry audio: a 16-byte little-endian
// header `u32 stream_no | u32 seq | f64 sent_ms`, then signed 16-bit little-endian mono
// samples at 16 kHz. The shared fixtures in crates/bridge/tests/fixtures pin both.

export const PROTOCOL_VERSION = 2;
export const SAMPLE_RATE = 16000;
export const FRAME_SAMPLES = 320; // 20 ms at 16 kHz
export const FRAME_MS = 20;
export const HEADER_BYTES = 16;
/** Ports of the application, in the order they are tried (ADR 0021). */
export const DEFAULT_PORTS = [17555, 17556, 17557, 17558, 17559, 17560, 17561, 17562, 17563, 17564];

export type StopReason = "user_stop" | "track_ended" | "tab_closed" | "connection_lost";
export type RefuseReason = "not_recording" | "duplicate" | "declined" | "unknown_stream";
export type ErrorCode =
  "bad_version" | "bad_code" | "code_expired" | "unpaired" | "bad_origin" | "busy" | "bad_message";
export type GroupTabStatus = "captured" | "waiting" | "failed";

export interface Hello {
  type: "hello";
  version: typeof PROTOCOL_VERSION;
  client: string;
  ext_version: string;
  pairing_code?: string;
  client_id?: string;
  secret?: string;
}
export interface Start {
  type: "start";
  stream_id: string;
  tab_id: number;
  group_id?: number;
  title: string;
  host: string;
  sample_rate: 16000;
  channels: 1;
  format: "s16le";
  frame_samples: number;
}
export interface Resume {
  type: "resume";
  stream_id: string;
  next_seq: number;
}
export interface StopStats {
  framesSent: number;
  samplesSent: number;
  framesDropped: number;
}
export interface Stop {
  type: "stop";
  stream_id: string;
  reason: StopReason;
  frames_sent: number;
  samples_sent: number;
  frames_dropped: number;
}
export interface Meet {
  type: "meet";
  tab_id: number;
  host: string;
  state: "joined" | "left";
}
export interface Group {
  type: "group";
  group_id: number;
  tabs: { tab_id: number; status: GroupTabStatus }[];
}
export interface Ping {
  type: "ping";
  t?: number;
}

export type ClientMessage =
  Hello | Start | Resume | Stop | Meet | Group | Ping | { type: "pong"; t?: number };

export interface HelloOk {
  type: "hello_ok";
  client_id: string;
  secret?: string;
  session_id: string;
  app_version: string;
  recording: boolean;
  heartbeat_ms: number;
}
export interface ErrorMessage {
  type: "error";
  code: ErrorCode;
  message: string;
}
export interface StartOk {
  type: "start_ok";
  stream_id: string;
  stream_no: number;
  track: string;
}
export interface StartRefused {
  type: "start_refused";
  stream_id: string;
  reason: RefuseReason;
}
export interface AppState {
  type: "state";
  recording: boolean;
  meeting_id?: string;
  streams: { stream_id: string; track: string; status: string }[];
}

export type ServerMessage =
  | HelloOk
  | ErrorMessage
  | StartOk
  | StartRefused
  | AppState
  | { type: "ping"; t?: number }
  | { type: "pong"; t?: number };

export interface Credentials {
  client_id: string;
  secret: string;
}

export function helloMessage(
  extVersion: string,
  client: string,
  auth: { pairingCode: string } | Credentials,
): Hello {
  const base = {
    type: "hello",
    version: PROTOCOL_VERSION,
    client,
    ext_version: extVersion,
  } as const;
  return "pairingCode" in auth
    ? { ...base, pairing_code: auth.pairingCode }
    : { ...base, client_id: auth.client_id, secret: auth.secret };
}

export function startMessage(s: {
  streamId: string;
  tabId: number;
  groupId?: number;
  title: string;
  host: string;
}): Start {
  const msg: Start = {
    type: "start",
    stream_id: s.streamId,
    tab_id: s.tabId,
    title: s.title,
    host: s.host,
    sample_rate: SAMPLE_RATE,
    channels: 1,
    format: "s16le",
    frame_samples: FRAME_SAMPLES,
  };
  if (s.groupId !== undefined) msg.group_id = s.groupId;
  return msg;
}

export function stopMessage(streamId: string, reason: StopReason, stats: StopStats): Stop {
  return {
    type: "stop",
    stream_id: streamId,
    reason,
    frames_sent: stats.framesSent,
    samples_sent: stats.samplesSent,
    frames_dropped: stats.framesDropped,
  };
}

/** Builds one binary frame: header followed by the samples (little-endian). */
export function packFrame(
  streamNo: number,
  seq: number,
  sentMs: number,
  pcm: Int16Array,
): ArrayBuffer {
  const out = new ArrayBuffer(HEADER_BYTES + pcm.length * 2);
  const view = new DataView(out);
  view.setUint32(0, streamNo >>> 0, true);
  view.setUint32(4, seq >>> 0, true);
  view.setFloat64(8, sentMs, true);
  // Explicit little-endian writes keep the layout independent of the host byte order.
  for (let i = 0; i < pcm.length; i++) view.setInt16(HEADER_BYTES + i * 2, pcm[i], true);
  return out;
}

export interface ParsedFrame {
  streamNo: number;
  seq: number;
  sentMs: number;
  samples: Int16Array;
}

/** Parses a binary frame; returns null for a frame the receiver would refuse. */
export function parseFrame(buf: ArrayBuffer): ParsedFrame | null {
  if (buf.byteLength <= HEADER_BYTES || (buf.byteLength - HEADER_BYTES) % 2 !== 0) return null;
  const view = new DataView(buf);
  const sentMs = view.getFloat64(8, true);
  if (!Number.isFinite(sentMs)) return null;
  const n = (buf.byteLength - HEADER_BYTES) / 2;
  const samples = new Int16Array(n);
  for (let i = 0; i < n; i++) samples[i] = view.getInt16(HEADER_BYTES + i * 2, true);
  return { streamNo: view.getUint32(0, true), seq: view.getUint32(4, true), sentMs, samples };
}

const SERVER_TYPES = ["hello_ok", "error", "start_ok", "start_refused", "state", "ping", "pong"];

/** Parses a text frame from the application; null when it is not a known message. */
export function parseServerMessage(text: string): ServerMessage | null {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  if (typeof value !== "object" || value === null) return null;
  const type = (value as { type?: unknown }).type;
  return typeof type === "string" && SERVER_TYPES.includes(type) ? (value as ServerMessage) : null;
}
