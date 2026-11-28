import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BackendProvider } from "../../backend-context";
import type { Backend } from "../../ipc/backend";
import { createMockBackend } from "../../ipc/mock";
import { SettingsView, type SettingsTab } from "../Settings";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const TOKEN = "github_pat_DemoSecret0123456789";

let host: HTMLDivElement;
let root: Root;

async function mount(backend: Backend, tab: SettingsTab) {
  await act(async () => {
    root.render(
      <BackendProvider value={backend}>
        <SettingsView initialTab={tab} />
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

const GH = "GitHub Token (api.github.com)";

describe("token setup (Connectors tab)", () => {
  it("lists the GitHub host with its state", async () => {
    await mount(createMockBackend({ clockRate: 0 }), "connectors");
    expect(host.querySelector(`form[aria-label="${GH}"]`)).not.toBeNull();
    expect(host.textContent).toContain("Token stored");
  });

  it("masks the field and keeps the token out of the page and of the state after storing it", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const store = vi.spyOn(backend, "setGithubToken");
    await mount(backend, "connectors");
    const field = input(GH);
    expect(field.type).toBe("password");
    expect(field.autocomplete).toBe("off");
    await type(field, TOKEN);
    // The typed value is in the field only; it is not written into the page as text.
    expect(host.textContent).not.toContain(TOKEN);
    expect(host.innerHTML).not.toContain(`>${TOKEN}<`);
    await submit(GH);
    expect(store).toHaveBeenCalledWith(TOKEN, undefined);
    expect(input(GH).value).toBe("");
    expect(host.innerHTML).not.toContain(TOKEN);
    expect(host.querySelector(`form[aria-label="${GH}"] [role="status"]`)?.textContent).toBe(
      "Stored.",
    );
  });

  it("never logs the token", async () => {
    const spies = (["log", "info", "warn", "error", "debug"] as const).map((m) =>
      vi.spyOn(console, m).mockImplementation(() => undefined),
    );
    await mount(createMockBackend({ clockRate: 0 }), "connectors");
    await type(input(GH), TOKEN);
    await submit(GH);
    for (const spy of spies)
      for (const call of spy.mock.calls) expect(JSON.stringify(call)).not.toContain(TOKEN);
  });

  it("refuses a token with spaces before sending it, without repeating it", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const store = vi.spyOn(backend, "setGithubToken");
    await mount(backend, "connectors");
    await type(input(GH), "two words");
    await submit(GH);
    expect(store).not.toHaveBeenCalled();
    const alert = host.querySelector('[role="alert"]')!;
    expect(alert.textContent).toMatch(/spaces or line breaks/);
    expect(alert.textContent).not.toContain("two words");
  });

  it("clears a token", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    await mount(backend, "connectors");
    await click(button(GH, "Clear"));
    expect(host.textContent).toContain("No token stored");
    expect((await backend.publishingSecrets()).github[0].tokenSet).toBe(false);
  });
});

describe("when to publish (Preferences tab)", () => {
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

  it("shows the rule of each target of each usable project as a switch and says what no rule changes", async () => {
    await mount(createMockBackend({ clockRate: 0 }), "preferences");
    expect(host.textContent).toContain("When to publish");
    expect(host.textContent).toContain("never written back automatically");
    const github = group("GitHub, project Nomad rover publishing rule");
    expect([...github.querySelectorAll('[role="radio"]')].map((r) => r.textContent)).toEqual([
      "Off",
      "On approval",
      "Automatic",
    ]);
    expect(checked(github)).toEqual(["On approval"]);
    expect(checked(group("Discord, project Nomad rover publishing rule"))).toEqual(["On approval"]);
  });

  it("saves a change at once through the rule command and warns under Automatic", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const set = vi.spyOn(backend, "setTargetRule");
    await mount(backend, "preferences");
    const github = () => group("GitHub, project Nomad rover publishing rule");
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
    await mount(backend, "preferences");
    const github = () => group("GitHub, project Nomad rover publishing rule");
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

  it("explains that the rules are global defaults and shows the global rules as switches", async () => {
    await mount(createMockBackend({ clockRate: 0 }), "preferences");
    expect(host.textContent).toContain("This is the default for every project");
    expect(host.textContent).toContain("Use global setting");
    const global = group("GitHub, all projects publishing rule");
    expect(checked(global)).toEqual(["On approval"]);
    expect(checked(group("Notion, all projects publishing rule"))).toEqual(["On approval"]);
  });

  it("a global change is saved at once and moves every project that follows it", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const set = vi.spyOn(backend, "setGlobalPublishRule");
    await mount(backend, "preferences");
    await click(radio(group("GitHub, all projects publishing rule"), "Automatic"));
    expect(set).toHaveBeenCalledWith("github", "auto");
    expect((await backend.getSettings()).publishRules?.github).toBe("auto");
    // Nomad follows the global GitHub rule: its switch shows the new rule, marked as global.
    const project = group("GitHub, project Nomad rover publishing rule");
    expect(checked(project)).toEqual(["Automatic"]);
    expect(host.textContent).toContain("Use global setting (currently: Automatic)");
    // Its Discord rule is its own, so the global Discord rule does not move it.
    await click(radio(group("Discord, all projects publishing rule"), "Off"));
    expect(checked(group("Discord, project Nomad rover publishing rule"))).toEqual(["On approval"]);
  });

  it("a project overrides the global rule and can go back to it", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const set = vi.spyOn(backend, "setTargetRule");
    await mount(backend, "preferences");
    const box = (name: string) =>
      host.querySelector<HTMLInputElement>(`input[aria-label="${name}: use global setting"]`)!;
    const github = () => group("GitHub, project Nomad rover publishing rule");
    expect(box("GitHub, project Nomad rover").checked).toBe(true);
    await click(radio(github(), "Off"));
    expect(set).toHaveBeenLastCalledWith("NOMAD", "github", "off");
    expect(box("GitHub, project Nomad rover").checked).toBe(false);
    expect(host.textContent).toContain("set for this project only");
    // Discord of this project has its own rule from the start.
    expect(box("Discord, project Nomad rover").checked).toBe(false);
    await click(box("GitHub, project Nomad rover"));
    expect(set).toHaveBeenLastCalledWith("NOMAD", "github", null);
    expect(box("GitHub, project Nomad rover").checked).toBe(true);
    expect(checked(github())).toEqual(["On approval"]);
    expect((await backend.getCycle("NOMAD")).cycle.targets.github?.publish).toBeUndefined();
  });

  it("offers no Discord switch for a project without a Discord target", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    await backend.createCycle("PLAIN", "Plain project");
    await mount(backend, "preferences");
    expect(
      host.querySelector('[aria-label="Discord, project Plain project publishing rule"]'),
    ).toBeNull();
    expect(host.textContent).toContain("No Discord target");
    expect(checked(group("GitHub, project Plain project publishing rule"))).toEqual([
      "On approval",
    ]);
  });
});
