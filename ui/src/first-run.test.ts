import { describe, expect, it } from "vitest";
import {
  bannerText,
  downloadFailure,
  fileName,
  finishState,
  firstOpenStep,
  formatBytes,
  parentDir,
  pathProblem,
  stepForCode,
  stepForError,
  stepForItem,
  stepState,
} from "./first-run";
import { demoModels, demoProcessingSettings } from "./ipc/mock-demo";
import { mockSetupStatus } from "./ipc/mock-readiness";

const allCached = () => demoModels().map((m) => ({ ...m, cached: true }));
const complete = () => mockSetupStatus(demoProcessingSettings(), allCached());
const withoutLlama = () =>
  mockSetupStatus({ ...demoProcessingSettings(), llamaServerPath: "" }, allCached());

describe("steps of the setup items and codes", () => {
  it("sends every item to the step that fixes it", () => {
    expect(stepForItem("workerEnv")).toBe("env");
    expect(stepForItem("model:whisper-turbo")).toBe("models");
    expect(stepForItem("model:diarization")).toBe("models");
    expect(stepForItem("hfToken")).toBe("models");
    expect(stepForItem("llamaServer")).toBe("llama");
    expect(stepForItem("llmModel")).toBe("llm");
    expect(stepForItem("vision")).toBe("vision");
    expect(stepForItem("other")).toBeNull();
  });

  it("maps the codes of a refused processing, with or without a model key", () => {
    expect(stepForCode("setup_worker_env")).toBe("env");
    expect(stepForCode("setup_model_missing")).toBe("models");
    expect(stepForCode("setup_model_missing:diarization")).toBe("models");
    expect(stepForCode("setup_hf_token")).toBe("models");
    expect(stepForCode("setup_llama_server")).toBe("llama");
    expect(stepForCode("setup_llm_model")).toBe("llm");
    expect(stepForCode("setup_vision")).toBe("vision");
    expect(stepForCode("setup_incomplete")).toBe("summary");
    expect(stepForCode("busy")).toBeNull();
    expect(stepForCode(undefined)).toBeNull();
  });

  it("recognizes a setup error among other rejections", () => {
    expect(stepForError({ code: "setup_vision", message: "x" })).toBe("vision");
    expect(stepForError({ code: "not_ready", message: "x" })).toBeNull();
    expect(stepForError(new Error("boom"))).toBeNull();
    expect(stepForError(null)).toBeNull();
  });
});

describe("readiness of the steps", () => {
  it("is quiet when everything is set up", () => {
    const status = complete();
    expect(status.ready).toBe(true);
    expect(bannerText(status)).toBeNull();
    expect(firstOpenStep(status)).toBe("summary");
    expect(finishState(status)).toEqual({ label: "Continue", note: null });
    expect(stepState(status, "llama")).toBe("ok");
    expect(stepState(status, "welcome")).toBe("none");
  });

  it("names what is missing and where to go first", () => {
    const status = withoutLlama();
    expect(status.ready).toBe(false);
    expect(firstOpenStep(status)).toBe("llama");
    expect(stepState(status, "llama")).toBe("todo");
    expect(bannerText(status)).toBe(
      "Setup is incomplete: llama-server. Processing stops until this is fixed.",
    );
    const finish = finishState(status);
    expect(finish.label).toBe("Continue anyway");
    expect(finish.note).toContain("llama-server");
  });

  it("does not count a skipped item as a problem", () => {
    const status = mockSetupStatus(
      {
        ...demoProcessingSettings(),
        llamaServerPath: "",
        describeScreenshots: false,
        skipReports: true,
      },
      allCached(),
    );
    expect(status.ready).toBe(true);
    expect(bannerText(status)).toBeNull();
  });
});

describe("texts", () => {
  it("gives the message of a failed path check together with the fix", () => {
    expect(pathProblem({ ok: false, code: "path_not_found", message: "No file at X." })).toBe(
      "No file at X. Check the spelling, or pick one of the files found.",
    );
    expect(pathProblem({ ok: false, code: "odd", message: "Odd." })).toBe("Odd.");
    expect(pathProblem({ ok: false, code: null, message: null })).toBe("The path cannot be used.");
  });

  it("tells a failed download from progress", () => {
    expect(downloadFailure("Downloading 40%")).toBeNull();
    expect(downloadFailure("Failed: network error")).toBe("network error");
    expect(downloadFailure("Failed:")).toBe("The download failed.");
  });

  it("formats sizes and paths", () => {
    expect(formatBytes(7.3 * 1024 ** 3)).toBe("7.3 GB");
    expect(formatBytes(900 * 1024 ** 2)).toBe("900 MB");
    expect(formatBytes(10)).toBe("1 KB");
    expect(parentDir("D:\\Models\\a.gguf")).toBe("D:\\Models");
    expect(parentDir("a.gguf")).toBe("");
    expect(fileName("D:\\Models\\a.gguf")).toBe("a.gguf");
    expect(fileName("a.gguf")).toBe("a.gguf");
  });
});
