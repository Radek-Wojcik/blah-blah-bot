import { defineConfig } from "@playwright/test";

/**
 * Two suites:
 * - `mock`: the UI against the mock backend in a browser. Runs in CI.
 * - `real`: the built application driven through its WebView (needs a debug build of the
 *   app, see e2e/real.spec.ts). Runs locally before a release.
 */
export default defineConfig({
  timeout: 60_000,
  fullyParallel: false,
  workers: 1,
  reporter: [["list"]],
  projects: [
    {
      name: "mock",
      testDir: "./tests",
      use: {
        baseURL: "http://localhost:1420",
        // The Edge that ships with Windows; no browser download needed.
        channel: "msedge",
        viewport: { width: 1280, height: 800 },
      },
    },
    { name: "real", testDir: "./e2e", timeout: 120_000 },
  ],
  webServer: {
    command: "npm run dev",
    url: "http://localhost:1420",
    reuseExistingServer: true,
    timeout: 30_000,
  },
});
