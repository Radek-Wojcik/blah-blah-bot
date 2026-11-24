// The bridge alone (crates/bridge/examples/test_receiver) as a child process. Only the process
// started here is ever killed.
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { REPO_ROOT } from "./tmp";

export interface ReceiverEvent {
  event: string;
  [key: string]: unknown;
}

export interface StreamEnd extends ReceiverEvent {
  stream_id: string;
  reason: string;
  stats: {
    frames: number;
    samples: number;
    lost_frames: number;
    late_frames: number;
    max_gap_ms: number;
    max_stall_ms: number;
    max_sender_gap_ms: number;
    wall_ms: number;
    audio_ms: number;
    drift_ms: number;
    sender_drift_ms: number;
    recorder: null | {
      stored_samples: number;
      stored_s: number;
      frames: number;
      missing_frames: number;
      stale_frames: number;
      stream_gap_events: number;
      silence_filled_events: number;
      correction_ppm: number;
    };
  };
}

export const RECEIVER_EXE = path.join(
  REPO_ROOT,
  "target",
  "debug",
  "examples",
  process.platform === "win32" ? "test_receiver.exe" : "test_receiver",
);

export interface ReceiverOptions {
  ports: number[];
  code: string;
  extensionId: string;
  clientsFile?: string;
  recordDir?: string;
  idle?: boolean;
}

export class Receiver {
  readonly events: ReceiverEvent[] = [];
  private buffer = "";
  private waiters: { pred: (e: ReceiverEvent) => boolean; resolve: (e: ReceiverEvent) => void }[] =
    [];
  exited = false;

  private constructor(private readonly proc: ChildProcessWithoutNullStreams) {
    proc.stdout.setEncoding("utf8");
    proc.stdout.on("data", (chunk: string) => this.onData(chunk));
    proc.stderr.on("data", (chunk: Buffer) => process.stderr.write(`[receiver] ${chunk}`));
    proc.on("exit", () => {
      this.exited = true;
    });
  }

  static async start(opts: ReceiverOptions): Promise<Receiver> {
    if (!fs.existsSync(RECEIVER_EXE)) {
      throw new Error(
        "test receiver not built: cargo build -p bridge --example test_receiver (npm run test:e2e does it)",
      );
    }
    const args = [
      ...opts.ports.flatMap((p) => ["--port", String(p)]),
      "--code",
      opts.code,
      "--extension-id",
      opts.extensionId,
    ];
    if (opts.clientsFile) args.push("--clients-file", opts.clientsFile);
    if (opts.recordDir) args.push("--record-dir", opts.recordDir);
    if (opts.idle) args.push("--idle");
    const r = new Receiver(spawn(RECEIVER_EXE, args, { stdio: ["pipe", "pipe", "pipe"] }));
    await r.waitFor((e) => e.event === "listening", 10_000);
    return r;
  }

  private onData(chunk: string): void {
    this.buffer += chunk;
    let nl: number;
    while ((nl = this.buffer.indexOf("\n")) >= 0) {
      const line = this.buffer.slice(0, nl).trim();
      this.buffer = this.buffer.slice(nl + 1);
      if (!line) continue;
      try {
        const ev = JSON.parse(line) as ReceiverEvent;
        this.events.push(ev);
        this.waiters = this.waiters.filter((w) => {
          if (!w.pred(ev)) return true;
          w.resolve(ev);
          return false;
        });
      } catch {
        /* not a JSON line */
      }
    }
  }

  /** The first event (already seen or still to come) that matches. */
  waitFor(pred: (e: ReceiverEvent) => boolean, timeoutMs = 15_000): Promise<ReceiverEvent> {
    const seen = this.events.find(pred);
    if (seen) return Promise.resolve(seen);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(
        () =>
          reject(new Error(`timeout waiting for a receiver event (${this.events.length} seen)`)),
        timeoutMs,
      );
      this.waiters.push({
        pred,
        resolve: (e) => {
          clearTimeout(timer);
          resolve(e);
        },
      });
    });
  }

  async waitForEnd(streamId: string, timeoutMs = 20_000): Promise<StreamEnd> {
    return (await this.waitFor(
      (e) => e.event === "stream_ended" && e.stream_id === streamId,
      timeoutMs,
    )) as StreamEnd;
  }

  of(event: string): ReceiverEvent[] {
    return this.events.filter((e) => e.event === event);
  }

  /** `drop`, `record` or `idle` (see the example's documentation). */
  command(cmd: string): void {
    this.proc.stdin.write(`${cmd}\n`);
  }

  async kill(): Promise<void> {
    if (this.exited) return;
    const done = new Promise<void>((resolve) => this.proc.once("exit", () => resolve()));
    this.proc.kill();
    await done;
  }
}
