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

describe("when to publish", () => {
  function group(label: string): HTMLElement {
    const g = host.querySelector<HTMLElement>(`[role="radiogroup"][aria-label="${label}"]`);
    if (!g) throw new Error(`no switch ${label}`);
    return g;
  }
  const checked = (g: HTMLElement) =>
    [...g.querySelectorAll('[role="radio"]')]
      .filter((r) => r.getAttribute("aria-checked") === "true")
      .map((r) => r.textContent);
  const radio = (g: HTMLElement, name: string) =>
    [...g.querySelectorAll<HTMLElement>('[role="radio"]')].find((r) => r.textContent === name)!;

  it("shows the rule of each target of each usable cycle as a switch and says what no rule changes", async () => {
    await mount(createMockBackend({ clockRate: 0 }));
    expect(host.textContent).toContain("When to publish");
    expect(host.textContent).toContain("never written back automatically");
    const github = group("GitHub, cycle Nomad rover publishing rule");
    expect([...github.querySelectorAll('[role="radio"]')].map((r) => r.textContent)).toEqual([
      "Off",
      "On approval",
      "Automatic",
    ]);
    expect(checked(github)).toEqual(["On approval"]);
    expect(checked(group("Discord, cycle Nomad rover publishing rule"))).toEqual(["On approval"]);
  });

  it("saves a change at once through the rule command and warns under Automatic", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const set = vi.spyOn(backend, "setTargetRule");
    await mount(backend);
    const github = () => group("GitHub, cycle Nomad rover publishing rule");
    const warning = () => host.querySelector(".target-rule-warn")?.textContent ?? null;
    // Every rule is explained, whichever is chosen; the warning only comes with Automatic.
    expect(host.textContent).toContain("Nothing is ever published to this target");
    expect(host.textContent).toContain("This is what is used until you choose another rule.");
    expect(warning()).toBeNull();
    await click(radio(github(), "Automatic"));
    expect(set).toHaveBeenCalledWith("NOMAD", "github", "auto");
    expect(checked(github())).toEqual(["Automatic"]);
    expect(warning()).toContain("cannot be taken back");
    expect((await backend.getCycle("NOMAD")).cycle.targets.github?.publish).toBe("auto");
    await click(radio(github(), "Off"));
    expect(checked(github())).toEqual(["Off"]);
    expect(warning()).toBeNull();
  });

  it("moves with the arrow keys like a radio group", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    await mount(backend);
    const github = () => group("GitHub, cycle Nomad rover publishing rule");
    await act(async () => {
      radio(github(), "On approval").dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }),
      );
    });
    await flush();
    expect(checked(github())).toEqual(["Automatic"]);
    expect(radio(github(), "Automatic").tabIndex).toBe(0);
    expect(radio(github(), "Off").tabIndex).toBe(-1);
    // It wraps around.
    await act(async () => {
      radio(github(), "Automatic").dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }),
      );
    });
    await flush();
    expect(checked(github())).toEqual(["Off"]);
  });

  it("offers no Discord switch for a cycle without a Discord target", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    await backend.createCycle("PLAIN", "Plain cycle");
    await mount(backend);
    expect(
      host.querySelector('[aria-label="Discord, cycle Plain cycle publishing rule"]'),
    ).toBeNull();
    expect(host.textContent).toContain("No Discord target");
    expect(host.textContent).toContain("No Notion target");
    expect(checked(group("GitHub, cycle Plain cycle publishing rule"))).toEqual(["On approval"]);
  });

  it("shows Discord bot, Notion integration token, invite link and Notion rule when configured", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const cycle = (await backend.getCycle("NOMAD")).cycle;
    cycle.targets.discord = {
      bot: {
        token: "cred:discord-bot/nomad",
        guild: "111111111111111111",
        channel: "222222222222222222",
        thread: true,
        editOnCorrection: true,
      },
      publish: "on-approval",
    };
    cycle.targets.notion = {
      token: "cred:notion/nomad",
      database: "0123456789abcdef0123456789abcdef",
      publish: "on-approval",
      properties: { title: "Name", key: "BBB key" },
    };
    await backend.saveCycle(cycle);
    await mount(backend);

    expect(host.textContent).toContain("Bot token for cred:discord-bot/nomad");
    expect(host.textContent).toContain("Notion token for cred:notion/nomad");
    expect(host.textContent).toContain("309237746688");
    expect(host.textContent).toContain("0 privileged intents required");

    const appInput = input("Discord Application ID (for invite link)");
    await type(appInput, "123456789012345678");
    expect(host.textContent).toContain(
      "https://discord.com/oauth2/authorize?client_id=123456789012345678&scope=bot&permissions=309237746688",
    );

    const botToken = [
      "MTIzNDU2Nzg5MDEyMzQ1Njc4",
      "Gdemo1",
      "DemoBotSecretSignature0123456789abcdef",
    ].join(".");
    await type(input("Bot token for cred:discord-bot/nomad"), botToken);
    await submit("Bot token for cred:discord-bot/nomad");
    expect(host.textContent).toContain("Bot token stored: bot MTIz...");
    expect(host.innerHTML).not.toContain("DemoBotSecretSignature");

    const notionToken = ["ntn", "demoSecretIntegrationToken0123456789"].join("_");
    await type(input("Notion token for cred:notion/nomad"), notionToken);
    await submit("Notion token for cred:notion/nomad");
    expect(host.textContent).toContain("Token stored: ntn_...6789");
    expect(host.innerHTML).not.toContain("demoSecretIntegrationToken");

    const notionRule = group("Notion, cycle Nomad rover publishing rule");
    expect(checked(notionRule)).toEqual(["On approval"]);
  });
});
