import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

describe("content security policy of the shell", () => {
  const conf = JSON.parse(
    readFileSync(resolve(process.cwd(), "../crates/app/tauri.conf.json"), "utf8"),
  ) as { app: { security: { csp: string } } };
  const csp = conf.app.security.csp;

  it("lets the page reach the IPC protocol, or every command falls back to postMessage", () => {
    const connect = /connect-src ([^;]*)/.exec(csp)?.[1] ?? "";
    expect(connect).toContain("ipc:");
    expect(connect).toContain("http://ipc.localhost");
  });

  it("lets images load from blob URLs, which carry the frozen desktop", () => {
    expect(/img-src [^;]*\bblob:/.test(csp)).toBe(true);
  });
});
