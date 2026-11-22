// The reconnect state machine of the bridge client with fake timers and a fake socket.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  BACKOFF_MAX_MS,
  BACKOFF_START_MS,
  BridgeClient,
  BridgeError,
  CONNECT_TIMEOUT_MS,
  type ClientState,
  type SocketLike,
} from "../src/bridge-client.js";
import type { HelloOk } from "../src/protocol.js";

class FakeSocket implements SocketLike {
  static all: FakeSocket[] = [];
  binaryType = "blob";
  readyState = 0;
  bufferedAmount = 0;
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: { code: number }) => void) | null = null;
  onerror: (() => void) | null = null;
  sent: string[] = [];
  closed = false;
  constructor(readonly url: string) {
    FakeSocket.all.push(this);
  }
  get port(): number {
    return Number(/:(\d+)\//.exec(this.url)?.[1]);
  }
  send(data: string | ArrayBuffer): void {
    if (typeof data === "string") this.sent.push(data);
  }
  close(): void {
    this.closed = true;
    this.readyState = 3;
  }
  // test controls
  open(): void {
    this.readyState = 1;
    this.onopen?.();
  }
  fail(): void {
    this.readyState = 3;
    this.onerror?.();
    this.onclose?.({ code: 1006 });
  }
  receive(msg: object): void {
    this.onmessage?.({ data: JSON.stringify(msg) });
  }
}

const PORTS = [100, 101, 102];
const creds = { client_id: "c-1", secret: "s" };
const helloOk = (over: Partial<HelloOk> = {}): HelloOk => ({
  type: "hello_ok",
  client_id: "c-1",
  session_id: "s-1",
  app_version: "0.6.0",
  recording: false,
  heartbeat_ms: 5000,
  ...over,
});

function make() {
  const states: ClientState[] = [];
  const ready = vi.fn();
  const fatal = vi.fn();
  const paired = vi.fn();
  const messages: object[] = [];
  const client = new BridgeClient({
    createSocket: (url) => new FakeSocket(url),
    extVersion: () => "0.6.0",
    clientName: "Chrome 136",
    ports: PORTS,
    onState: (s) => states.push(s),
    onReady: ready,
    onFatal: fatal,
    onPaired: paired,
    onMessage: (m) => messages.push(m),
  });
  return { client, states, ready, fatal, paired, messages };
}

/** Every socket still waiting for an answer fails, as a refused connection does. */
function refuseAll(): void {
  for (const s of FakeSocket.all.filter((x) => !x.closed && x.readyState === 0)) s.fail();
}

beforeEach(() => {
  FakeSocket.all = [];
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-01-01T00:00:00Z"));
});
afterEach(() => {
  vi.useRealTimers();
});

describe("connecting", () => {
  it("says hello with the credentials and is ready after hello_ok", () => {
    const { client, ready } = make();
    client.connect(creds);
    const s = FakeSocket.all[0];
    expect(s.port).toBe(100);
    s.open();
    expect(JSON.parse(s.sent[0])).toMatchObject({
      type: "hello",
      version: 2,
      client_id: "c-1",
      secret: "s",
      ext_version: "0.6.0",
    });
    s.receive(helloOk());
    expect(client.state).toBe("ready");
    expect(client.isReady).toBe(true);
    expect(ready).toHaveBeenCalledWith(expect.objectContaining({ type: "hello_ok" }), false);
  });

  it("tries the next port when one is refused, then waits before the next round", () => {
    const { client } = make();
    client.connect(creds);
    for (let i = 0; i < PORTS.length; i++) {
      expect(FakeSocket.all).toHaveLength(i + 1);
      FakeSocket.all[i].fail();
    }
    expect(client.state).toBe("backoff");
    expect(FakeSocket.all.map((s) => s.port)).toEqual([100, 101, 102]);
    vi.advanceTimersByTime(BACKOFF_START_MS - 1);
    expect(FakeSocket.all).toHaveLength(3);
    vi.advanceTimersByTime(1);
    expect(FakeSocket.all).toHaveLength(4);
    expect(FakeSocket.all[3].port).toBe(100);
  });

  it("backs off 0.5, 1, 2, 4, 5, 5 seconds", () => {
    const { client } = make();
    client.connect(creds);
    const waits: number[] = [];
    for (let round = 0; round < 6; round++) {
      refuseAll();
      while (FakeSocket.all.some((s) => !s.closed && s.readyState === 0)) refuseAll();
      const before = FakeSocket.all.length;
      let waited = 0;
      while (FakeSocket.all.length === before && waited < 10_000) {
        vi.advanceTimersByTime(100);
        waited += 100;
      }
      waits.push(waited);
    }
    expect(waits).toEqual([500, 1000, 2000, 4000, BACKOFF_MAX_MS, BACKOFF_MAX_MS]);
    expect(client.state).not.toBe("stopped");
  });

  it("gives up on a port that stays silent after the connect timeout", () => {
    const { client } = make();
    client.connect(creds);
    FakeSocket.all[0].open(); // opens but never answers hello
    vi.advanceTimersByTime(CONNECT_TIMEOUT_MS);
    expect(FakeSocket.all).toHaveLength(2);
    expect(FakeSocket.all[0].closed).toBe(true);
    expect(FakeSocket.all[1].port).toBe(101);
  });
});

