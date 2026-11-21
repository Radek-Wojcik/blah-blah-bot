/**
 * End-to-end tests against the real application: the debug build of the Tauri shell is
 * started in demo mode (generated tones instead of audio devices) with its data in a
 * temporary folder, and driven through the WebView debugging port.
 *
 * Build first:  cargo build -p app --features custom-protocol   (after `npm run build`)
 * Run:          npx playwright test --project=real
 */
import { type ChildProcess, execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { type Browser, type Page, chromium, expect, test } from "@playwright/test";
import { cleanupDirs, freshDir } from "../support/e2e-tmp";

const EXE = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../target/debug/app.exe",
);
// The shared Python environment of the worker: no test installs a copy of its own.
const WORKER_ENV = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../worker/.venv");
let nextPort = 9330;

interface Running {
  proc: ChildProcess;
  browser: Browser;
  dataDir: string;
}

function pageWith(browser: Browser, view: string): Page | undefined {
  return browser
    .contexts()
    .flatMap((c) => c.pages())
    .find((p) => p.url().includes(`view=${view}`));
}

/**
 * Presses Record and confirms the report mode gate. A fresh data folder has no cycle yet, so the
 * gate offers to create one: the demo cycle is made and recorded with its default mode.
 */
async function recordThroughGate(widget: Page) {
  await widget.getByRole("button", { name: "Record" }).click();
  const gate = widget.getByRole("dialog", { name: "Choose the report for this recording" });
  await expect(gate).toBeVisible();
  await expect(gate.getByText("Loading the cycles…")).toHaveCount(0);
  const create = gate.getByRole("button", { name: "Create cycle" });
  if (await create.isVisible()) {
    await gate.getByLabel("Cycle id").fill("DEMO");
    await gate.getByLabel("Name", { exact: true }).fill("Demo cycle");
    await create.click();
  }
  await gate.getByRole("button", { name: "Start recording" }).click();
}

