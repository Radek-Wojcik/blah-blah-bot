// The wire format against the fixtures shared with the Rust tests
// (crates/bridge/tests/fixtures): the same bytes and messages must mean the same thing on
// both sides.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  DEFAULT_PORTS,
  HEADER_BYTES,
  helloMessage,
  packFrame,
  parseFrame,
  parseServerMessage,
  startMessage,
  stopMessage,
} from "../src/protocol.js";

const dir = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../crates/bridge/tests/fixtures",
);
const messages = JSON.parse(fs.readFileSync(path.join(dir, "messages.json"), "utf8"));
const frames = JSON.parse(fs.readFileSync(path.join(dir, "frames.json"), "utf8"));

const byName = (list: { name: string; message: unknown }[], name: string) =>
  list.find((m) => m.name === name)?.message;

const fromHex = (hex: string): ArrayBuffer => {
  const bytes = new Uint8Array(hex.match(/../g)?.map((h) => parseInt(h, 16)) ?? []);
  return bytes.buffer;
};
const toHex = (buf: ArrayBuffer): string =>
  [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, "0")).join("");

describe("binary frames", () => {
  for (const f of frames.valid) {
    it(`packs and parses: ${f.name}`, () => {
      const packed = packFrame(f.stream_no, f.seq, f.sent_ms, Int16Array.from(f.samples));
      expect(toHex(packed)).toBe(f.hex);
      const parsed = parseFrame(fromHex(f.hex));
      expect(parsed).not.toBeNull();
      expect(parsed?.streamNo).toBe(f.stream_no);
      expect(parsed?.seq).toBe(f.seq);
      expect(parsed?.sentMs).toBe(f.sent_ms);
      expect([...(parsed?.samples ?? [])]).toEqual(f.samples);
    });
  }
  for (const f of frames.invalid) {
    it(`refuses: ${f.name}`, () => {
      expect(parseFrame(fromHex(f.hex))).toBeNull();
    });
  }
  it("a 20 ms frame is 16 + 640 bytes", () => {
    expect(packFrame(1, 0, 0, new Int16Array(320)).byteLength).toBe(HEADER_BYTES + 640);
  });
});

describe("text messages", () => {
  it("hello with a pairing code matches the fixture", () => {
    expect(helloMessage("0.6.0", "Chrome 136", { pairingCode: "7K3M9QXD" })).toEqual(
      byName(messages.client, "hello with a pairing code"),
    );
  });

  it("hello with the secret matches the fixture", () => {
    const fx = byName(messages.client, "hello with the client secret") as {
      client_id: string;
      secret: string;
    };
    expect(
      helloMessage("0.6.0", "Chrome 136", { client_id: fx.client_id, secret: fx.secret }),
    ).toEqual(fx);
  });

  it("start with a group matches the fixture", () => {
    expect(
      startMessage({
        streamId: "t812-1",
        tabId: 812,
        groupId: 4,
        title: "Weekly sync - Google Meet",
        host: "meet.google.com",
      }),
    ).toEqual(byName(messages.client, "start of a tab in a group"));
  });

  it("start without a group has no group_id", () => {
    const msg = startMessage({
      streamId: "t95-2",
      tabId: 95,
      title: "Design review recording",
      host: "video.example.org",
    });
    expect(msg).toEqual(byName(messages.client, "start of a tab without a group"));
    expect("group_id" in msg).toBe(false);
  });

  it("stop matches the fixture", () => {
    expect(
      stopMessage("t812-1", "tab_closed", {
        framesSent: 1600,
        samplesSent: 512000,
        framesDropped: 0,
      }),
    ).toEqual(byName(messages.client, "stop when the tab closes"));
  });

  it("understands every message the application sends", () => {
    for (const m of messages.server) {
      const parsed = parseServerMessage(JSON.stringify(m.message));
      expect(parsed, m.name).not.toBeNull();
      expect(parsed?.type, m.name).toBe(m.message.type);
    }
  });

  it("ignores what is not a message of the application", () => {
    for (const m of messages.invalid) {
      expect(parseServerMessage(m.text), m.name).toBeNull();
    }
  });

  it("every client fixture has a type the schema knows", () => {
    const types = ["hello", "start", "resume", "stop", "meet", "group", "ping", "pong"];
    for (const m of messages.client) expect(types).toContain(m.message.type);
  });
});

describe("ports", () => {
  it("are 17555 and the fallbacks 17556 to 17564", () => {
    expect(DEFAULT_PORTS[0]).toBe(17555);
    expect(DEFAULT_PORTS).toHaveLength(10);
    expect(DEFAULT_PORTS[9]).toBe(17564);
  });
});
