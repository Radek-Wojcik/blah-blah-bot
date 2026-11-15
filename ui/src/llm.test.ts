import { describe, expect, it } from "vitest";
import type { LlmSettings } from "./ipc/types";
import {
  backendDetail,
  baseName,
  isConfigured,
  missingForKind,
  skipText,
  urlHost,
  validateLlmSettings,
} from "./llm";

const settings = (patch: Partial<LlmSettings> = {}): LlmSettings => ({
  defaultKind: "managed",
  managed: { modelPath: "D:\\Models\\gemma-4-12b-q4_k_m.gguf", ctx: 16384 },
  external: { baseUrl: "http://127.0.0.1:11434/v1", model: "gemma4:12b" },
  cloud: { baseUrl: "https://api.llm-vendor.example/v1", model: "large-model-1" },
  externalKeySet: false,
  cloudKeySet: false,
  ...patch,
});

describe("urlHost", () => {
  it("takes the host of an http address, without port, path and credentials", () => {
    expect(urlHost("https://api.llm-vendor.example/v1")).toBe("api.llm-vendor.example");
    expect(urlHost("http://127.0.0.1:11434/v1")).toBe("127.0.0.1");
    expect(urlHost("https://user@host.example/x")).toBe("host.example");
    expect(urlHost("ftp://host.example")).toBeNull();
    expect(urlHost("https://")).toBeNull();
  });
});

describe("validateLlmSettings", () => {
  it("accepts the demo settings", () => {
    expect(validateLlmSettings(settings())).toBeNull();
  });

  it("keeps the context within what the worker accepts", () => {
    const bad = (ctx: number) => settings({ managed: { modelPath: "m.gguf", ctx } });
    expect(validateLlmSettings(bad(1024))).toMatch(/between 2048/);
    expect(validateLlmSettings(bad(2048))).toBeNull();
    expect(validateLlmSettings(bad(2_000_000))).toMatch(/between/);
    expect(validateLlmSettings(bad(4096.5))).toMatch(/between/);
  });

  it("refuses an address that is not http(s), has no host or carries credentials", () => {
    const ext = (baseUrl: string) => settings({ external: { baseUrl, model: "m" } });
    expect(validateLlmSettings(ext(""))).toBeNull();
    expect(validateLlmSettings(ext("localhost:11434"))).toMatch(/http:\/\/ or https:\/\//);
    expect(validateLlmSettings(ext("https://key@host.example/v1"))).toMatch(
      /do not put a user name/,
    );
    expect(validateLlmSettings(ext("https:///v1"))).toMatch(/no host/);
    const cloud = settings({ cloud: { baseUrl: "ws://x", model: "m" } });
    expect(validateLlmSettings(cloud)).toMatch(/^Cloud endpoint/);
  });
});

describe("what is configured", () => {
  it("names what is missing", () => {
    expect(missingForKind(settings(), "managed")).toBeNull();
    expect(missingForKind(settings({ managed: { modelPath: " ", ctx: 4096 } }), "managed")).toBe(
      "model file",
    );
    expect(missingForKind(settings({ external: { baseUrl: "", model: "m" } }), "external")).toBe(
      "address",
    );
    expect(
      missingForKind(settings({ cloud: { baseUrl: "https://x/v1", model: "" } }), "cloud"),
    ).toBe("model name");
    expect(isConfigured(settings(), "cloud")).toBe(true);
  });

  it("describes a backend by the model it uses", () => {
    expect(backendDetail(settings(), "managed")).toBe("gemma-4-12b-q4_k_m.gguf");
    expect(backendDetail(settings(), "external")).toBe("gemma4:12b");
    expect(backendDetail(settings(), "cloud")).toBe("large-model-1 at api.llm-vendor.example");
    expect(backendDetail(settings({ managed: { modelPath: "", ctx: 4096 } }), "managed")).toBe(
      "not configured (no model file)",
    );
  });

  it("takes the file name from a path of either kind", () => {
    expect(baseName("C:\\a\\b\\m.gguf")).toBe("m.gguf");
    expect(baseName("/a/b/m.gguf")).toBe("m.gguf");
    expect(baseName("m.gguf")).toBe("m.gguf");
  });

  it("writes the skip note of a stage", () => {
    expect(skipText("the meeting has no cycle")).toBe("Skipped: the meeting has no cycle");
  });
});
