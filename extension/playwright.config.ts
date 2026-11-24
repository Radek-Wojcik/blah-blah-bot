import { defineConfig } from "@playwright/test";

/**
 * Scripted Chrome tests of the extension. They run locally only (CI has no use for a second
 * Chrome): `npm run test:e2e` builds the test build and the test receiver first.
 *
 * - `bridge`: pairing through the popup, a synthetic stream, service worker and receiver
 *   restarts, a fake Meet page.
 * - `measure`: the continuity measurement (`npm run measure`).
 *
 * Chrome profiles and receiver data live under `<repo>/target/e2e-tmp`, never in the user's
 * temp directory or profile.
 */
export default defineConfig({
  testDir: "./e2e",
  globalSetup: "./e2e/support/global-setup.ts",
  globalTeardown: "./e2e/support/global-teardown.ts",
  fullyParallel: false,
  workers: 1,
  reporter: [["list"]],
  projects: [
    { name: "bridge", testMatch: "bridge.spec.ts", timeout: 180_000 },
    { name: "measure", testMatch: "measure.spec.ts", timeout: 3 * 60 * 60 * 1000 },
  ],
});
