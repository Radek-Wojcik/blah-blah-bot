import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BackendProvider } from "../../backend-context";
import type { Backend } from "../../ipc/backend";
import { createMockBackend } from "../../ipc/mock";
import type { MockHooks } from "../../ipc/mock-publish";
import { SettingsView } from "../Settings";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let host: HTMLDivElement;
let root: Root;

const hooks = () => (globalThis as { __bbbMock?: MockHooks }).__bbbMock!;

async function flush() {
  for (let i = 0; i < 4; i += 1) await act(async () => void (await Promise.resolve()));
}

async function mount(backend: Backend) {
  await act(async () => {
    root.render(
      <BackendProvider value={backend}>
        <SettingsView initialTab="connectors" />
      </BackendProvider>,
    );
  });
  await flush();
}

function button(name: string): HTMLButtonElement {
  const b = [...host.querySelectorAll("button")].find((x) => x.textContent === name);
  if (!b) throw new Error(`no button ${name}`);
  return b;
}

async function click(el: HTMLElement) {
  await act(async () => {
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
  await flush();
}

/** Lets the timer of the poll fire and its promises settle. */
async function tick(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
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
  vi.useRealTimers();
  vi.restoreAllMocks();
  delete (globalThis as { __bbbMock?: MockHooks }).__bbbMock;
});

describe("Connect account (GitHub device flow)", () => {
  it("says it is not configured, disables the button and leaves the token field", async () => {
    await mount(createMockBackend({ clockRate: 0 }));
    expect(host.querySelector('[data-testid="github-connect-unconfigured"]')).toBeNull();
    act(() => root.unmount());
    root = createRoot(host);
    const backend = createMockBackend({ clockRate: 0 });
    hooks().setGithubConnect({ configured: false });
    await mount(backend);
    expect(host.querySelector('[data-testid="github-connect-unconfigured"]')?.textContent).toMatch(
      /not configured.*token/i,
    );
    expect(button("Connect account").disabled).toBe(true);
    expect(host.querySelector('form[aria-label="GitHub Token (api.github.com)"]')).not.toBeNull();
  });

  it("shows the code, opens only an https github page, polls and reports who connected", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const poll = vi.spyOn(backend, "githubDevicePoll");
    await mount(backend);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    await click(button("Connect account"));
    expect(host.querySelector('[data-testid="github-device-code"]')?.textContent).toBe("WDJB-MJHT");
    const link = [...host.querySelectorAll("a")].find((a) =>
      a.textContent?.includes("github.com/login/device"),
    )!;
    expect(link.getAttribute("href")).toBe("https://github.com/login/device");
    expect(link.target).toBe("_blank");
    expect(link.rel).toContain("noopener");
    expect(poll).not.toHaveBeenCalled();
    await tick(1000);
    expect(poll).toHaveBeenCalledTimes(1);
    expect(host.textContent).toContain("Waiting for approval");
    await tick(1000);
    expect(poll).toHaveBeenCalledTimes(2);
    expect(host.querySelector('[data-testid="github-connected"]')?.textContent).toBe(
      "Connected as demo-user",
    );
    // No more polling after the connection.
    await tick(5000);
    expect(poll).toHaveBeenCalledTimes(2);
  });

  it("slows down when told to, and reports denial and expiry", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const poll = vi.spyOn(backend, "githubDevicePoll");
    await mount(backend);
    hooks().setGithubConnect({
      script: [{ state: "slowDown", interval: 4 }, { state: "denied" }],
    });
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    await click(button("Connect account"));
    await tick(1000);
    expect(poll).toHaveBeenCalledTimes(1);
    await tick(3000);
    expect(poll).toHaveBeenCalledTimes(1);
    await tick(1000);
    expect(poll).toHaveBeenCalledTimes(2);
    expect(host.querySelector('[role="alert"]')?.textContent).toMatch(/denied/i);
    expect(host.querySelector('[data-testid="github-device-code"]')).toBeNull();

    hooks().setGithubConnect({ script: [{ state: "expired" }] });
    await click(button("Connect account"));
    await tick(1000);
    expect(host.querySelector('[role="alert"]')?.textContent).toMatch(/expired/i);
  });

  it("cancels, stops polling and forgets the connection", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const poll = vi.spyOn(backend, "githubDevicePoll");
    const cancel = vi.spyOn(backend, "githubDeviceCancel");
    await mount(backend);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    await click(button("Connect account"));
    await click(button("Cancel"));
    await tick(5000);
    expect(cancel).toHaveBeenCalled();
    expect(poll).not.toHaveBeenCalled();
    expect(host.querySelector('[data-testid="github-device-code"]')).toBeNull();
  });

  it("disconnects by removing the stored token", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const clear = vi.spyOn(backend, "clearGithubToken");
    await mount(backend);
    await click(button("Disconnect"));
    expect(clear).toHaveBeenCalledWith();
    expect((await backend.publishingSecrets()).github[0].tokenSet).toBe(false);
    expect(host.textContent).toContain("Disconnected");
  });

  it("never puts a token in what the backend returns", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const start = await backend.githubDeviceStart();
    const first = await backend.githubDevicePoll();
    const second = await backend.githubDevicePoll();
    for (const value of [start, first, second])
      expect(JSON.stringify(value)).not.toMatch(/gh[op]_|access_token|device_code/);
    expect(second).toEqual({ state: "connected", login: "demo-user" });
    await expect(backend.githubDevicePoll()).rejects.toMatchObject({ code: "not_found" });
  });

  it("refuses to start without a client id", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    hooks().setGithubConnect({ configured: false });
    expect(await backend.githubConnectStatus()).toEqual({ configured: false });
    await expect(backend.githubDeviceStart()).rejects.toMatchObject({ code: "not_configured" });
  });
});

describe("Open token page buttons", () => {
  it("every connector with a token has one that opens an https provider page", async () => {
    await mount(createMockBackend({ clockRate: 0 }));
    const hrefs = (form: string) =>
      [...(host.querySelector(`form[aria-label="${form}"]`)?.querySelectorAll("a") ?? [])]
        .filter((a) => a.textContent === "Open token page")
        .map((a) => a.getAttribute("href"));
    expect(hrefs("GitHub Token (api.github.com)")).toEqual([
      "https://github.com/settings/personal-access-tokens/new",
    ]);
    expect(hrefs("Hugging Face Token")).toEqual(["https://huggingface.co/settings/tokens"]);
    // The cloud key page depends on the provider; an unknown host gets a disabled button.
    const cloud = host.querySelector('form[aria-label="Cloud LLM API Key"]')!;
    const open = [...cloud.querySelectorAll("a, button")].find(
      (e) => e.textContent === "Open token page",
    );
    expect(open).toBeDefined();
    expect(host.querySelectorAll('a[href^="http://"]').length).toBe(0);
  });

  it("lists the permissions a fine-grained GitHub token needs", async () => {
    await mount(createMockBackend({ clockRate: 0 }));
    const text = host.querySelector('[data-testid="github-permissions"]')?.textContent ?? "";
    for (const p of ["Administration", "Contents", "Issues"]) expect(text).toContain(p);
  });
});
