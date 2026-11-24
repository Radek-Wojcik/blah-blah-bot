// Scripted Chrome with the unpacked test build of the extension.
//
// Rules: a temporary profile under target/e2e-tmp (never the user's profile, no sign-in),
// a window placed far outside the screen (headless Chrome never reports a tab as audible,
// which the Meet test needs), and only the browser started here is ever closed or killed. Chrome 137 and later
// ignore --load-extension in branded builds; the installed Chrome here is older. `loadViaCdp`
// is the documented replacement (Extensions.loadUnpacked behind
// --enable-unsafe-extension-debugging) and is used when the flag did not load the extension.
import { execFileSync } from "node:child_process";
import path from "node:path";
import { chromium, type BrowserContext, type Page, type Worker } from "@playwright/test";
import { REPO_ROOT } from "./tmp";

export const TEST_EXTENSION_DIR = path.join(REPO_ROOT, "target", "extension", "test-unpacked");
/** Ports the test build listens for (scripts/build.mjs --test). */
export const TEST_PORTS = [17564, 17563];

export interface Chrome {
  context: BrowserContext;
  extensionId: string;
  version: string;
  serviceWorker(): Promise<Worker>;
  close(): Promise<void>;
}

/** Kills what is left of a browser that was started with this profile (and only that). */
function killProfileProcesses(profileDir: string): void {
  if (process.platform !== "win32") return;
  const needle = profileDir.replace(/'/g, "''");
  try {
    execFileSync(
      "powershell",
      [
        "-NoProfile",
        "-Command",
        `Get-CimInstance Win32_Process -Filter "Name='chrome.exe'" | Where-Object { $_.CommandLine -like '*${needle}*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`,
      ],
      { stdio: "ignore" },
    );
  } catch {
    /* nothing to kill */
  }
}

/** Waits until the extension APIs exist in the worker; right after start they can be missing. */
async function ready(worker: Worker): Promise<Worker> {
  for (let i = 0; i < 50; i++) {
    const ok = await worker
      .evaluate(() => typeof chrome !== "undefined" && !!chrome.storage)
      .catch(() => false);
    if (ok) return worker;
    await new Promise((r) => setTimeout(r, 100));
  }
  return worker;
}

async function findServiceWorker(context: BrowserContext): Promise<Worker> {
  return ready(await findRawServiceWorker(context));
}

async function findRawServiceWorker(context: BrowserContext): Promise<Worker> {
  // The newest worker: a stopped one stays in the list until Playwright notices.
  const existing = context
    .serviceWorkers()
    .filter((w) => w.url().startsWith("chrome-extension://"))
    .at(-1);
  if (existing) return existing;
  return context.waitForEvent("serviceworker", {
    predicate: (w) => w.url().startsWith("chrome-extension://"),
    timeout: 15_000,
  });
}

export async function launchChrome(profileDir: string): Promise<Chrome> {
  const context = await chromium.launchPersistentContext(profileDir, {
    channel: "chrome",
    headless: false,
    args: [
      `--disable-extensions-except=${TEST_EXTENSION_DIR}`,
      `--load-extension=${TEST_EXTENSION_DIR}`,
      "--autoplay-policy=no-user-gesture-required",
      "--no-first-run",
      "--no-default-browser-check",
      "--disable-sync",
      "--disable-background-networking",
      "--disable-component-update",
      "--window-position=-2400,-2400",
      "--window-size=500,400",
    ],
  });
  try {
    // Nothing of the test may reach Google: the fake Meet page is answered by the test itself,
    // every other request to a Google host is refused.
    await context.route(/^https?:\/\/([^/]*\.)?(google|gstatic|googleapis)\.[a-z.]+\//, (r) =>
      r.abort(),
    );
    const sw = await findServiceWorker(context);
    const extensionId = new URL(sw.url()).host;
    const version = context.browser()?.version() ?? "unknown";
    return {
      context,
      extensionId,
      version,
      serviceWorker: () => findServiceWorker(context),
      async close() {
        try {
          await context.close();
        } finally {
          killProfileProcesses(profileDir);
        }
      },
    };
  } catch (err) {
    await context.close().catch(() => undefined);
    killProfileProcesses(profileDir);
    throw err;
  }
}

/**
 * Stops the service worker of the extension, as Chrome does after 30 s without events.
 * `page` must be an extension page: the ServiceWorker domain only sees the storage partition
 * of the page it is attached to.
 */
export async function stopServiceWorkers(context: BrowserContext, page: Page): Promise<void> {
  const cdp = await context.newCDPSession(page);
  try {
    const stopped = new Promise<void>((resolve) => {
      cdp.on("ServiceWorker.workerVersionUpdated", (e) => {
        if (e.versions.some((v) => v.runningStatus === "stopped")) resolve();
      });
    });
    await cdp.send("ServiceWorker.enable");
    await cdp.send("ServiceWorker.stopAllWorkers");
    await Promise.race([
      stopped,
      new Promise<void>((_, reject) =>
        setTimeout(() => reject(new Error("the service worker did not stop")), 10_000),
      ),
    ]);
  } finally {
    await cdp.detach().catch(() => undefined);
  }
}

/** Sends a message from the service worker to the offscreen document (test hook). */
export async function hook<T = { ok: boolean; streamId?: string; error?: string }>(
  bc: Chrome,
  message: Record<string, unknown>,
): Promise<T> {
  const sw = await bc.serviceWorker();
  return sw.evaluate(
    (m) => chrome.runtime.sendMessage({ target: "offscreen-test", ...m }),
    message,
  ) as Promise<T>;
}
