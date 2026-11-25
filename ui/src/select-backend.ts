import type { Backend } from "./ipc/backend";
import { createMockBackend } from "./ipc/mock";
import { parseSetupScenario } from "./ipc/mock-setup";
import { createTauriBackend } from "./ipc/tauri";

/**
 * The real backend inside the Tauri shell, the mock backend everywhere else (browser
 * development, UI tests, screenshot script). `?backend=mock` forces the mock.
 */
export function selectBackend(search: string = window.location.search): Backend {
  const params = new URLSearchParams(search);
  const inTauri = "__TAURI_INTERNALS__" in window;
  if (inTauri && params.get("backend") !== "mock") return createTauriBackend();
  // `rate=0` freezes the simulated clock, which makes screenshots reproducible.
  const rate = params.get("rate");
  // `jobms=N` sets how long a processing stage takes in the mock (tests use a short time).
  const jobms = params.get("jobms");
  // `overlayfault=blank|black|broken` makes the screenshot overlay receive a bad capture.
  const fault = params.get("overlayfault");
  // `setup=fresh|ready|no-vision|no-llama|download-fails` starts in a state of the first-run setup.
  const setup = parseSetupScenario(params.get("setup"));
  // `pairms=N` makes a simulated extension enter the pairing code after N milliseconds.
  const pairms = params.get("pairms");
  // `detectms=N` makes the mock detect a Discord call N milliseconds after start.
  const detectms = params.get("detectms");
  // `tabs=1` makes the mock extension offer captured tabs; `tabs=failed` adds a failed tab.
  const tabs = params.get("tabs");
  // `ghtoken=none` starts without a saved GitHub token; `ghfault=name-taken|offline` makes
  // creating a GitHub repository fail. The project wizard then falls back to a local project.
  const ghfault = params.get("ghfault");
  return createMockBackend({
    recording: params.get("state") === "recording",
    // `newdraft=1` starts with a newer draft next to the edited one of the demo meeting.
    newDraft: params.get("newdraft") === "1",
    // `net=offline` starts with the network off: publishing waits until it is back.
    offline: params.get("net") === "offline",
    ...(setup === undefined ? {} : { setup }),
    ...(rate === null ? {} : { clockRate: Number(rate) }),
    ...(jobms === null ? {} : { jobStepMs: Number(jobms) }),
    ...(pairms === null ? {} : { bridgePairMs: Number(pairms) }),
    ...(detectms === null ? {} : { detectMs: Number(detectms) }),
    ...(params.get("ghtoken") === "none" ? { noGithubToken: true } : {}),
    ...(ghfault === "name-taken" || ghfault === "offline" ? { githubCreateFault: ghfault } : {}),
    ...(tabs === "1" ? { browserTabs: "ok" as const } : {}),
    ...(tabs === "failed" ? { browserTabs: "failed" as const } : {}),
    ...(fault === "blank" || fault === "black" || fault === "broken"
      ? { overlayFault: fault }
      : {}),
  });
}
