import type { SetupPhase, SetupProgress } from "../ipc/types";

export const SETUP_TIME_NOTE =
  "This takes 10-30 minutes and needs about 8 GB of free space. Keep the PC on.";

export const OFFLINE_MESSAGE =
  "No internet connection. Connect and press Retry; the download continues where it stopped.";

export type TaskAction = "start" | "cancel" | "retry" | "none";

const NETWORK_ERROR_RE =
  /offline|no internet|no network|network error|connection refused|connection reset|dns|timed?\s*out|unreachable|failed to connect|could not resolve|could not reach|connection to .+ was interrupted|download from .+ was interrupted|secure connection to .+ failed/i;

/** Whether a setup task is currently running and can be cancelled. */
export function isTaskActive(phase: SetupPhase | null | undefined): boolean {
  return phase === "started" || phase === "progress" || phase === "verifying";
}

/** True when a backend error indicates that a setup task is already running. */
export function isBusyError(e: unknown): boolean {
  if (typeof e === "object" && e !== null && "code" in e) {
    return (e as { code: unknown }).code === "busy";
  }
  return false;
}

/** Which primary button to show for a setup task based on its phase and readiness. */
export function taskActionForPhase(
  phase: SetupPhase | null | undefined,
  ready: boolean,
): TaskAction {
  if (isTaskActive(phase)) return "cancel";
  if (ready) return "none";
  if (phase === "failed" || phase === "cancelled" || phase === "done") return "retry";
  return "start";
}

/** Ensures a cancelled status sentence ends with terminal punctuation. */
export function formatCancelledMessage(message: string | undefined): string {
  const trimmed = (message ?? "").trim();
  if (trimmed === "") return "Cancelled.";
  return /[.!?]$/.test(trimmed) ? trimmed : `${trimmed}.`;
}

/** Percentage (0..100) when both downloaded and total byte counts are known. */
export function progressPercent(
  downloadedBytes: number | null,
  totalBytes: number | null,
): number | null {
  if (downloadedBytes === null || totalBytes === null || totalBytes <= 0) return null;
  return Math.max(0, Math.min(100, Math.round((downloadedBytes / totalBytes) * 100)));
}

/** Formats a byte count in megabytes for setup progress lines. */
export function formatMb(bytes: number): string {
  const mb = Math.max(0, bytes) / (1024 * 1024);
  if (mb > 0 && mb < 1) return `${mb.toFixed(1)} MB`;
  return `${Math.round(mb)} MB`;
}

/** Formats elapsed seconds as `12s` or `2m 05s`. */
export function formatElapsed(elapsedSecs: number): string {
  const total = Math.max(0, Math.floor(elapsedSecs));
  const mins = Math.floor(total / 60);
  const secs = total % 60;
  if (mins === 0) return `${secs}s`;
  return `${mins}m ${String(secs).padStart(2, "0")}s`;
}

/** True when a failure message indicates a network or offline error. */
export function isNetworkFailure(message: string): boolean {
  return NETWORK_ERROR_RE.test(message);
}

/** Plain user-facing failure text, replacing raw network errors with the offline message. */
export function formatFailureMessage(message: string): string {
  if (isNetworkFailure(message)) return OFFLINE_MESSAGE;
  const trimmed = message.trim();
  return trimmed !== "" ? trimmed : "The setup task failed.";
}

/**
 * Formats a `setup-progress` event for display:
 * - known totalBytes: percent and MB (`40% (80 MB / 200 MB)`)
 * - unknown totalBytes with downloadedBytes: MB downloaded and elapsed time (`120 MB downloaded · 4s`)
 * - verifying/done/cancelled/failed: phase-appropriate status text
 */
export function formatSetupProgress(p: SetupProgress): string {
  if (p.phase === "verifying") return p.message.trim() || "Verifying download...";
  if (p.phase === "done") return p.message.trim() || "Done.";
  if (p.phase === "cancelled") return p.message.trim() || "Cancelled.";
  if (p.phase === "failed") return formatFailureMessage(p.message);

  const pct = progressPercent(p.downloadedBytes, p.totalBytes);
  if (pct !== null && p.downloadedBytes !== null && p.totalBytes !== null) {
    return `${pct}% (${formatMb(p.downloadedBytes)} / ${formatMb(p.totalBytes)})`;
  }
  if (p.downloadedBytes !== null) {
    return `${formatMb(p.downloadedBytes)} downloaded · ${formatElapsed(p.elapsedSecs)}`;
  }
  const base = p.message.trim() || "Starting...";
  return p.elapsedSecs > 0 ? `${base} · ${formatElapsed(p.elapsedSecs)}` : base;
}
