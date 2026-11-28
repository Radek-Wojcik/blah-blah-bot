import { describe, expect, it } from "vitest";
import { cloudKeyPage, GITHUB_PERMISSIONS, GITHUB_TOKEN_PAGE, HF_TOKEN_PAGE } from "./tokenPages";

describe("token pages", () => {
  it("are https pages of the providers", () => {
    expect(GITHUB_TOKEN_PAGE).toBe("https://github.com/settings/personal-access-tokens/new");
    expect(HF_TOKEN_PAGE).toBe("https://huggingface.co/settings/tokens");
  });

  it("list the permissions a GitHub token needs", () => {
    const text = GITHUB_PERMISSIONS.join(" ");
    for (const p of ["Administration", "Contents", "Issues"]) expect(text).toContain(p);
  });

  it("knows the key page of common cloud hosts and nothing else", () => {
    expect(cloudKeyPage("https://api.openai.com/v1")).toBe("https://platform.openai.com/api-keys");
    expect(cloudKeyPage(" https://OpenRouter.ai/api/v1 ")).toBe("https://openrouter.ai/keys");
    expect(cloudKeyPage("https://api.example.com/v1")).toBeNull();
    expect(cloudKeyPage("not a url")).toBeNull();
    expect(cloudKeyPage("")).toBeNull();
  });
});
