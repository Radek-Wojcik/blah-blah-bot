// Flows shared by the scripted tests.
import type { Page } from "@playwright/test";
import type { Chrome } from "./chrome";

/** Opens the popup page in a tab, as the toolbar button would show it. */
export async function openPopup(bc: Chrome): Promise<Page> {
  const page = await bc.context.newPage();
  await page.goto(`chrome-extension://${bc.extensionId}/popup.html`);
  return page;
}

/** Turns off "capture when the popup opens": a popup in a tab would capture itself. */
export async function disableAutoCapture(bc: Chrome): Promise<void> {
  const sw = await bc.serviceWorker();
  await sw.evaluate(() => chrome.storage.local.set({ autoCapture: false }));
}

/** Pairs the extension by typing the code into the popup. Returns the status line shown. */
export async function pairViaPopup(bc: Chrome, code: string): Promise<string> {
  await disableAutoCapture(bc);
  const popup = await openPopup(bc);
  try {
    await popup.locator("#pair").waitFor({ state: "visible" });
    await popup.fill("#code", code);
    await popup.click("#pairBtn");
    await popup.locator("#capture").waitFor({ state: "visible", timeout: 15_000 });
    return (await popup.locator("#line").textContent()) ?? "";
  } finally {
    await popup.close();
  }
}

/** A fake Meet page: a call URL that plays a tone, so the tab is audible. */
export const FAKE_MEET_HTML = `<!doctype html>
<html><head><meta charset="utf-8"><title>Meet (test page)</title></head>
<body><p>Fake call</p>
<script>
  const ctx = new AudioContext();
  const osc = ctx.createOscillator();
  osc.frequency.value = 330;
  const gain = ctx.createGain();
  gain.gain.value = 0.02;
  osc.connect(gain).connect(ctx.destination);
  osc.start();
  ctx.resume();
</script></body></html>`;

export const FAKE_MEET_CODE = "abc-defg-hij";
