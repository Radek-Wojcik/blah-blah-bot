import type { SetupItem, SetupPathCheck, SetupStatus } from "./ipc/types";

/** The steps of the first-run wizard, in order. */
export type StepId =
  "welcome" | "env" | "models" | "llama" | "llm" | "vision" | "summary" | "cycle";

export const STEPS: ReadonlyArray<{ id: StepId; title: string }> = [
  { id: "welcome", title: "Welcome" },
  { id: "env", title: "Processing environment" },
  { id: "models", title: "Speech models" },
  { id: "llama", title: "llama-server" },
  { id: "llm", title: "Language model" },
  { id: "vision", title: "Screenshot analysis" },
  { id: "summary", title: "Summary" },
  { id: "cycle", title: "First cycle" },
];

/** The step that fixes the setup item with this key, or null for a key the wizard does not know. */
export function stepForItem(key: string): StepId | null {
  if (key === "workerEnv") return "env";
  if (key.startsWith("model:") || key === "hfToken") return "models";
  if (key === "llamaServer") return "llama";
  if (key === "llmModel") return "llm";
  if (key === "vision") return "vision";
  return null;
}

/** The step a `setup_*` code of the backend belongs to (`setup_model_missing:<key>` included). */
export function stepForCode(code: string | undefined): StepId | null {
  if (!code?.startsWith("setup_")) return null;
  const base = code.split(":")[0];
  switch (base) {
    case "setup_worker_env":
      return "env";
    case "setup_model_missing":
    case "setup_hf_token":
      return "models";
    case "setup_llama_server":
      return "llama";
    case "setup_llm_model":
      return "llm";
    case "setup_vision":
      return "vision";
    default:
      return "summary";
  }
}

/** The step a rejected `processMeeting` points to, or null for any other error. */
export function stepForError(e: unknown): StepId | null {
  if (typeof e !== "object" || e === null || !("code" in e)) return null;
  const code = (e as { code: unknown }).code;
  return typeof code === "string" ? stepForCode(code) : null;
}

export const isProblem = (i: SetupItem) => i.state === "missing" || i.state === "invalid";

export function itemOf(status: SetupStatus | null, key: string): SetupItem | undefined {
  return status?.items.find((i) => i.key === key);
}

/** The items a step takes care of. */
export function stepItems(status: SetupStatus | null, step: StepId): SetupItem[] {
  return (status?.items ?? []).filter((i) => stepForItem(i.key) === step);
}

export type StepState = "ok" | "todo" | "working" | "none";

/** `none`: the step has no item (welcome, summary, cycle). */
export function stepState(status: SetupStatus | null, step: StepId): StepState {
  const items = stepItems(status, step);
  if (items.length === 0) return "none";
  if (items.some((i) => i.state === "working")) return "working";
  return items.some(isProblem) ? "todo" : "ok";
}

/** The items that stop processing. */
export function problems(status: SetupStatus | null): SetupItem[] {
  return (status?.items ?? []).filter(isProblem);
}

/** The first step with a problem, or the summary when nothing is missing. */
export function firstOpenStep(status: SetupStatus | null): StepId {
  const item = problems(status)[0];
  return (item && stepForItem(item.key)) || "summary";
}

/** Short names of the items for the banner and the summary. */
export const ITEM_LABELS: Record<string, string> = {
  workerEnv: "Processing environment",
  "model:whisper-turbo": "Speech recognition model",
  "model:diarization": "Speaker model",
  hfToken: "Hugging Face token",
  llamaServer: "llama-server",
  llmModel: "Language model",
  vision: "Screenshot analysis",
};

export const itemLabel = (key: string) => ITEM_LABELS[key] ?? key;

export const STATE_TEXT: Record<SetupItem["state"], string> = {
  ok: "Ready",
  missing: "Missing",
  invalid: "Needs fixing",
  skipped: "Skipped",
  working: "Working",
};

/** The text of a readiness banner, or null when nothing is missing. */
export function bannerText(status: SetupStatus | null): string | null {
  const list = problems(status);
  if (list.length === 0) return null;
  const names = list.map((i) => itemLabel(i.key)).join(", ");
  return `Setup is incomplete: ${names}. Processing stops until this is fixed.`;
}

/** What a failed path check tells the user to do next. */
const PATH_FIX: Record<string, string> = {
  path_empty: "Enter a path or pick one of the files found.",
  path_not_absolute: "Use the full path, starting with a drive letter such as C:.",
  path_network: "Network paths are not accepted. Copy the file to a local drive first.",
  path_not_found: "Check the spelling, or pick one of the files found.",
  path_not_file: "The path must point to a file, not a folder.",
  path_not_dir: "The path must point to a folder.",
  path_wrong_kind: "Choose the right kind of file.",
};

/** The message of a failed check as the backend gave it, followed by the fix. */
export function pathProblem(check: SetupPathCheck): string {
  const fix = check.code ? PATH_FIX[check.code] : undefined;
  const message = check.message ?? "The path cannot be used.";
  return fix ? `${message} ${fix}` : message;
}

/** A download failure arrives as a progress message that starts with "Failed:". */
export function downloadFailure(message: string): string | null {
  const m = /^Failed:\s*(.*)$/s.exec(message);
  return m ? m[1] || "The download failed." : null;
}

export function formatBytes(bytes: number): string {
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
  if (bytes >= 1024 ** 2) return `${Math.round(bytes / 1024 ** 2)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

/** The folder part of a path (used to scan the folder of a chosen file). */
export function parentDir(path: string): string {
  const cut = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  return cut > 0 ? path.slice(0, cut) : "";
}

export function fileName(path: string): string {
  return path.slice(Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/")) + 1);
}

/** The text of the Finish button and what stands next to it. */
export function finishState(status: SetupStatus | null): { label: string; note: string | null } {
  const list = problems(status);
  if (list.length === 0) return { label: "Continue", note: null };
  return {
    label: "Continue anyway",
    note: `Not set up yet: ${list.map((i) => itemLabel(i.key)).join(", ")}. You can finish this later. The setup can be run again from Settings, and processing will say what is missing.`,
  };
}
