// Scripted Chrome against the bridge (the real server and hub in test_receiver): pairing
// through the popup, a synthetic stream through the worklet and the socket, a service worker
// that is stopped and woken, a connection that drops, a receiver that restarts, and a fake
// Meet page. Nothing leaves the machine: the Meet page is answered by the test itself.
import fs from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { launchChrome, hook, stopServiceWorkers, TEST_PORTS, type Chrome } from "./support/chrome";
import { FAKE_MEET_CODE, FAKE_MEET_HTML, openPopup, pairViaPopup } from "./support/flows";
import { Receiver } from "./support/receiver";
import { cleanupDirs, freshDir, REPO_ROOT } from "./support/tmp";

const CODE = "7K3M9QXD";
const manifest = JSON.parse(
  fs.readFileSync(path.join(REPO_ROOT, "extension", "manifest.json"), "utf8"),
) as { key: string };

let bc: Chrome;
let receiver: Receiver;
let work: string;
let clientsFile: string;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function startReceiver(): Promise<Receiver> {
  return Receiver.start({
    ports: TEST_PORTS,
    code: CODE,
    extensionId: bc.extensionId,
    clientsFile,
    recordDir: path.join(work, "record"),
  });
}

async function streamStatus(bc: Chrome) {
  // A service worker does not receive its own messages, so ask through an extension page.
  const page = await bc.context.newPage();
  try {
    await page.goto(`chrome-extension://${bc.extensionId}/popup.html`);
    return await page.evaluate(async () => {
      const reply = await chrome.runtime.sendMessage({ target: "service-worker", type: "status" });
      return reply.status as {
        paired: boolean;
        clientState: string;
        streams: { tabId: number; status: string; framesSent: number; pendingFrames: number }[];
      };
    });
  } finally {
    await page.close();
  }
}

test.describe.configure({ mode: "serial" });

test.beforeAll(async () => {
  work = freshDir("bridge");
  clientsFile = path.join(work, "clients.json");
  // The id is known only once the extension runs: start Chrome first, then the receiver.
  bc = await launchChrome(path.join(work, "profile"));
  receiver = await startReceiver();
});

test.afterAll(async () => {
  try {
    await bc?.close();
  } finally {
    try {
      await receiver?.kill();
    } finally {
      cleanupDirs();
    }
  }
});

test("the extension id follows from the key in the manifest", async () => {
  const { createHash } = await import("node:crypto");
  const der = Buffer.from(manifest.key, "base64");
  const id = [...createHash("sha256").update(der).digest().subarray(0, 16)]
    .map((b) => String.fromCharCode(97 + (b >> 4)) + String.fromCharCode(97 + (b & 15)))
    .join("");
  expect(bc.extensionId).toBe(id);
  expect(bc.extensionId).toBe("pbgjebpmicmffhjbphlgginplaflodnj");
});

test("a wrong pairing code is refused and shown in the popup", async () => {
  const sw = await bc.serviceWorker();
  await sw.evaluate(() => chrome.storage.local.set({ autoCapture: false }));
  const popup = await openPopup(bc);
  await popup.fill("#code", "AAAAAAAA");
  await popup.click("#pairBtn");
  await expect(popup.locator("#line")).toContainText("bad_code");
  await expect(popup.locator("#pair")).toBeVisible();
  await popup.close();
});

test("pairs through the popup", async () => {
  const line = await pairViaPopup(bc, CODE);
  expect(line).toContain("Paired");
  const paired = await receiver.waitFor((e) => e.event === "paired");
  expect(String(paired.client)).toContain("Chrome");
  const sw = await bc.serviceWorker();
  const stored = await sw.evaluate(() => chrome.storage.local.get(["credentials", "port"]));
  expect((stored.credentials as { secret: string }).secret).toHaveLength(64);
  expect(stored.port).toBe(TEST_PORTS[0]);
  const status = await streamStatus(bc);
  expect(status.paired).toBe(true);
  expect(status.clientState).toBe("ready");
});

test("streams a synthetic tab through the worklet to the recorder", async () => {
  const started = await hook(bc, { type: "synthetic-start", tabId: 101, hz: 440 });
  expect(started.ok, JSON.stringify(started)).toBe(true);
  const id = started.streamId as string;
  const ev = await receiver.waitFor((e) => e.event === "stream_started" && e.stream_id === id);
  expect(ev.host).toBe("tone.test");
  await sleep(4000);
  await hook(bc, { type: "synthetic-stop", tabId: 101 });
  const end = await receiver.waitForEnd(id);
  expect(end.reason).toBe("user_stop");
  expect(end.stats.frames).toBeGreaterThan(150); // 4 s are 200 frames
  expect(end.stats.lost_frames).toBe(0);
  expect(end.stats.recorder?.missing_frames).toBe(0);
  expect(end.stats.recorder?.stream_gap_events).toBe(0);
  expect(end.stats.recorder?.stored_s).toBeGreaterThan(3.5);
});

