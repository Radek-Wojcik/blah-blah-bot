import { describe, expect, it } from "vitest";
import { demoModels, demoProcessingSettings } from "./mock-demo";
import { createMockBackend } from "./mock";
import { mockCheckSetupPath, mockSetupStatus } from "./mock-readiness";

describe("setup readiness (mock)", () => {
  it("reports each problem with a code, a message and a fix", () => {
    const settings = {
      ...demoProcessingSettings(),
      llamaServerPath: "",
      llm: { ...demoProcessingSettings().llm!, managed: { modelPath: "", ctx: 16384 } },
    };
    const status = mockSetupStatus(settings, demoModels());
    expect(status.ready).toBe(false);
    const byKey = Object.fromEntries(status.items.map((i) => [i.key, i]));
    expect(byKey.llamaServer).toMatchObject({ state: "missing", code: "setup_llama_server" });
    expect(byKey.llmModel).toMatchObject({ state: "missing", code: "setup_llm_model" });
    expect(byKey.vision).toMatchObject({ state: "missing", code: "setup_vision" });
    for (const key of ["llamaServer", "llmModel", "vision"]) {
      expect(byKey[key].fixHint).toBeTruthy();
    }
  });

  it("turns missing items into skipped ones when the user skips them", () => {
    const settings = {
      ...demoProcessingSettings(),
      llamaServerPath: "",
      describeScreenshots: false,
      skipReports: true,
      llm: { ...demoProcessingSettings().llm!, managed: { modelPath: "", ctx: 16384 } },
    };
    const byKey = Object.fromEntries(
      mockSetupStatus(settings, demoModels()).items.map((i) => [i.key, i.state]),
    );
    expect(byKey.vision).toBe("skipped");
    expect(byKey.llmModel).toBe("skipped");
    expect(byKey.llamaServer).toBe("skipped");
  });

  it("checks paths, remembers the first-run flag and never exposes a token", async () => {
    expect(mockCheckSetupPath("llmModel", "gemma.gguf")).toMatchObject({
      ok: false,
      code: "path_not_absolute",
    });
    expect(mockCheckSetupPath("llmModel", "D:/m/gemma.bin").code).toBe("path_wrong_kind");
    expect(mockCheckSetupPath("llmModel", "D:/m/gemma.gguf").ok).toBe(true);
    const b = createMockBackend({ clockRate: 0 });
    await b.setHfToken("hf_secret_value");
    await b.setFirstRunDone(false);
    expect((await b.getProcessingSettings()).firstRunDone).toBe(false);
    expect(JSON.stringify(await b.setupStatus())).not.toContain("hf_secret_value");
    await b.setFirstRunDone();
    expect((await b.getProcessingSettings()).firstRunDone).toBe(true);
  });
});
