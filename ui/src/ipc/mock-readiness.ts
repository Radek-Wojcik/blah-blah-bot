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
 * A minimal stand-in for the setup readiness report of the real application, derived from the
 * settings and the model list of the mock backend. The first-run wizard replaces it with
 * scenarios of its own.
 */
export function mockSetupStatus(
  settings: ProcessingSettings,
  models: ModelStatus[],
  llamaCandidates: string[] = [],
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
  const llamaPath = settings.llamaServerPath || llamaCandidates[0] || "";
  const items: SetupItem[] = [
    ok("workerEnv", "The processing environment is ready."),
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
  return {
    ready: items.every((i) => i.state === "ok" || i.state === "skipped"),
    firstRunDone: settings.firstRunDone,
    items,
    llamaCandidates,
  };
}

/** The path rules of the real application in short: absolute, and a file of the right kind. */
export function mockCheckSetupPath(kind: SetupPathKind, path: string): SetupPathCheck {
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
  return { ok: true, code: null, message: null };
}

export function mockGgufModels(): GgufFile[] {
  return [];
}
