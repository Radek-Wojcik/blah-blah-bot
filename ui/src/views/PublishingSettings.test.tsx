import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BackendProvider } from "../backend-context";
import type { Backend } from "../ipc/backend";
import { createMockBackend } from "../ipc/mock";
import { PublishingSettingsView } from "./PublishingSettings";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const TOKEN = "github_pat_DemoSecret0123456789";
const WEBHOOK = "https://discord.com/api/webhooks/000000000000000042/demo-secret-token";

let host: HTMLDivElement;
let root: Root;

async function mount(backend: Backend) {
  await act(async () => {
    root.render(
      <BackendProvider value={backend}>
        <PublishingSettingsView />
      </BackendProvider>,
    );
  });
  await flush();
}

/** Lets the promises of the mock backend settle and React render their results. */
async function flush() {
  for (let i = 0; i < 4; i += 1) await act(async () => void (await Promise.resolve()));
}

function input(label: string): HTMLInputElement {
  const field = [...host.querySelectorAll<HTMLLabelElement>("label")]
    .find((l) => l.textContent?.startsWith(label))
    ?.querySelector("input");
  if (!field) throw new Error(`no field ${label}`);
  return field;
}

async function type(el: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
  await act(async () => {
    setter.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

function button(form: string, name: string): HTMLButtonElement {
  const f = host.querySelector(`form[aria-label="${form}"]`);
  const b = [...(f?.querySelectorAll("button") ?? [])].find((x) => x.textContent === name);
  if (!b) throw new Error(`no button ${name} in ${form}`);
  return b;
}

async function click(el: HTMLElement) {
  await act(async () => {
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
  await flush();
}

async function submit(form: string) {
  const f = host.querySelector(`form[aria-label="${form}"]`)!;
  await act(async () => {
    f.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  });
  await flush();
}

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

describe("token setup", () => {
  it("lists the GitHub host, the repository link and the Discord webhook with their state", async () => {
    await mount(createMockBackend({ clockRate: 0 }));
    expect(host.textContent).toContain("GitHub token for api.github.com");
    expect(host.textContent).toContain("Token stored");
    expect(host.textContent).toContain("Webhook stored: discord.com webhook 0000...0007");
    expect(host.querySelector<HTMLInputElement>('input[placeholder="owner/name"]')).not.toBeNull();
  });

  it("masks the field and keeps the token out of the page and of the state after storing it", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const store = vi.spyOn(backend, "setGithubToken");
    await mount(backend);
    const field = input("GitHub token for api.github.com");
    expect(field.type).toBe("password");
    expect(field.autocomplete).toBe("off");
    await type(field, TOKEN);
    // The typed value is in the field only; it is not written into the page as text.
    expect(host.textContent).not.toContain(TOKEN);
    expect(host.innerHTML).not.toContain(`>${TOKEN}<`);
    await submit("GitHub token for api.github.com");
    expect(store).toHaveBeenCalledWith(TOKEN, undefined);
    expect(input("GitHub token for api.github.com").value).toBe("");
    expect(host.innerHTML).not.toContain(TOKEN);
    expect(host.querySelector('[role="status"]')?.textContent).toBe("Stored.");
  });

  it("never logs the token", async () => {
    const spies = (["log", "info", "warn", "error", "debug"] as const).map((m) =>
      vi.spyOn(console, m).mockImplementation(() => undefined),
    );
    await mount(createMockBackend({ clockRate: 0 }));
    await type(input("GitHub token for api.github.com"), TOKEN);
    await submit("GitHub token for api.github.com");
    for (const spy of spies)
      for (const call of spy.mock.calls) expect(JSON.stringify(call)).not.toContain(TOKEN);
  });

  it("refuses a token with spaces before sending it, without repeating it", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const store = vi.spyOn(backend, "setGithubToken");
    await mount(backend);
    await type(input("GitHub token for api.github.com"), "two words");
    await submit("GitHub token for api.github.com");
    expect(store).not.toHaveBeenCalled();
    const alert = host.querySelector('[role="alert"]')!;
    expect(alert.textContent).toMatch(/spaces or line breaks/);
    expect(alert.textContent).not.toContain("two words");
  });

  it("stores a webhook, shows it masked, and refuses one that is not Discord's", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    await backend.clearTargetSecret("cred:discord/nomad");
    await mount(backend);
    expect(host.textContent).toContain("No webhook stored (used by the cycle NOMAD)");
    const form = "Webhook for cred:discord/nomad";
    const field = input(form);
    await type(field, "https://example.org/api/webhooks/000000000000000042/leaky-secret");
    await submit(form);
    const alert = host.querySelector('[role="alert"]')!;
    expect(alert.textContent).toMatch(/not a Discord address/);
    expect(host.textContent).not.toContain("leaky-secret");
    // A refused value stays in the field so it can be corrected; it is not part of the page.
    await type(input(form), WEBHOOK);
    await submit(form);
    expect(input(form).value).toBe("");
    expect(host.textContent).toContain("Webhook stored: discord.com webhook 0000...0042");
    expect(host.innerHTML).not.toContain("demo-secret-token");
    expect((await backend.publishingSecrets()).targets[0].masked).toBe(
      "discord.com webhook 0000...0042",
    );
  });

  it("clears a token and tests the webhook", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    await mount(backend);
    await click(button("Webhook for cred:discord/nomad", "Test webhook"));
    expect(
      host.querySelector('form[aria-label="Webhook for cred:discord/nomad"] [role="status"]')
        ?.textContent,
    ).toBe("The webhook exists.");
    await click(button("GitHub token for api.github.com", "Clear"));
    expect(host.textContent).toContain("No token stored");
    expect((await backend.publishingSecrets()).github[0].tokenSet).toBe(false);
  });

  it("links a repository to GitHub and tests the access", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    await mount(backend);
    const form = "GitHub repository of local";
    await type(input("GitHub repository of local"), "demo-org/sandbox");
    await submit(form);
    expect(host.querySelector(`form[aria-label="${form}"] [role="status"]`)?.textContent).toBe(
      "Saved.",
    );
    expect((await backend.listCycleRepos()).find((r) => r.id === "local")?.githubRepo).toBe(
      "demo-org/sandbox",
    );
    await click(button("GitHub repository of team-cycles", "Test access"));
    expect(
      host.querySelector('form[aria-label="GitHub repository of team-cycles"] [role="status"]')
        ?.textContent,
    ).toBe("Connected to demo-org/nomad-cycles as demo-bot");
  });
});
