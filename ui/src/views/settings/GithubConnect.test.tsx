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

/** One poll interval of the mock (1 s) plus the margin the component adds. */
const INTERVAL = 1250;

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
    await tick(INTERVAL);
    expect(poll).toHaveBeenCalledTimes(1);
    expect(host.textContent).toContain("Waiting for approval");
    await tick(INTERVAL);
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
    await tick(INTERVAL);
    expect(poll).toHaveBeenCalledTimes(1);
    await tick(4000);
    expect(poll).toHaveBeenCalledTimes(1);
    await tick(250);
    expect(poll).toHaveBeenCalledTimes(2);
    expect(host.querySelector('[role="alert"]')?.textContent).toMatch(/denied/i);
    expect(host.querySelector('[data-testid="github-device-code"]')).toBeNull();

    hooks().setGithubConnect({ script: [{ state: "expired" }] });
    await click(button("Connect account"));
    await tick(INTERVAL);
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

describe("Connect account: races and safety", () => {
  const start = {
    userCode: "WDJB-MJHT",
    verificationUri: "https://github.com/login/device",
    interval: 1,
    expiresIn: 900,
  };
  const deferred = <T,>() => {
    let resolve!: (v: T) => void;
    const promise = new Promise<T>((r) => (resolve = r));
    return { promise, resolve };
  };

  it("cancels the backend when unmounted while the start is still out", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const gate = deferred<typeof start>();
    vi.spyOn(backend, "githubDeviceStart").mockReturnValue(gate.promise);
    const poll = vi.spyOn(backend, "githubDevicePoll");
    const cancel = vi.spyOn(backend, "githubDeviceCancel");
    await mount(backend);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    await click(button("Connect account"));
    expect(button("Starting…").disabled).toBe(true);
    act(() => root.unmount());
    expect(cancel).toHaveBeenCalledTimes(1);
    await act(async () => gate.resolve(start));
    // The late answer is dropped: the session it made is cancelled too, and nothing polls.
    expect(cancel).toHaveBeenCalledTimes(2);
    await tick(10_000);
    expect(poll).not.toHaveBeenCalled();
    root = createRoot(host);
  });

  it("a double click starts once, and Disconnect is not offered while starting", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const gate = deferred<typeof start>();
    const startSpy = vi.spyOn(backend, "githubDeviceStart").mockReturnValue(gate.promise);
    await mount(backend);
    const connect = button("Connect account");
    await act(async () => {
      connect.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      connect.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(startSpy).toHaveBeenCalledTimes(1);
    expect([...host.querySelectorAll("button")].some((b) => b.textContent === "Disconnect")).toBe(
      false,
    );
    await act(async () => gate.resolve(start));
  });

  it("a cancel while starting drops the late answer and allows a new attempt", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const gate = deferred<typeof start>();
    const startSpy = vi.spyOn(backend, "githubDeviceStart").mockReturnValueOnce(gate.promise);
    const poll = vi.spyOn(backend, "githubDevicePoll");
    const cancel = vi.spyOn(backend, "githubDeviceCancel");
    await mount(backend);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    await click(button("Connect account"));
    expect(button("Starting…").disabled).toBe(true);
    // Still starting: the answer has not come.
    await click(button("Cancel"));
    expect(cancel).toHaveBeenCalledTimes(1);
    expect(button("Connect account").disabled).toBe(false);
    await act(async () => gate.resolve(start));
    // The late answer neither shows a code nor polls; its session is cancelled once more.
    expect(host.querySelector('[data-testid="github-device-code"]')).toBeNull();
    expect(cancel).toHaveBeenCalledTimes(2);
    await tick(10_000);
    expect(poll).not.toHaveBeenCalled();
    // A fresh attempt works.
    await click(button("Connect account"));
    expect(startSpy).toHaveBeenCalledTimes(2);
    expect(host.querySelector('[data-testid="github-device-code"]')).not.toBeNull();
  });

  it("the late answer of a cancelled poll cannot disturb the next attempt", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const old = deferred<{ state: "pending" }>();
    const pollSpy = vi
      .spyOn(backend, "githubDevicePoll")
      .mockReturnValueOnce(old.promise)
      .mockResolvedValue({ state: "pending" });
    const cancel = vi.spyOn(backend, "githubDeviceCancel");
    await mount(backend);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    await click(button("Connect account"));
    await tick(INTERVAL);
    expect(pollSpy).toHaveBeenCalledTimes(1);
    await click(button("Cancel"));
    await click(button("Connect account"));
    expect(cancel).toHaveBeenCalledTimes(1);
    // The old poll answers late, with pending: no second timer starts from it.
    await act(async () => old.resolve({ state: "pending" }));
    await tick(INTERVAL);
    expect(pollSpy).toHaveBeenCalledTimes(2);
    await tick(INTERVAL);
    expect(pollSpy).toHaveBeenCalledTimes(3);
    expect(cancel).toHaveBeenCalledTimes(1);
  });

  it("a late start answer after unmount does not cancel a newer instance", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const first = deferred<typeof start>();
    const second = deferred<typeof start>();
    vi.spyOn(backend, "githubDeviceStart")
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);
    const cancel = vi.spyOn(backend, "githubDeviceCancel");
    await mount(backend);
    await click(button("Connect account"));
    act(() => root.unmount());
    expect(cancel).toHaveBeenCalledTimes(1);
    root = createRoot(host);
    await mount(backend);
    await click(button("Connect account"));
    await act(async () => first.resolve(start));
    // The older answer is ignored without touching the backend session of the newer attempt.
    expect(cancel).toHaveBeenCalledTimes(1);
    await act(async () => second.resolve(start));
    expect(host.querySelector('[data-testid="github-device-code"]')).not.toBeNull();
  });

  it("opens only github.com without a port: any other page is refused", async () => {
    for (const verificationUri of [
      "https://evil.example/login/device",
      "https://github.com.evil.example/login/device",
      "https://github.com:8443/login/device",
      "http://github.com/login/device",
      "https://user:pw@github.com/login/device",
      "https://github.com/login/device?next=https://evil.example",
      "https://github.com/login/device#x",
      "https://github.com",
      "https://github.com/other",
    ]) {
      const backend = createMockBackend({ clockRate: 0 });
      vi.spyOn(backend, "githubDeviceStart").mockResolvedValue({ ...start, verificationUri });
      await mount(backend);
      await click(button("Connect account"));
      expect(host.querySelector('[data-testid="github-device-code"]')).not.toBeNull();
      expect(
        [...host.querySelectorAll("a")].some((a) => a.textContent?.includes("github.com/login")),
        verificationUri,
      ).toBe(false);
      expect(host.querySelector('[role="alert"]')?.textContent).toMatch(/safe page/i);
      await click(button("Cancel"));
      act(() => root.unmount());
      root = createRoot(host);
    }
  });

  it("a failed poll goes back to idle and cancels the dead session", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    vi.spyOn(backend, "githubDevicePoll").mockRejectedValue(new Error("GitHub refused"));
    const cancel = vi.spyOn(backend, "githubDeviceCancel");
    await mount(backend);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    await click(button("Connect account"));
    await tick(INTERVAL);
    expect(cancel).toHaveBeenCalledTimes(1);
    expect(host.querySelector('[role="alert"]')?.textContent).toContain("GitHub refused");
    expect(button("Connect account").disabled).toBe(false);
  });

  it("unmounting while waiting stops the polling and cancels", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const poll = vi.spyOn(backend, "githubDevicePoll");
    const cancel = vi.spyOn(backend, "githubDeviceCancel");
    await mount(backend);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    await click(button("Connect account"));
    act(() => root.unmount());
    await tick(10_000);
    expect(poll).not.toHaveBeenCalled();
    expect(cancel).toHaveBeenCalled();
    root = createRoot(host);
  });

  it("forgets Connected when the token is removed in the field below", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    await mount(backend);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    await click(button("Connect account"));
    await tick(INTERVAL);
    await tick(INTERVAL);
    expect(host.querySelector('[data-testid="github-connected"]')).not.toBeNull();
    const form = host.querySelector('form[aria-label="GitHub Token (api.github.com)"]')!;
    const clear = [...form.querySelectorAll("button")].find((b) => b.textContent === "Clear")!;
    await click(clear);
    expect(host.querySelector('[data-testid="github-connected"]')).toBeNull();
  });

  it("forgets Connected when a token is stored over it in the field below", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    await mount(backend);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    await click(button("Connect account"));
    await tick(INTERVAL);
    await tick(INTERVAL);
    expect(host.querySelector('[data-testid="github-connected"]')).not.toBeNull();
    const form = host.querySelector('form[aria-label="GitHub Token (api.github.com)"]')!;
    const input = form.querySelector("input")!;
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    await act(async () => {
      setter.call(input, "github_pat_" + "a".repeat(30));
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    const store = [...form.querySelectorAll("button")].find((b) => b.textContent === "Store")!;
    await click(store);
    expect(host.querySelector('[data-testid="github-connected"]')).toBeNull();
  });

  it("Connect account is disabled while Disconnect is out", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const gate = deferred<void>();
    vi.spyOn(backend, "clearGithubToken").mockReturnValue(gate.promise);
    await mount(backend);
    await click(button("Disconnect"));
    expect(button("Connect account").disabled).toBe(true);
    await act(async () => gate.resolve());
    await flush();
    expect(button("Connect account").disabled).toBe(false);
  });

  it("Disconnect is disabled while the request is out and ignored after unmount", async () => {
    const backend = createMockBackend({ clockRate: 0 });
    const gate = deferred<void>();
    vi.spyOn(backend, "clearGithubToken").mockReturnValue(gate.promise);
    await mount(backend);
    await click(button("Disconnect"));
    expect(button("Disconnect").disabled).toBe(true);
    act(() => root.unmount());
    await act(async () => gate.resolve());
    root = createRoot(host);
  });
});

describe("Open token page buttons", () => {
  it("every connector with a token has one that opens an https provider page", async () => {
    await mount(createMockBackend({ clockRate: 0 }));
    const hrefs = (form: string) =>
      [...(host.querySelector(`form[aria-label="${form}"]`)?.querySelectorAll("a") ?? [])]
        .filter((a) => a.textContent === "Open token page")
        .map((a) => a.getAttribute("href"));
    const labels = [...host.querySelectorAll("a, button")]
      .filter((e) => e.textContent === "Open token page")
      .map((e) => e.getAttribute("aria-label"));
    // Every one says whose token page it opens, and no two read alike.
    expect(labels.every(Boolean)).toBe(true);
    expect(new Set(labels).size).toBe(labels.length);
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
