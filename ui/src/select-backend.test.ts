import { describe, expect, it } from "vitest";
import { selectBackend } from "./select-backend";

describe("selectBackend: GitHub test switches of the mock", () => {
  const request = {
    mode: "auto" as const,
    id: "LUNAR",
    name: "Lunar",
  };
  const github = { name: "lunar", owner: null, confirmed: true };

  it("has a saved token and creates the repository by default", async () => {
    const backend = selectBackend("?backend=mock&rate=0");
    expect((await backend.githubRepoStatus()).tokenSet).toBe(true);
    const result = await backend.wizardCreateProjectWithGithub(request, github);
    expect(result.github).toMatchObject({ status: "created", repo: "demo-user/lunar" });
  });

  it("ghtoken=none starts without a token: a local-only project", async () => {
    const backend = selectBackend("?backend=mock&rate=0&ghtoken=none");
    expect((await backend.githubRepoStatus()).tokenSet).toBe(false);
    const result = await backend.wizardCreateProjectWithGithub(request, github);
    expect(result.github).toMatchObject({ status: "local_only", reason: "no_token" });
  });

  it.each(["name-taken", "offline"])("ghfault=%s makes the creation fail", async (fault) => {
    const backend = selectBackend(`?backend=mock&rate=0&ghfault=${fault}`);
    const result = await backend.wizardCreateProjectWithGithub(request, github);
    expect(result.github).toMatchObject({ status: "local_only", reason: "failed" });
  });

  it("ignores an unknown ghfault value", async () => {
    const backend = selectBackend("?backend=mock&rate=0&ghfault=boom");
    const result = await backend.wizardCreateProjectWithGithub(request, github);
    expect(result.github).toMatchObject({ status: "created" });
  });
});