async function launch(dataDir: string): Promise<Running> {
  const port = nextPort++;
  const proc = spawn(EXE, [], {
    env: {
      ...process.env,
      BBB_DEMO: "1",
      BBB_DATA_DIR: dataDir,
      ...(fs.existsSync(WORKER_ENV) ? { BBB_WORKER_ENV_DIR: WORKER_ENV } : {}),
      WEBVIEW2_USER_DATA_FOLDER: path.join(dataDir, "webview"),
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}`,
    },
    stdio: "ignore",
  });
  let browser: Browser | undefined;
  for (let i = 0; i < 60 && !browser; i++) {
    try {
      browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
    } catch {
      await new Promise((r) => setTimeout(r, 500));
    }
  }
  if (!browser) throw new Error("the application did not open its debugging port");
  for (let i = 0; i < 40 && !(pageWith(browser, "widget") && pageWith(browser, "main")); i++) {
    await new Promise((r) => setTimeout(r, 250));
  }
  return { proc, browser, dataDir };
}

/** Hard kill of the whole process tree: no shutdown code runs. */
function kill(app: Running) {
  try {
    execFileSync("taskkill", ["/PID", String(app.proc.pid), "/T", "/F"], { stdio: "ignore" });
  } catch {
    /* already gone */
  }
}

test.skip(!fs.existsSync(EXE), "debug build of the app not found");

// Data folders live in <repo>/target/e2e-tmp (see support/e2e-tmp.ts) and are removed after
// every test, also a failed one; the apps are killed in each test's own `finally`.
test.afterEach(() => {
  cleanupDirs();
});

test("record three tracks, add a source, take a screenshot, stop", async () => {
  const dir = freshDir("flow");
  const app = await launch(dir);
  try {
    const widget = pageWith(app.browser, "widget")!;
    const main = pageWith(app.browser, "main")!;
    await widget.reload();

    // Microphone tone is preselected; add the Discord tone.
    await widget.getByLabel("Discord (demo tone)").check();
    await recordThroughGate(widget);
    await expect(widget.getByText("Recording", { exact: true })).toBeVisible();
    const sources = widget.getByRole("list", { name: "Recorded sources" });
    await expect(sources.getByRole("listitem")).toHaveCount(2);

    // Add a third source while recording.
    await widget.waitForTimeout(1_200);
    await widget.getByLabel("Add source").selectOption({ label: "Chrome (demo tone)" });
    await expect(sources.getByRole("listitem")).toHaveCount(3);
    await expect(widget.getByLabel("Elapsed time")).not.toHaveText("0:00");

    // Screenshot: the overlay window opens, a region is dragged.
    const overlayPromise = app.browser.contexts()[0].waitForEvent("page", { timeout: 20_000 });
    await widget.getByRole("button", { name: /Screenshot/ }).click();
    const overlay = await overlayPromise;
    await overlay.waitForLoadState();
    await expect(overlay.locator("img.shot-image")).toBeVisible({ timeout: 20_000 });
    // The picture must be decoded and accepted as non-blank: a broken image is "visible" too,
    // and used to leave a black screen over every monitor.
    await expect(overlay.getByRole("application")).toHaveAttribute("data-phase", "ready", {
      timeout: 30_000,
    });
    expect(
      await overlay.locator("img.shot-image").evaluate((el: HTMLImageElement) => el.naturalWidth),
    ).toBeGreaterThan(0);
    await overlay.mouse.move(100, 100);
    await overlay.mouse.down();
    await overlay.mouse.move(420, 300, { steps: 8 });
    await overlay.mouse.up();
    await expect(widget.getByRole("button", { name: /Screenshot/ })).toContainText("1", {
      timeout: 20_000,
    });

    await widget.waitForTimeout(800);
    await widget.getByRole("button", { name: "Stop" }).click();
    await expect(widget.getByText("Standby")).toBeVisible();

    // The meeting is listed with its tracks and screenshot.
    await main.reload();
    const card = main.getByRole("article").first();
    await expect(card).toContainText("Meeting 20");
    await expect(card.locator("dd").nth(1)).toHaveText("3");
    await expect(card.locator("dd").nth(2)).toHaveText("1");

    // Files on disk: journal, three track folders with segments, one screenshot.
    const root = path.join(dir, "recordings");
    const [meeting] = fs.readdirSync(root);
    const folder = path.join(root, meeting);
    expect(fs.existsSync(path.join(folder, "journal.jsonl"))).toBe(true);
    const tracks = fs
      .readdirSync(folder, { withFileTypes: true })
      .filter((e) => e.isDirectory() && e.name !== "img");
    expect(tracks.map((t) => t.name).sort()).toEqual(["tone-220", "tone-440", "tone-880"]);
    for (const t of tracks) {
      const segs = fs.readdirSync(path.join(folder, t.name));
      expect(segs.length).toBeGreaterThan(0);
    }
    const shots = fs.readdirSync(path.join(folder, "img"));
    expect(shots).toHaveLength(1);
    expect(shots[0]).toMatch(/^0001-\d\d-\d\d-\d\d\.png$/);
    // A PNG signature.
    const head = fs.readFileSync(path.join(folder, "img", shots[0])).subarray(0, 8);
    expect([...head]).toEqual([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  } finally {
    kill(app);
  }
});

test("a killed recording is found again and can be recovered", async () => {
  const dir = freshDir("kill");
  const first = await launch(dir);
  try {
    const widget = pageWith(first.browser, "widget")!;
    await widget.reload();
    await recordThroughGate(widget);
    await expect(widget.getByText("Recording", { exact: true })).toBeVisible();
    // Long enough for at least one 60 s segment to be closed and journaled.
    await widget.waitForTimeout(63_000);
  } finally {
    kill(first);
  }

  // Let Windows release the WebView data folder of the killed process before reopening it.
  await new Promise((r) => setTimeout(r, 5_000));
  const second = await launch(dir);
  try {
    const main = pageWith(second.browser, "main")!;
    await main.reload();
    const card = main.getByRole("article").first();
    await expect(card.getByText("Needs recovery")).toBeVisible();
    await card.getByRole("button", { name: "Recover" }).click();
    await expect(card.getByText("Recovered", { exact: true })).toBeVisible();
    await expect(card.locator("dd").nth(1)).toHaveText("1");
    // At most the segment being written was lost: the 60 s one survived.
    await expect(card.locator("dd").first()).not.toHaveText("0:00");
  } finally {
    kill(second);
  }
});

// Needs the models and a GPU: set BBB_MINI_DIR to the folder with the reference recordings
// (it holds `mini-en/track-magda.wav`). The recording itself is made from generated tones;
// the test then replaces the audio of the track with the scripted speech, as a stand-in for
// a recorded call, and processes it with the real worker.
test("record, process with the real models, open the transcript and exclude a passage", async () => {
  test.skip(!process.env.BBB_MINI_DIR, "BBB_MINI_DIR is not set");
  test.setTimeout(15 * 60_000);
  const speech = path.join(process.env.BBB_MINI_DIR!, "mini-en", "track-magda.wav");
  const dir = freshDir("process");
  const app = await launch(dir);
  try {
    const widget = pageWith(app.browser, "widget")!;
    const main = pageWith(app.browser, "main")!;
    await widget.reload();
    await recordThroughGate(widget);
    await expect(widget.getByText("Recording", { exact: true })).toBeVisible();
    await widget.waitForTimeout(1_500);
    await widget.getByRole("button", { name: "Stop" }).click();
    await expect(widget.getByText("Standby")).toBeVisible();

    // Swap the tone for speech in the first segment of the track.
    const root = path.join(dir, "recordings");
    const folder = path.join(root, fs.readdirSync(root)[0]);
    const track = fs
      .readdirSync(folder, { withFileTypes: true })
      .find((e) => e.isDirectory() && e.name !== "img")!.name;
    const segment = path.join(folder, track, fs.readdirSync(path.join(folder, track)).sort()[0]);
    fs.copyFileSync(speech, segment);

    await main.reload();
    const card = main.getByRole("article").first();
    await card.getByRole("button", { name: "Process" }).click();

    await main.getByRole("button", { name: "Queue", exact: true }).click();
    const stages = main.getByRole("list", { name: /^Stages of / });
    await expect(stages.getByRole("listitem").first()).toBeVisible({ timeout: 30_000 });
    // The last stage writes `descriptions.json`; wait for the whole job, not for a stage.
    const work = path.join(folder, "work");
    await expect
      .poll(() => fs.existsSync(path.join(work, "descriptions.json")), {
        timeout: 12 * 60_000,
        intervals: [2_000],
      })
      .toBe(true);

    // Results on disk.
    const transcript = JSON.parse(fs.readFileSync(path.join(work, "transcript.json"), "utf8"));
    expect(transcript.segments.length).toBeGreaterThan(0);

    // Open the transcript and exclude the first passage; the edit survives a reload.
    await main.reload();
    await main.getByRole("button", { name: "Meetings", exact: true }).click();
    await main.getByRole("article").first().getByRole("button", { name: "Transcript" }).click();
    const exclude = main.getByRole("button", { name: /^Exclude the segment at / }).first();
    await exclude.click();
    await expect(main.getByRole("button", { name: /^Include the segment at / })).toHaveCount(1);
    const edits = JSON.parse(fs.readFileSync(path.join(work, "edits.json"), "utf8"));
    expect(Object.values(edits.segments)).toEqual([{ excluded: true }]);
    await main.reload();
    await main.getByRole("article").first().getByRole("button", { name: "Transcript" }).click();
    await expect(main.getByRole("button", { name: /^Include the segment at / })).toHaveCount(1);
  } finally {
    kill(app);
  }
});
