// Test-only hook, loaded by offscreen.html in the test build (scripts/build.mjs --test) and
// never present in the packaged extension. It lets the scripted Chrome tests start a
// synthetic tone stream that runs through the real worklet, session and socket, because the
// scripted tests cannot invoke the extension on a tab the way a user does.

import { offscreen } from "./offscreen.js";

interface SyntheticStart {
  target: "offscreen-test";
  type: "synthetic-start";
  tabId: number;
  title?: string;
  host?: string;
  hz?: number;
  groupId?: number;
}
interface SyntheticStop {
  target: "offscreen-test";
  type: "synthetic-stop";
  tabId: number;
}
type TestMessage = SyntheticStart | SyntheticStop;

const sources = new Map<number, { ctx: AudioContext; osc: OscillatorNode }>();

async function start(msg: SyntheticStart): Promise<string> {
  const ctx = new AudioContext();
  await ctx.resume();
  const osc = ctx.createOscillator();
  osc.frequency.value = msg.hz ?? 440;
  const gain = ctx.createGain();
  gain.gain.value = 0.2;
  const dest = ctx.createMediaStreamDestination();
  osc.connect(gain).connect(dest);
  osc.start();
  sources.set(msg.tabId, { ctx, osc });
  const streamId = `t${msg.tabId}-test${Date.now().toString(36)}`;
  await offscreen.mux.add(
    {
      streamId,
      tabId: msg.tabId,
      groupId: msg.groupId,
      title: msg.title ?? "Synthetic tone",
      host: msg.host ?? "tone.test",
    },
    dest.stream,
    false,
  );
  return streamId;
}

async function stop(tabId: number): Promise<void> {
  await offscreen.mux.stop(tabId, "user_stop");
  const s = sources.get(tabId);
  sources.delete(tabId);
  if (s) {
    s.osc.stop();
    await s.ctx.close();
  }
}

chrome.runtime.onMessage.addListener((msg: TestMessage, sender, sendResponse) => {
  if (sender.id !== chrome.runtime.id || msg?.target !== "offscreen-test") return false;
  const run = msg.type === "synthetic-start" ? start(msg) : stop(msg.tabId).then(() => "");
  run.then(
    (streamId) => sendResponse({ ok: true, streamId }),
    (err: unknown) => sendResponse({ ok: false, error: String((err as Error)?.message ?? err) }),
  );
  return true;
});
