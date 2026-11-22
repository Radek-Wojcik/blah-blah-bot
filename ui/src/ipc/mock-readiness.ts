import type {
  GgufFile,
  ModelStatus,
  ProcessingSettings,
  SetupItem,
  SetupPathCheck,
  SetupPathKind,
  SetupStatus,
} from "./types";

/**
 * A stand-in for the setup readiness report of the real application, derived from the settings
 * and the model list of the mock backend. `workerEnv` tells whether the processing environment
 * exists; `working` names the item that is being prepared right now.
 */
export function mockSetupStatus(
  settings: ProcessingSettings,
  models: ModelStatus[],
  llamaCandidates: string[] = [],
  workerEnv = true,
  working: string | null = null,
): SetupStatus {
  const cached = (key: string) => models.find((m) => m.key === key)?.cached ?? false;
  const ok = (key: string, message: string): SetupItem => ({ key, state: "ok", message });
  const problem = (
    key: string,
    code: SetupItem["code"],
    message: string,
    fixHint: string,
  ): SetupItem => ({ key, state: "missing", code, message, fixHint });
  const modelItem = (key: "whisper-turbo" | "diarization", name: string): SetupItem =>
    cached(key)
      ? ok(`model:${key}`, `${name} is downloaded.`)
      : problem(
          `model:${key}`,
          "setup_model_missing",
          `${name} is not downloaded.`,
          "Download it in the setup.",
        );
  const visionOn = settings.describeScreenshots;
  const managed = settings.llm?.defaultKind !== "external";
  const llamaNeeded = visionOn || (managed && !settings.skipReports);
  const llamaPath = settings.llamaServerPath;
  const items: SetupItem[] = [
    workerEnv
      ? ok("workerEnv", "The processing environment is ready.")
      : problem(
          "workerEnv",
          "setup_worker_env",
          "The processing environment has not been created yet.",
          "Prepare it in the setup.",
        ),
    modelItem("whisper-turbo", "The speech recognition model (Whisper turbo)"),
    modelItem("diarization", "The speaker model (pyannote)"),
    settings.hfTokenSet
      ? ok("hfToken", "A Hugging Face token is stored.")
      : cached("diarization")
        ? { key: "hfToken", state: "skipped", message: "No token is stored and none is needed." }
        : problem(
            "hfToken",
            "setup_hf_token",
            "No Hugging Face token is stored. The speaker model needs one.",
            "Enter a Hugging Face token in the setup.",
          ),
    llamaPath !== ""
      ? ok("llamaServer", `llama-server: ${llamaPath}.`)
      : llamaNeeded
        ? problem(
            "llamaServer",
            "setup_llama_server",
            "llama-server was not found on this computer.",
            "Install llama.cpp, then detect it in the setup.",
          )
        : { key: "llamaServer", state: "skipped", message: "llama-server is not needed." },
    settings.llm?.managed.modelPath
      ? ok("llmModel", "A language model is chosen.")
      : settings.skipReports
        ? { key: "llmModel", state: "skipped", message: "Reports are turned off." }
        : problem(
            "llmModel",
            "setup_llm_model",
            "No language model was chosen, so no report can be written.",
            "Choose a .gguf model file, use an external server, or skip reports for now.",
          ),
    !visionOn
      ? { key: "vision", state: "skipped", message: "Screenshot analysis is turned off." }
      : cached("vision")
        ? ok("vision", "The screenshot model is downloaded.")
        : problem(
            "vision",
            "setup_vision",
            "The screenshot model is not downloaded.",
            "Download it in the setup, or skip screenshot analysis.",
          ),
  ];
  const shown = items.map((i): SetupItem => {
    if (i.key !== working) return i;
    return { key: i.key, state: "working", message: "Working on it." };
  });
  return {
    ready: shown.every((i) => i.state === "ok" || i.state === "skipped"),
    firstRunDone: settings.firstRunDone,
    items: shown,
    llamaCandidates,
  };
}

/**
 * The path rules of the real application in short: absolute, a file of the right kind and, when
 * the files that exist are given, an existing one.
 */
export function mockCheckSetupPath(
  kind: SetupPathKind,
  path: string,
  existing?: ReadonlySet<string>,
): SetupPathCheck {
  const fail = (code: string, message: string): SetupPathCheck => ({ ok: false, code, message });
  const p = path.trim();
  if (p === "") return fail("path_empty", "No path was given.");
  if (!/^[A-Za-z]:[\\/]/.test(p)) return fail("path_not_absolute", "The path must be absolute.");
  const lower = p.toLowerCase();
  const wrong = (what: string) => fail("path_wrong_kind", `${what} is expected: ${p}`);
  if (kind === "uv" && !/[\\/]uv\.exe$/.test(lower)) return wrong("uv.exe");
  if (kind === "llamaServer" && !/llama-server[^\\/]*\.exe$/.test(lower))
    return wrong("llama-server.exe");
  if (kind === "llmModel" && !lower.endsWith(".gguf")) return wrong("A .gguf file");
  if (existing && !existing.has(p)) return fail("path_not_found", `No file exists at ${p}`);
  return { ok: true, code: null, message: null };
}

/** The GGUF files of `dir` (every known one without a folder), as the real scan lists them. */
export function mockGgufModels(known: GgufFile[], dir?: string): GgufFile[] {
  if (dir === undefined || dir.trim() === "") return [...known];
  const prefix = dir
    .trim()
    .replace(/[\\/]+$/, "")
    .toLowerCase();
  return known.filter((g) => g.path.toLowerCase().startsWith(`${prefix}\\`));
}
