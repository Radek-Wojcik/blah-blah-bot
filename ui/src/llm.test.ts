import { describe, expect, it } from "vitest";
import type { LlmSettings } from "./ipc/types";
import {
  backendDetail,
  baseName,
  isConfigured,
  isLocalHost,
  isLoopbackHost,
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
  externalHostIsRemote: false,
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

describe("the cloud address", () => {
  const cloud = (baseUrl: string) => settings({ cloud: { baseUrl, model: "m" } });

  it("needs https unless it is this machine", () => {
    expect(validateLlmSettings(cloud("http://api.llm-vendor.example/v1"))).toMatch(/https:\/\//);
    expect(validateLlmSettings(cloud("http://localhost:8080/v1"))).toBeNull();
    expect(validateLlmSettings(cloud("http://127.0.0.1:8080/v1"))).toBeNull();
    expect(validateLlmSettings(cloud("https://api.llm-vendor.example/v1"))).toBeNull();
    // The external server may use plain http on the network.
    const ext = settings({ external: { baseUrl: "http://192.168.1.20:8000/v1", model: "m" } });
    expect(validateLlmSettings(ext)).toBeNull();
  });

  it("refuses a token used as the user name", () => {
    expect(validateLlmSettings(cloud("https://sk-123@api.llm-vendor.example/v1"))).toMatch(
      /do not put a user name/,
    );
  });
});

describe("local and remote hosts", () => {
  it("tells this machine and private networks from the internet", () => {
    for (const h of [
      "localhost",
      "127.0.0.1",
      "10.0.0.5",
      "172.16.1.1",
      "172.31.0.1",
      "192.168.1.20",
      "169.254.3.4",
      "fd12::1",
      "fe80::1",
      "gpu-box",
      "nas.local",
    ])
      expect(isLocalHost(h), h).toBe(true);
    for (const h of ["api.openai.com", "8.8.8.8", "172.32.0.1", "2001:db8::1"])
      expect(isLocalHost(h), h).toBe(false);
    expect(isLoopbackHost("127.0.0.2")).toBe(true);
    expect(isLoopbackHost("192.168.1.20")).toBe(false);
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