describe("after a lost connection", () => {
  function readyClient() {
    const t = make();
    t.client.connect(creds);
    const s = FakeSocket.all[0];
    s.open();
    s.receive(helloOk());
    return { ...t, socket: s };
  }

  it("reconnects at once to the port that worked and reports a reconnect", () => {
    const { client, ready, socket } = readyClient();
    socket.fail();
    expect(FakeSocket.all).toHaveLength(2);
    expect(FakeSocket.all[1].port).toBe(100);
    FakeSocket.all[1].open();
    FakeSocket.all[1].receive(helloOk({ recording: true }));
    expect(client.state).toBe("ready");
    expect(ready).toHaveBeenLastCalledWith(expect.objectContaining({ recording: true }), true);
  });

  it("resets the backoff after a successful reconnect", () => {
    const { client, socket } = readyClient();
    socket.fail();
    refuseAll(); // port 100 again
    refuseAll();
    refuseAll();
    expect(client.state).toBe("backoff");
    vi.advanceTimersByTime(BACKOFF_START_MS);
    const s = FakeSocket.all[FakeSocket.all.length - 1];
    s.open();
    s.receive(helloOk());
    // next loss: first wait is 500 ms again
    s.fail();
    refuseAll();
    refuseAll();
    refuseAll();
    const before = FakeSocket.all.length;
    vi.advanceTimersByTime(BACKOFF_START_MS);
    expect(FakeSocket.all.length).toBe(before + 1);
  });

  it("sends a ping every heartbeat and answers pings", () => {
    const { socket } = readyClient();
    socket.sent.length = 0;
    socket.readyState = 1;
    vi.advanceTimersByTime(5000);
    expect(socket.sent.map((t) => JSON.parse(t).type)).toEqual(["ping"]);
    socket.receive({ type: "ping", t: 7 });
    expect(JSON.parse(socket.sent[1])).toEqual({ type: "pong", t: 7 });
  });

  it("reconnects when the application is silent for three heartbeats", () => {
    const { socket } = readyClient();
    socket.readyState = 1;
    vi.advanceTimersByTime(5000 * 3 + 5000);
    expect(socket.closed).toBe(true);
    expect(FakeSocket.all.length).toBeGreaterThan(1);
  });

  it("stays connected while the application answers", () => {
    const { client, socket } = readyClient();
    socket.readyState = 1;
    for (let i = 0; i < 6; i++) {
      vi.advanceTimersByTime(5000);
      socket.receive({ type: "pong" });
    }
    expect(client.state).toBe("ready");
    expect(FakeSocket.all).toHaveLength(1);
  });

  it("stops for good when the pairing was revoked", () => {
    const { client, fatal, socket } = readyClient();
    socket.receive({ type: "error", code: "unpaired", message: "gone" });
    expect(client.state).toBe("stopped");
    expect(fatal).toHaveBeenCalledWith("unpaired", "gone");
    vi.advanceTimersByTime(60_000);
    expect(FakeSocket.all).toHaveLength(1);
  });

  it("stops for good after disconnect()", () => {
    const { client } = readyClient();
    client.disconnect();
    vi.advanceTimersByTime(60_000);
    expect(client.state).toBe("stopped");
    expect(FakeSocket.all).toHaveLength(1);
  });

  it("retries after a busy answer", () => {
    const { client, socket } = readyClient();
    socket.fail();
    const s2 = FakeSocket.all[1];
    s2.open();
    s2.receive({ type: "error", code: "busy", message: "two connections" });
    s2.fail();
    expect(client.state).not.toBe("stopped");
    expect(FakeSocket.all.length).toBeGreaterThan(2);
  });
});

describe("pairing", () => {
  it("resolves with the credentials and keeps them for reconnects", async () => {
    const { client, paired, ready } = make();
    const result = client.pair("7K3M9QXD");
    const s = FakeSocket.all[0];
    s.open();
    expect(JSON.parse(s.sent[0])).toMatchObject({ type: "hello", pairing_code: "7K3M9QXD" });
    s.receive(helloOk({ secret: "abc" }));
    await expect(result).resolves.toMatchObject({ client_id: "c-1", secret: "abc" });
    expect(paired).toHaveBeenCalledWith({ client_id: "c-1", secret: "abc" });
    expect(ready).toHaveBeenCalledTimes(1);
    // a later loss reconnects with the secret, not the code
    s.fail();
    const s2 = FakeSocket.all[1];
    s2.open();
    expect(JSON.parse(s2.sent[0])).toMatchObject({ client_id: "c-1", secret: "abc" });
    expect(JSON.parse(s2.sent[0]).pairing_code).toBeUndefined();
  });

  it.each(["bad_code", "code_expired", "busy"] as const)("rejects with %s", async (code) => {
    const { client, states } = make();
    const result = client.pair("AAAAAAAA");
    const s = FakeSocket.all[0];
    s.open();
    s.receive({ type: "error", code, message: "no" });
    await expect(result).rejects.toMatchObject({ code });
    expect(states.at(-1)).toBe("stopped");
    vi.advanceTimersByTime(30_000);
    expect(FakeSocket.all).toHaveLength(1); // a wrong code is not retried
  });

  it("rejects as unreachable when no port answers", async () => {
    const { client } = make();
    const result = client.pair("AAAAAAAA");
    const assertion = expect(result).rejects.toBeInstanceOf(BridgeError);
    refuseAll();
    refuseAll();
    refuseAll();
    await assertion;
    await expect(result).rejects.toMatchObject({ code: "unreachable" });
  });
});

describe("messages", () => {
  it("passes other messages to the owner", () => {
    const { client, messages } = make();
    client.connect(creds);
    const s = FakeSocket.all[0];
    s.open();
    s.receive(helloOk());
    s.receive({ type: "start_ok", stream_id: "t1-1", stream_no: 1, track: "tab" });
    s.receive({ type: "teleport" });
    expect(messages).toEqual([{ type: "start_ok", stream_id: "t1-1", stream_no: 1, track: "tab" }]);
  });
});
