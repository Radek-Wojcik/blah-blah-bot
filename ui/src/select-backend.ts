import type { Backend } from "./ipc/backend";
import { createMockBackend } from "./ipc/mock";
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
  return createMockBackend({
    recording: params.get("state") === "recording",
    // `newdraft=1` starts with a newer draft next to the edited one of the demo meeting.
    newDraft: params.get("newdraft") === "1",
    // `net=offline` starts with the network off: publishing waits until it is back.
    offline: params.get("net") === "offline",
    ...(rate === null ? {} : { clockRate: Number(rate) }),
    ...(jobms === null ? {} : { jobStepMs: Number(jobms) }),
    ...(fault === "blank" || fault === "black" || fault === "broken"
      ? { overlayFault: fault }
      : {}),
  });
}
