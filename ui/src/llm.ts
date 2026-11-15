import type { LlmKind, LlmSettings } from "./ipc/types";

/** The context sizes the worker accepts (`crates/app/src/llm.rs`). */
export const MIN_CTX = 2048;
export const MAX_CTX = 1_048_576;

export const KIND_LABELS: Record<LlmKind, string> = {
  managed: "Local model",
  external: "External server",
  cloud: "Cloud",
};

/** The host of an http(s) address, or null when it has none. */
export function urlHost(url: string): string | null {
  const m = /^https?:\/\/([^/?#]*)/i.exec(url.trim());
  if (!m) return null;
  const authority = m[1];
  const host = authority.slice(authority.lastIndexOf("@") + 1).replace(/:\d+$/, "");
  return host === "" ? null : host;
}

function checkUrl(what: string, url: string): string | null {
  const u = url.trim();
  if (u === "") return null;
  const m = /^https?:\/\/([^/?#]*)/i.exec(u);
  if (!m) return `${what}: the address must start with http:// or https://`;
  if (m[1].includes("@"))
    return `${what}: do not put a user name or key into the address; keys are stored separately`;
  if (urlHost(u) === null) return `${what}: the address has no host`;
  return null;
}

/** The rules the backend enforces on the language model settings. */
export function validateLlmSettings(s: LlmSettings): string | null {
  if (!Number.isInteger(s.managed.ctx) || s.managed.ctx < MIN_CTX || s.managed.ctx > MAX_CTX)
    return `The context size must be between ${MIN_CTX} and ${MAX_CTX} tokens.`;
  return (
    checkUrl("External server", s.external.baseUrl) ?? checkUrl("Cloud endpoint", s.cloud.baseUrl)
  );
}

/** What is missing for `kind` to be usable, or null when it is configured. */
export function missingForKind(s: LlmSettings, kind: LlmKind): string | null {
  if (kind === "managed") return s.managed.modelPath.trim() === "" ? "model file" : null;
  const e = kind === "external" ? s.external : s.cloud;
  if (e.baseUrl.trim() === "") return "address";
  if (e.model.trim() === "") return "model name";
  return null;
}

export function isConfigured(s: LlmSettings, kind: LlmKind): boolean {
  return missingForKind(s, kind) === null;
}

/** The last part of a file path (either separator). */
export function baseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

/** Short description of a backend for choices: the model it would use. */
export function backendDetail(s: LlmSettings, kind: LlmKind): string {
  const missing = missingForKind(s, kind);
  if (missing) return `not configured (no ${missing})`;
  if (kind === "managed") return baseName(s.managed.modelPath);
  const e = kind === "external" ? s.external : s.cloud;
  const host = urlHost(e.baseUrl);
  return kind === "cloud" && host ? `${e.model} at ${host}` : e.model;
}

/** The note shown for an extraction stage that was skipped. */
export function skipText(note: string): string {
  return `Skipped: ${note}`;
}