test("a stopped service worker loses nothing", async () => {
  const started = await hook(bc, { type: "synthetic-start", tabId: 102 });
  const id = started.streamId as string;
  await receiver.waitFor((e) => e.event === "stream_started" && e.stream_id === id);
  await sleep(1500);

  const page = await openPopup(bc);
  await stopServiceWorkers(bc.context, page);
  await sleep(2500); // the stream keeps flowing with no service worker

  // The next event wakes a new worker, which finds everything in the offscreen document.
  await page.reload();
  await expect(page.locator("#count")).toHaveText("1", { timeout: 15_000 });
  const sw2 = await bc.serviceWorker();
  expect(await sw2.evaluate(() => chrome.runtime.id)).toBe(bc.extensionId); // a live new worker
  await page.close();

  const status = await streamStatus(bc);
  expect(status.clientState).toBe("ready");
  expect(status.streams.map((s) => s.tabId)).toEqual([102]);
  await hook(bc, { type: "synthetic-stop", tabId: 102 });
  const end = await receiver.waitForEnd(id);
  expect(end.stats.lost_frames).toBe(0);
  expect(end.stats.frames).toBeGreaterThan(150);
});

test("a dropped connection resumes the same stream", async () => {
  const started = await hook(bc, { type: "synthetic-start", tabId: 103 });
  const id = started.streamId as string;
  await receiver.waitFor((e) => e.event === "stream_started" && e.stream_id === id);
  await sleep(1500);
  const startedBefore = receiver.of("stream_started").length;

  receiver.command("drop");
  await sleep(3000);
  const status = await streamStatus(bc);
  expect(status.clientState).toBe("ready");
  expect(status.streams[0]?.status).toBe("active");
  expect(receiver.of("stream_started").length).toBe(startedBefore); // resumed, not restarted

  await hook(bc, { type: "synthetic-stop", tabId: 103 });
  const end = await receiver.waitForEnd(id);
  expect(end.reason).toBe("user_stop");
  expect(end.stats.frames).toBeGreaterThan(200);
  // Frames still in flight when the connection died may be lost; the buffered ones are not.
  expect(end.stats.lost_frames).toBeLessThanOrEqual(10);
  expect(end.stats.recorder?.stored_s).toBeGreaterThan(4);
});

test("a restarted receiver is found again and the stream resumes", async () => {
  const started = await hook(bc, { type: "synthetic-start", tabId: 104 });
  const id = started.streamId as string;
  await receiver.waitFor((e) => e.event === "stream_started" && e.stream_id === id);
  await sleep(1500);

  await receiver.kill();
  await sleep(3500); // frames are buffered meanwhile
  receiver = await startReceiver();
  // The new receiver does not know the stream: the extension offers it again.
  const again = await receiver.waitFor(
    (e) => e.event === "stream_started" && e.stream_id === id,
    30_000,
  );
  expect(again.tab_id).toBe(104);
  await sleep(2000);

  const status = await streamStatus(bc);
  expect(status.clientState).toBe("ready");
  expect(status.streams[0]?.status).toBe("active");
  expect(status.streams[0]?.pendingFrames).toBeLessThan(10);
  await hook(bc, { type: "synthetic-stop", tabId: 104 });
  const end = await receiver.waitForEnd(id);
  // The buffered seconds arrive on the new connection (backlog is drained at a limited rate).
  expect(end.stats.frames).toBeGreaterThan(150);
});

test("an idle application keeps the capture waiting until a recording starts", async () => {
  receiver.command("idle");
  await sleep(500);
  const started = await hook(bc, { type: "synthetic-start", tabId: 105 });
  const id = started.streamId as string;
  await expect
    .poll(async () => (await streamStatus(bc)).streams[0]?.status, { timeout: 10_000 })
    .toBe("waiting");
  const before = receiver.of("stream_started").length;
  receiver.command("record");
  await expect
    .poll(async () => (await streamStatus(bc)).streams[0]?.status, { timeout: 10_000 })
    .toBe("active");
  expect(receiver.of("stream_started").length).toBe(before + 1);
  await sleep(1500);
  await hook(bc, { type: "synthetic-stop", tabId: 105 });
  const end = await receiver.waitForEnd(id);
  expect(end.stats.frames).toBeGreaterThan(30);
});

test("a fake Meet page is reported as joined and left, never with its code", async () => {
  await bc.context.route("https://meet.google.com/**", (route) =>
    route.fulfill({ status: 200, contentType: "text/html", body: FAKE_MEET_HTML }),
  );
  const sw = await bc.serviceWorker();
  const page = await bc.context.newPage();
  await page.goto(`https://meet.google.com/${FAKE_MEET_CODE}`);

  const joined = await receiver.waitFor((e) => e.event === "meet" && e.state === "joined", 30_000);
  expect(joined.host).toBe("meet.google.com");
  const badge = await sw.evaluate(async () => {
    const [tab] = await chrome.tabs.query({ url: "https://meet.google.com/*" });
    return chrome.action.getBadgeText({ tabId: tab.id as number });
  });
  expect(badge).toBe("MEET");

  await page.goto("https://meet.google.com/landing");
  await receiver.waitFor((e) => e.event === "meet" && e.state === "left", 15_000);
  await page.close();

  const text = JSON.stringify(receiver.events);
  expect(text).not.toContain(FAKE_MEET_CODE);
});
