// WebSocket client of the bridge (127.0.0.1 only) with reconnect.
//
// State machine:
//   idle -> connecting -> ready            hello_ok received
//   connecting -> connecting               next port after a refused or silent connection
//   connecting -> backoff -> connecting    every port failed: wait 0.5 s, 1 s, 2 s, 4 s, 5 s, 5 s ...
//   ready -> backoff                       connection lost or silent for three heartbeats
//   any -> stopped                         unpaired / bad_version / user disconnect (no retries)
//
// The socket factory and the timers are the globals, so tests drive the machine with fake
// timers and a fake socket. The client knows nothing about streams: the owner of the streams
// listens to `onReady` and `onState` and resends what it needs (resume, buffered frames).

import {
  DEFAULT_PORTS,
  helloMessage,
  parseServerMessage,
  type Credentials,
  type ErrorCode,
  type HelloOk,
  type ServerMessage,
} from "./protocol.js";

export const BACKOFF_START_MS = 500;
export const BACKOFF_MAX_MS = 5000;
export const CONNECT_TIMEOUT_MS = 2000;

export type ClientState = "idle" | "connecting" | "ready" | "backoff" | "stopped";

export interface SocketLike {
  binaryType: string;
  readyState: number;
  bufferedAmount: number;
  onopen: (() => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
  onclose: ((ev: { code: number }) => void) | null;
  onerror: (() => void) | null;
  send(data: string | ArrayBuffer): void;
  close(code?: number): void;
}

export interface BridgeClientOptions {
  createSocket: (url: string) => SocketLike;
  extVersion: () => string;
  clientName: string;
  ports?: number[];
  onState?: (state: ClientState, detail?: string) => void;
  /** Called after every hello_ok (first connection and each reconnect). */
  onReady?: (ok: HelloOk, reconnect: boolean) => void;
  /** Called for every other message from the application. */
  onMessage?: (msg: ServerMessage) => void;
  /** Called when the application revoked the pairing or refused the protocol version. */
  onFatal?: (code: ErrorCode, message: string) => void;
  /** Called once with the credentials received at pairing. */
  onPaired?: (credentials: Credentials) => void;
}

const OPEN = 1;

export class BridgeError extends Error {
  constructor(
    readonly code: ErrorCode | "unreachable" | "timeout",
    message: string,
  ) {
    super(message);
  }
}

type Auth = { pairingCode: string } | Credentials;

export class BridgeClient {
  state: ClientState = "idle";
  port: number | null = null;
  heartbeatMs = 5000;

  private readonly ports: number[];
  private credentials: Credentials | null = null;
  private pairingCode: string | null = null;
  private pairing: { resolve: (ok: HelloOk) => void; reject: (e: BridgeError) => void } | null =
    null;
  private socket: SocketLike | null = null;
  private portIndex = 0;
  private preferredPort: number | null = null;
  private delay = BACKOFF_START_MS;
  private connectTimer: unknown = null;
  private retryTimer: unknown = null;
  private heartbeatTimer: unknown = null;
  private lastHeard = 0;
  private hadReady = false;

  constructor(private readonly opts: BridgeClientOptions) {
    this.ports = opts.ports ?? DEFAULT_PORTS;
  }

  get isReady(): boolean {
    return this.state === "ready" && this.socket !== null && this.socket.readyState === OPEN;
  }

  get bufferedAmount(): number {
    return this.socket ? this.socket.bufferedAmount : 0;
  }

  /** Keep a connection with stored credentials, reconnecting until stopped. */
  connect(credentials: Credentials, preferredPort?: number): void {
    this.reset();
    this.credentials = credentials;
    this.preferredPort = preferredPort ?? null;
    this.beginRound();
  }

  /**
   * Pair with a code shown by the application. Resolves when the application accepted it;
   * from then on the client stays connected with the received credentials. Rejects with
   * `bad_code`, `code_expired`, `busy` or `unreachable` (no application on any port).
   */
  pair(code: string): Promise<HelloOk> {
    this.reset();
    this.pairingCode = code;
    return new Promise((resolve, reject) => {
      this.pairing = { resolve, reject };
      this.beginRound();
    });
  }

  /** Close the connection and stop retrying. */
  disconnect(): void {
    this.reset();
    this.setState("stopped", "disconnected");
  }

  sendText(msg: unknown): boolean {
    if (!this.isReady || !this.socket) return false;
    this.socket.send(JSON.stringify(msg));
    return true;
  }

  sendBinary(buf: ArrayBuffer): boolean {
    if (!this.isReady || !this.socket) return false;
    this.socket.send(buf);
    return true;
  }

  // --- internals --------------------------------------------------------------------

  private reset(): void {
    this.clearTimers();
    this.dropSocket();
    if (this.pairing) {
      const p = this.pairing;
      this.pairing = null;
      p.reject(new BridgeError("unreachable", "pairing cancelled"));
    }
    this.credentials = null;
    this.pairingCode = null;
    this.delay = BACKOFF_START_MS;
    this.hadReady = false;
  }

  private clearTimers(): void {
    for (const t of [this.connectTimer, this.retryTimer]) {
      if (t !== null) clearTimeout(t as ReturnType<typeof setTimeout>);
    }
    if (this.heartbeatTimer !== null) {
      clearInterval(this.heartbeatTimer as ReturnType<typeof setInterval>);
    }
    this.connectTimer = this.retryTimer = this.heartbeatTimer = null;
  }

  private dropSocket(): void {
    const s = this.socket;
    this.socket = null;
    if (!s) return;
    s.onopen = s.onmessage = s.onclose = s.onerror = null;
    try {
      s.close(1000);
    } catch {
      // already closed
    }
  }

