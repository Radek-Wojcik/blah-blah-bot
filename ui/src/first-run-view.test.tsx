import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BackendProvider } from "./backend-context";
import type { StepId } from "./first-run";
import type { Backend } from "./ipc/backend";
import { createMockBackend } from "./ipc/mock";
import type { SetupScenario } from "./ipc/mock-setup";
import { FirstRun } from "./views/FirstRun";

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
});

const wait = (ms = 20) => act(async () => void (await new Promise((r) => setTimeout(r, ms))));

async function until(check: () => boolean, what: string) {
  for (let i = 0; i < 150 && !check(); i++) await wait();
  if (!check()) throw new Error(`timed out waiting for: ${what}`);
}

function backendFor(setup: SetupScenario): Backend {
  return createMockBackend({ clockRate: 0, jobStepMs: 40, setup });
}

async function show(
  backend: Backend,
  props: { initialStep?: StepId; onClose?: () => void; reason?: string } = {},
) {
  await act(async () => {
    root.render(
      <BackendProvider value={backend}>
        <FirstRun {...props} onClose={props.onClose ?? (() => {})} />
      </BackendProvider>,
    );
  });
  await wait();
}

const text = () => host.textContent ?? "";
const buttons = () => [...host.querySelectorAll("button")];
const button = (name: string | RegExp) =>
  buttons().find((b) => {
    const label = b.getAttribute("aria-label") ?? b.textContent ?? "";
    return typeof name === "string" ? label === name : name.test(label);
  });
const labelled = (name: string) =>
  [...host.querySelectorAll("label")]
    .find((l) => l.textContent?.includes(name))
    ?.querySelector("input") as HTMLInputElement;

async function click(name: string | RegExp) {
  const b = button(name);
  if (!b)
    throw new Error(`no button ${String(name)}; buttons: ${buttons().map((x) => x.textContent)}`);
  await act(async () => b.click());
  await wait();
}

