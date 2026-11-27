import type { GgufFile, ModelStatus, ProcessingSettings } from "./types";

/**
 * Setup scenarios of the mock backend, chosen with `?setup=` (and `MockOptions.setup`). Without a
 * scenario the mock reports a finished setup, so the first-run wizard never shows in the demo.
 *
 * - `fresh`: nothing is set up; a llama-server and some model files can be found.
 * - `ready`: everything is set up, only the wizard has not been completed yet.
 * - `no-vision`: ready, except that the screenshot model is not downloaded.
 * - `no-llama`: ready, except that no llama-server is configured or found.
 * - `download-fails`: fresh, and the first download of every model fails.
 * - `setup-offline`: fresh, and the first run of each setup task fails with an offline error.
 */
export type SetupScenario =
  "fresh" | "ready" | "no-vision" | "no-llama" | "download-fails" | "setup-offline";

export const SETUP_SCENARIOS: SetupScenario[] = [
  "fresh",
  "ready",
  "no-vision",
  "no-llama",
  "download-fails",
  "setup-offline",
];

/** The scenario named by a query value, or undefined. */
export function parseSetupScenario(value: string | null): SetupScenario | undefined {
  return SETUP_SCENARIOS.find((s) => s === value);
}

/** What the simulated computer holds; the mock backend reads and changes it. */
export interface SetupWorld {
  /** A scenario was chosen: readiness is enforced like in the real application. */
  active: boolean;
  /** The processing environment exists. */
  workerEnv: boolean;
  /** Attempts to prepare the environment that still fail before one succeeds. */
  envFailures: number;
  /** Attempts to download llama-server that still fail before one succeeds. */
  llamaFailures: number;
  /** Attempts to download the default language model that still fail before one succeeds. */
  llmFailures: number;
  llamaCandidates: string[];
  ggufs: GgufFile[];
  /** Downloads that still fail before one succeeds, per model key. */
  downloadFailures: Record<string, number>;
  /** Files that exist; a path outside this set is "not found". */
  knownFiles: Set<string>;
}

export const DEMO_LLAMA = "C:\\Tools\\llama\\llama-server.exe";
export const DEMO_LLM = "D:\\Models\\gemma-4-12b-q4_k_m.gguf";
const DEMO_LLM_SMALL = "D:\\Models\\gemma-4-4b-q4_k_m.gguf";
const DEMO_MMPROJ = "D:\\Models\\mmproj-gemma-4-12b-f16.gguf";
const GB = 1024 ** 3;

export function demoGgufs(): GgufFile[] {
  return [
    { path: DEMO_LLM, size: Math.round(7.3 * GB), kind: "llm" },
    { path: DEMO_LLM_SMALL, size: Math.round(2.5 * GB), kind: "llm" },
    { path: DEMO_MMPROJ, size: Math.round(0.9 * GB), kind: "mmproj" },
  ];
}

/** The world of a finished demo setup (no scenario). */
export function idleWorld(): SetupWorld {
  return {
    active: false,
    workerEnv: true,
    envFailures: 0,
    llamaFailures: 0,
    llmFailures: 0,
    llamaCandidates: [],
    ggufs: [],
    downloadFailures: {},
    knownFiles: new Set(),
  };
}

/**
 * Puts `processing` and `models` into the state of the scenario (they are changed in place) and
 * returns the simulated computer.
 */
export function applySetupScenario(
  scenario: SetupScenario | undefined,
  processing: ProcessingSettings,
  models: ModelStatus[],
): SetupWorld {
  if (scenario === undefined) return idleWorld();
  const cache = (cached: Record<string, boolean>) => {
    for (const m of models) m.cached = cached[m.key] ?? false;
  };
  const ggufs = demoGgufs();
  const world: SetupWorld = {
    active: true,
    workerEnv: false,
    envFailures: 0,
    llamaFailures: 0,
    llmFailures: 0,
    llamaCandidates: [DEMO_LLAMA],
    ggufs,
    downloadFailures: {},
    knownFiles: new Set([DEMO_LLAMA, ...ggufs.map((g) => g.path)]),
  };
  processing.firstRunDone = false;
  processing.describeScreenshots = true;
  processing.skipReports = false;
  if (scenario === "fresh" || scenario === "download-fails" || scenario === "setup-offline") {
    processing.hfTokenSet = false;
    processing.llamaServerPath = "";
    processing.llm!.managed.modelPath = "";
    cache({});
    if (scenario === "download-fails") {
      world.workerEnv = true;
      world.downloadFailures = { "whisper-turbo": 1, diarization: 1, vision: 1 };
    }
    if (scenario === "setup-offline") {
      world.envFailures = 1;
      world.llamaFailures = 1;
      world.llmFailures = 1;
    }
    return world;
  }
  // The other scenarios start from a complete setup.
  world.workerEnv = true;
  processing.hfTokenSet = true;
  processing.llamaServerPath = DEMO_LLAMA;
  processing.llm!.managed.modelPath = DEMO_LLM;
  cache({ "whisper-turbo": true, diarization: true, vision: scenario !== "no-vision" });
  if (scenario === "no-llama") {
    processing.llamaServerPath = "";
    world.llamaCandidates = [];
    world.knownFiles.delete(DEMO_LLAMA);
  }
  return world;
}