  private setState(state: ClientState, detail?: string): void {
    if (this.state === state && detail === undefined) return;
    this.state = state;
    this.opts.onState?.(state, detail);
  }

  private portOrder(): number[] {
    const first = this.preferredPort ?? this.port;
    return first !== null && this.ports.includes(first)
      ? [first, ...this.ports.filter((p) => p !== first)]
      : this.ports;
  }

  private beginRound(): void {
    this.portIndex = 0;
    this.attempt();
  }

  private auth(): Auth | null {
    if (this.credentials) return this.credentials;
    return this.pairingCode ? { pairingCode: this.pairingCode } : null;
  }

  private attempt(): void {
    const auth = this.auth();
    if (!auth) return;
    const order = this.portOrder();
    const port = order[this.portIndex];
    this.setState("connecting", `port ${port}`);
    let socket: SocketLike;
    try {
      socket = this.opts.createSocket(`ws://127.0.0.1:${port}/`);
    } catch {
      this.failAttempt();
      return;
    }
    socket.binaryType = "arraybuffer";
    this.socket = socket;
    this.connectTimer = setTimeout(() => this.failAttempt(), CONNECT_TIMEOUT_MS);
    socket.onopen = () => {
      socket.send(JSON.stringify(helloMessage(this.opts.extVersion(), this.opts.clientName, auth)));
    };
    socket.onerror = () => {
      // onclose follows; nothing to do here.
    };
    socket.onclose = () => {
      if (this.socket !== socket) return;
      this.socket = null;
      this.onLost();
    };
    socket.onmessage = (ev) => {
      if (this.socket !== socket || typeof ev.data !== "string") return;
      this.lastHeard = Date.now();
      const msg = parseServerMessage(ev.data);
      if (msg) this.handle(msg, port);
    };
  }

  private handle(msg: ServerMessage, port: number): void {
    if (msg.type === "hello_ok") {
      this.onHelloOk(msg, port);
    } else if (msg.type === "error") {
      this.onError(msg.code, msg.message);
    } else if (msg.type === "ping") {
      this.sendText({ type: "pong", t: msg.t });
    } else if (msg.type !== "pong") {
      this.opts.onMessage?.(msg);
    }
  }

  private onHelloOk(ok: HelloOk, port: number): void {
    if (this.connectTimer !== null)
      clearTimeout(this.connectTimer as ReturnType<typeof setTimeout>);
    this.connectTimer = null;
    this.port = port;
    this.preferredPort = port;
    this.delay = BACKOFF_START_MS;
    this.heartbeatMs = ok.heartbeat_ms > 0 ? ok.heartbeat_ms : 5000;
    if (ok.secret && this.pairingCode) {
      this.credentials = { client_id: ok.client_id, secret: ok.secret };
      this.pairingCode = null;
      this.opts.onPaired?.(this.credentials);
    }
    const reconnect = this.hadReady;
    this.hadReady = true;
    this.lastHeard = Date.now();
    this.setState("ready", `port ${port}`);
    this.heartbeatTimer = setInterval(() => this.beat(), this.heartbeatMs);
    const p = this.pairing;
    this.pairing = null;
    p?.resolve(ok);
    this.opts.onReady?.(ok, reconnect);
  }

  private beat(): void {
    if (Date.now() - this.lastHeard > this.heartbeatMs * 3) {
      this.onLostSocket();
      return;
    }
    this.sendText({ type: "ping", t: Date.now() });
  }

  private onLostSocket(): void {
    this.dropSocket();
    this.onLost();
  }

  private onError(code: ErrorCode, message: string): void {
    const fatal = code === "unpaired" || code === "bad_version" || code === "bad_origin";
    if (this.pairing) {
      const p = this.pairing;
      this.pairing = null;
      this.clearTimers();
      this.dropSocket();
      this.pairingCode = null;
      this.setState("stopped", code);
      p.reject(new BridgeError(code, message));
      return;
    }
    if (fatal) {
      this.clearTimers();
      this.dropSocket();
      this.credentials = null;
      this.setState("stopped", code);
      this.opts.onFatal?.(code, message);
    }
    // busy and other errors: the application closes the socket, the loss path retries.
  }

  private onLost(): void {
    if (this.state === "stopped" || this.state === "idle") return;
    // A lost connection tries the port that worked first again.
    if (this.state === "ready") this.portIndex = -1;
    this.clearTimers();
    this.dropSocket();
    this.failAttempt();
  }

  /** The current port did not work out: try the next one, or wait and start a new round. */
  private failAttempt(): void {
    if (this.connectTimer !== null)
      clearTimeout(this.connectTimer as ReturnType<typeof setTimeout>);
    this.connectTimer = null;
    this.dropSocket();
    if (this.heartbeatTimer !== null) {
      clearInterval(this.heartbeatTimer as ReturnType<typeof setInterval>);
      this.heartbeatTimer = null;
    }
    if (this.state === "stopped") return;
    this.portIndex += 1;
    if (this.portIndex < this.portOrder().length) {
      this.attempt();
      return;
    }
    if (this.pairing) {
      // Pairing tries every port once, then gives up: the user is waiting.
      const p = this.pairing;
      this.pairing = null;
      this.pairingCode = null;
      this.setState("stopped", "unreachable");
      p.reject(new BridgeError("unreachable", "The application does not answer on any port."));
      return;
    }
    const wait = this.delay;
    this.delay = Math.min(this.delay * 2, BACKOFF_MAX_MS);
    this.setState("backoff", `${wait} ms`);
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      this.beginRound();
    }, wait);
  }
}