async function type(input: HTMLInputElement, value: string) {
  await act(async () => {
    const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    set.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

const heading = () => host.querySelector("h2")?.textContent ?? "";
const alerts = () => [...host.querySelectorAll("[role=alert]")].map((a) => a.textContent).join("|");

describe("first-run wizard", () => {
  it("starts at the welcome step and moves with Next and Back", async () => {
    await show(backendFor("fresh"));
    expect(heading()).toBe("Welcome");
    expect(text()).toContain("Everything runs on this computer");
    expect(button("Back")?.disabled).toBe(true);
    await click("Next");
    expect(heading()).toBe("Processing environment");
    const current = host.querySelector("[aria-current=step]");
    expect(current?.textContent).toContain("Processing environment");
    await click("Back");
    expect(heading()).toBe("Welcome");
  });

  it("opens at the step of a refused processing and says why", async () => {
    await show(backendFor("no-vision"), {
      initialStep: "vision",
      reason: "The screenshot model is not downloaded.",
    });
    expect(heading()).toBe("Screenshot analysis");
    expect(alerts()).toContain(
      "Processing could not start: The screenshot model is not downloaded.",
    );
    // The focus is on the heading of the step.
    expect(document.activeElement?.id).toBe("setup-title");
  });

  it("prepares the environment with progress", async () => {
    await show(backendFor("fresh"), { initialStep: "env" });
    expect(text()).toContain("has not been created yet");
    await act(async () => button("Prepare environment")?.click());
    expect(host.querySelector("[role=progressbar]")).not.toBeNull();
    await until(() => text().includes("The processing environment is ready."), "env ready");
    expect(button("Prepare environment")?.disabled).toBe(true);
  });

  it("downloads a model with a progress bar and then shows it as downloaded", async () => {
    await show(backendFor("fresh"), { initialStep: "models" });
    await click(/^Download Whisper/);
    const bar = host.querySelector("[role=progressbar][aria-label^='Download of Whisper']");
    expect(bar).not.toBeNull();
    await until(() => text().includes("Downloaded"), "download done");
    expect(host.querySelector("[role=progressbar]")).toBeNull();
  });

  it("shows a failed download with a retry that works", async () => {
    await show(backendFor("download-fails"), { initialStep: "models" });
    await click(/^Download Whisper/);
    await until(() => alerts().includes("Download failed:"), "failure");
    expect(alerts()).toContain("No internet connection");
    await click(/^Retry download of Whisper/);
    await until(() => text().includes("Downloaded"), "retry done");
    expect(alerts()).not.toContain("Download failed");
  });

  it("keeps the token masked, empties the field and never puts it in the page", async () => {
    const secret = "hf_this_is_a_secret_token_value";
    const backend = backendFor("fresh");
    await show(backend, { initialStep: "models" });
    const input = labelled("Hugging Face token");
    expect(input.type).toBe("password");
    expect(input.autocomplete).toBe("off");
    expect(button(/^Download Speaker/)?.disabled).toBe(true);
    expect(text()).toContain("Store a Hugging Face token below first.");
    await type(input, secret);
    expect(button("Store token")?.disabled).toBe(false);
    await click("Store token");
    await until(() => text().includes("A token is stored"), "token stored");
    expect(labelled("Hugging Face token").value).toBe("");
    expect(labelled("Hugging Face token").type).toBe("password");
    expect(host.innerHTML).not.toContain(secret);
    expect(JSON.stringify(await backend.getProcessingSettings())).not.toContain(secret);
    expect(button(/^Download Speaker/)?.disabled).toBe(false);
  });

  it("explains a missing llama-server and refuses a path that does not exist", async () => {
    const backend = backendFor("no-llama");
    await show(backend, { initialStep: "llama" });
    expect(text()).toContain("llama-server was not found on this computer.");
    expect(text()).toContain("Install llama.cpp, then detect it in the setup.");
    expect(text()).toContain("None found.");
    const input = labelled("Path of llama-server.exe");
    await type(input, "C:\\nowhere\\llama-server.exe");
    await click("Use this llama-server");
    expect(alerts()).toContain("No file exists at C:\\nowhere\\llama-server.exe");
    expect(alerts()).toContain("Check the spelling");
    expect((await backend.getProcessingSettings()).llamaServerPath).toBe("");
    await type(input, "tools\\llama-server.exe");
    await click("Use this llama-server");
    expect(alerts()).toContain("The path must be absolute.");
  });

  it("chooses a detected llama-server", async () => {
    const backend = backendFor("fresh");
    await show(backend, { initialStep: "llama" });
    const candidate = host.querySelector("input[name=llama-candidate]") as HTMLInputElement;
    expect(candidate).not.toBeNull();
    await act(async () => candidate.click());
    await click("Use this llama-server");
    await until(() => text().includes("In use:"), "path saved");
    expect((await backend.getProcessingSettings()).llamaServerPath).toContain("llama-server.exe");
    expect(text()).toContain("llama-server: ");
  });

  it("scans for model files, chooses one and checks it", async () => {
    const backend = backendFor("fresh");
    await show(backend, { initialStep: "llm" });
    expect(text()).toContain("No language model was chosen");
    await click("Find model files");
    const files = host.querySelectorAll("input[name=llm-file]");
    expect(files).toHaveLength(2);
    // The projector of a vision model is not a language model.
    expect(text()).not.toContain("mmproj");
    await act(async () => (files[0] as HTMLInputElement).click());
    await click("Use this model");
    await until(() => text().includes("A language model is chosen."), "model saved");
    expect((await backend.getProcessingSettings()).llm?.managed.modelPath).toContain(".gguf");

    await type(labelled("Path of the model file"), "D:\\Models\\notes.txt");
    await click("Use this model");
    expect(alerts()).toContain("A .gguf file is expected");
  });

  it("skips reports and says what that means, and can turn them on again", async () => {
    const backend = backendFor("fresh");
    await show(backend, { initialStep: "llm" });
    await click("Skip reports for now");
    await until(() => text().includes("no report is written"), "consequence");
    expect((await backend.getProcessingSettings()).skipReports).toBe(true);
    expect(text()).toContain("Skipped");
    await click("Turn reports on");
    await until(() => !text().includes("no report is written"), "reports on");
    expect((await backend.getProcessingSettings()).skipReports).toBe(false);
  });

  it("offers to skip screenshot analysis when the vision model is missing", async () => {
    const backend = backendFor("no-vision");
    await show(backend, { initialStep: "vision" });
    expect(text()).toContain("The screenshot model is not downloaded.");
    await click("Skip screenshot analysis");
    await until(() => text().includes("Screenshot analysis is off."), "skipped");
    expect((await backend.getProcessingSettings()).describeScreenshots).toBe(false);
    expect((await backend.setupStatus()).ready).toBe(true);
    await click("Turn screenshot analysis on");
    await until(() => text().includes("The screenshot model is not downloaded."), "on again");
  });

  it("summarizes a complete setup", async () => {
    await show(backendFor("ready"), { initialStep: "summary" });
    expect(host.querySelectorAll(".setup-item")).toHaveLength(7);
    expect(text()).toContain("Everything needed is ready.");
    expect(button(/^Fix /)).toBeUndefined();
    expect(button("Continue")).toBeDefined();
  });

  it("lists what is missing on the summary and leads to the fix, without blocking", async () => {
    await show(backendFor("no-vision"), { initialStep: "summary" });
    expect(button("Continue anyway")).toBeDefined();
    expect(text()).toContain("Not set up yet: Screenshot analysis.");
    await click("Fix Screenshot analysis");
    expect(heading()).toBe("Screenshot analysis");
    await click("Next");
    expect(heading()).toBe("Summary");
    await click("Continue anyway");
    expect(heading()).toBe("First project");
  });

  it("creates the first cycle and finishes by marking the setup as done", async () => {
    const backend = backendFor("ready");
    const onClose = vi.fn();
    await show(backend, { initialStep: "cycle", onClose });
    expect(button("Create first project")?.disabled).toBe(true);
    await type(labelled("Project id"), "WEEKLY");
    await type(labelled("Project name"), "Weekly sync");
    await click("Create first project");
    await until(() => text().includes("Project WEEKLY was created."), "project created");
    expect((await backend.listCycles()).some((c) => c.id === "WEEKLY")).toBe(true);
    expect((await backend.getProcessingSettings()).firstRunDone).toBe(false);
    await click("Finish");
    await until(() => onClose.mock.calls.length > 0, "closed");
    expect((await backend.getProcessingSettings()).firstRunDone).toBe(true);
  });

  it("can be left at any time", async () => {
    const backend = backendFor("fresh");
    const onClose = vi.fn();
    await show(backend, { onClose });
    await click("Leave setup");
    await until(() => onClose.mock.calls.length > 0, "closed");
    expect((await backend.getProcessingSettings()).firstRunDone).toBe(true);
  });
});
