import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BackendProvider } from "../backend-context";
import type { Backend } from "../ipc/backend";
import { createMockBackend, type MockOptions } from "../ipc/mock";
import { planGithubRepo, ProjectWizard, suggestRepoName } from "./ProjectWizard";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let host: HTMLDivElement;
let root: Root;

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root.unmount());
  host.remove();
  vi.restoreAllMocks();
});

async function flush(rounds = 8) {
  for (let i = 0; i < rounds; i += 1) {
    await act(async () => {
      await Promise.resolve();
    });
  }
}

async function setInputValue(el: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
  await act(async () => {
    setter.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
  });
}

function findButton(text: string): HTMLButtonElement {
  const btn = Array.from(host.querySelectorAll<HTMLButtonElement>("button")).find((b) =>
    b.textContent?.includes(text),
  );
  if (!btn) throw new Error(`Button not found: ${text}`);
  return btn;
}

async function click(el: HTMLElement) {
  await act(async () => {
    el.click();
  });
  await flush();
}

function input(label: string): HTMLInputElement {
  const el = host.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`);
  if (!el) throw new Error(`Input not found: ${label}`);
  return el;
}

interface Opened {
  backend: Backend;
  onCreated: ReturnType<typeof vi.fn>;
}

/** Opens the wizard on step 2 of a new project "Lunar Scout", with the GitHub box ticked. */
async function openOnStepTwo(opts: MockOptions, tick = true): Promise<Opened> {
  const backend = createMockBackend({ clockRate: 0, ...opts });
  const onCreated = vi.fn();
  await act(async () => {
    root.render(
      <BackendProvider value={backend}>
        <ProjectWizard onClose={vi.fn()} onCreated={onCreated} />
      </BackendProvider>,
    );
  });
  await flush();
  await setInputValue(input("Project name"), "Lunar Scout");
  await click(findButton("Next"));
  if (tick) await click(input("Create a private GitHub repository"));
  return { backend, onCreated };
}

describe("project wizard: private GitHub repository", () => {
  it("with a saved token: explains, asks to confirm, and only then creates the repository", async () => {
    const { backend, onCreated } = await openOnStepTwo({});
    const withGithub = vi.spyOn(backend, "wizardCreateProjectWithGithub");
    const plain = vi.spyOn(backend, "wizardCreateProject");

    expect(host.textContent).toContain("A saved GitHub token was found");
    expect(host.textContent).toContain("Will create: lunar-scout (in your account), private.");

    await click(findButton("Create project"));
    // The confirm step explains what will be created and nothing has been sent yet.
    const confirm = host.querySelector('section[aria-label="Confirm the new GitHub repository"]');
    expect(confirm).not.toBeNull();
    expect(confirm!.textContent).toContain("lunar-scout (in your account)");
    expect(confirm!.textContent).toContain("Private");
    expect(confirm!.textContent).toContain("Your GitHub account");
    expect(confirm!.textContent).toContain("Nothing has been sent to GitHub yet");
    expect(withGithub).not.toHaveBeenCalled();
    expect(plain).not.toHaveBeenCalled();

    await click(findButton("Create repository & project"));
    expect(withGithub).toHaveBeenCalledTimes(1);
    expect(withGithub).toHaveBeenCalledWith(
      expect.objectContaining({ mode: "auto", id: "LUNAR-SCOUT", name: "Lunar Scout" }),
      { name: "lunar-scout", owner: null, confirmed: true },
    );
    const done = host.querySelector('section[aria-label="Project created"]');
    expect(done!.textContent).toContain(
      "Created the private GitHub repository demo-user/lunar-scout and linked it to the project.",
    );
    expect(onCreated).not.toHaveBeenCalled();
    await click(findButton("Open project"));
    expect(onCreated).toHaveBeenCalledWith("LUNAR-SCOUT");
    // The project is linked to the repository that was created.
    const repos = await backend.listCycleRepos();
    expect(repos.some((r) => r.githubRepo === "demo-user/lunar-scout")).toBe(true);
  });

  it("uses the owner/name field as an organization repository and shows it in the confirm step", async () => {
    const { backend } = await openOnStepTwo({});
    const withGithub = vi.spyOn(backend, "wizardCreateProjectWithGithub");
    await setInputValue(input("GitHub repository"), "demo-org/lunar-notes");
    await click(findButton("Create project"));
    expect(host.textContent).toContain("demo-org/lunar-notes");
    expect(host.textContent).toContain("demo-org (an organization");
    await click(findButton("Create repository & project"));
    expect(withGithub).toHaveBeenCalledWith(expect.anything(), {
      name: "lunar-notes",
      owner: "demo-org",
      confirmed: true,
    });
    expect(host.textContent).toContain(
      "Created the private GitHub repository demo-org/lunar-notes",
    );
  });

  it("refuses a malformed owner/name before the confirm step", async () => {
    const { backend } = await openOnStepTwo({});
    const withGithub = vi.spyOn(backend, "wizardCreateProjectWithGithub");
    await setInputValue(input("GitHub repository"), "not a repo");
    expect(host.textContent).toContain("Write the GitHub repository as owner/name");
    await click(findButton("Create project"));
    expect(
      host.querySelector('section[aria-label="Confirm the new GitHub repository"]'),
    ).toBeNull();
    expect(withGithub).not.toHaveBeenCalled();
  });

  it("can fall back to a local-only project from the confirm step without any GitHub call", async () => {
    const { backend, onCreated } = await openOnStepTwo({});
    const withGithub = vi.spyOn(backend, "wizardCreateProjectWithGithub");
    await click(findButton("Create project"));
    await click(findButton("Create local project only"));
    expect(withGithub).not.toHaveBeenCalled();
    expect(onCreated).toHaveBeenCalledWith("LUNAR-SCOUT");
  });

  it("without a saved token: says so, sends nothing and creates a local-only project with a message", async () => {
    const { backend, onCreated } = await openOnStepTwo({ noGithubToken: true });
    expect((await backend.githubRepoStatus()).tokenSet).toBe(false);
    expect(host.textContent).toContain("No GitHub token is saved yet");

    const withGithub = vi.spyOn(backend, "wizardCreateProjectWithGithub");
    await click(findButton("Create project"));
    // No token: there is nothing to confirm, so no confirm step.
    expect(
      host.querySelector('section[aria-label="Confirm the new GitHub repository"]'),
    ).toBeNull();
    expect(withGithub).toHaveBeenCalledTimes(1);
    const status = host.querySelector('[role="status"]');
    expect(status!.textContent).toContain("No GitHub token is saved");
    expect(status!.textContent).toContain("Settings > Connectors");
    expect(status!.className).toContain("notice-warn");
    const result = await withGithub.mock.results[0].value;
    expect(result.github).toMatchObject({ status: "local_only", reason: "no_token", repo: null });
    // The project exists even though no repository was created.
    await click(findButton("Open project"));
    expect(onCreated).toHaveBeenCalledWith("LUNAR-SCOUT");
    expect(
      (await backend.listCycleRepos()).some((r) => (r.githubRepo ?? "").includes("lunar-scout")),
    ).toBe(false);
  });

  it.each([
    ["name-taken" as const, "already has a repository named lunar-scout"],
    ["offline" as const, "GitHub could not be reached"],
  ])("when GitHub fails (%s): falls back to a local project and shows why", async (fault, text) => {
    const { backend, onCreated } = await openOnStepTwo({ githubCreateFault: fault });
    const withGithub = vi.spyOn(backend, "wizardCreateProjectWithGithub");
    await click(findButton("Create project"));
    await click(findButton("Create repository & project"));
    expect(withGithub).toHaveBeenCalledTimes(1);
    const status = host.querySelector('[role="status"]');
    expect(status!.textContent).toContain(text);
    expect(status!.textContent).toContain("Only a local project folder was created");
    expect(host.querySelector('[role="alert"]')).toBeNull();
    await click(findButton("Open project"));
    expect(onCreated).toHaveBeenCalledWith("LUNAR-SCOUT");
    // No link to a repository that was never created.
    expect(
      (await backend.listCycleRepos()).some((r) => (r.githubRepo ?? "").includes("lunar-scout")),
    ).toBe(false);
  });

  it("keeps the plain flow when the box is not ticked and Discord stays a manual field", async () => {
    const { backend, onCreated } = await openOnStepTwo({}, false);
    const withGithub = vi.spyOn(backend, "wizardCreateProjectWithGithub");
    const plain = vi.spyOn(backend, "wizardCreateProject");
    expect(host.textContent).toContain("Discord is never set up automatically");
    await setInputValue(input("Discord webhook reference"), "cred:discord/lunar");
    await click(findButton("Create project"));
    expect(withGithub).not.toHaveBeenCalled();
    expect(plain).toHaveBeenCalledWith(
      expect.objectContaining({ discordWebhookRef: "cred:discord/lunar", githubRepo: null }),
    );
    expect(onCreated).toHaveBeenCalledWith("LUNAR-SCOUT");
  });
});

describe("repository plan", () => {
  it("proposes a valid name from the project id", () => {
    expect(suggestRepoName("NOMAD-ROVER")).toBe("nomad-rover");
    expect(suggestRepoName("  A b/c  ")).toBe("a-b-c");
    expect(suggestRepoName("---")).toBe("meeting-notes");
  });

  it("reads owner/name or falls back to the id", () => {
    expect(planGithubRepo("", "NOMAD")).toEqual({ owner: null, name: "nomad" });
    expect(planGithubRepo(" org/notes ", "NOMAD")).toEqual({ owner: "org", name: "notes" });
    for (const bad of ["notes", "a/b/c", "a/", "/b", "a/..", "a/b.git", "a b/c"]) {
      expect(planGithubRepo(bad, "NOMAD"), bad).toHaveProperty("error");
    }
  });
});

describe("mock backend: repository creation", () => {
  it("is deterministic and never creates the same repository twice", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const req = {
      mode: "auto" as const,
      id: "ALPHA",
      name: "Alpha",
    };
    const github = { name: "alpha", owner: null, confirmed: true };
    const first = await backend.wizardCreateProjectWithGithub(req, github);
    expect(first.github).toMatchObject({ status: "created", repo: "demo-user/alpha" });
    const second = await backend.wizardCreateProjectWithGithub(
      { ...req, id: "BETA", name: "Beta" },
      github,
    );
    expect(second.github).toMatchObject({ status: "local_only", reason: "failed" });
    const unconfirmed = await backend.wizardCreateProjectWithGithub(
      { ...req, id: "GAMMA", name: "Gamma" },
      { name: "gamma", confirmed: false },
    );
    expect(unconfirmed.github).toMatchObject({ status: "local_only", reason: "not_confirmed" });
    await expect(
      backend.wizardCreateProjectWithGithub({ ...req, mode: "existing" }, github),
    ).rejects.toMatchObject({ code: "invalid" });
  });
});
