/** View-model logic of the briefing screen: wording and small calculations, no React. */
import type { Briefing, BriefingTask } from "./ipc/types";

export function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** Whole days from `from` to `to`, both `YYYY-MM-DD` (negative when `to` is earlier). */
export function daysBetween(from: string, to: string): number {
  const ms = (d: string) => {
    const [y, m, day] = d.split("-").map(Number);
    return Date.UTC(y, m - 1, day);
  };
  return Math.round((ms(to) - ms(from)) / 86_400_000);
}

/** The due date as the task row shows it: `Due 2026-11-20`, `Overdue by 3 days (2026-11-10)`, `No due date`. */
export function dueLabel(task: Pick<BriefingTask, "due" | "overdue">, today: string): string {
  if (task.due === null) return "No due date";
  if (task.overdue) {
    const days = Math.max(1, daysBetween(task.due, today));
    return `Overdue by ${plural(days, "day", "days")} (${task.due})`;
  }
  return task.due === today ? "Due today" : `Due ${task.due}`;
}

/** Where a task comes from: `Issue #12`, `Meeting 2026-11-06`, `Meeting 2026-11-06 · issue #12`. */
export function sourceLabel(task: Pick<BriefingTask, "source" | "issue" | "meeting">): string {
  if (task.source === "issue") return task.issue === null ? "Issue" : `Issue #${task.issue}`;
  const meeting = task.meeting === null ? "Meeting" : `Meeting ${task.meeting}`;
  return task.issue === null ? meeting : `${meeting} · issue #${task.issue}`;
}

/** `7 open tasks, 2 overdue · 3 open topics` */
export function summaryLine(b: Pick<Briefing, "openCount" | "overdueCount" | "topics">): string {
  const tasks = plural(b.openCount, "open task", "open tasks");
  const overdue = b.overdueCount > 0 ? `, ${b.overdueCount} overdue` : "";
  return `${tasks}${overdue} · ${plural(b.topics.length, "open topic", "open topics")}`;
}

export type StatusTone = "ok" | "stale";

/**
 * The line under the title: how fresh what is shown is. A clone always says "State as of <date>";
 * a failed update says why and keeps the date, an update that was skipped because of
 * uncommitted cycle edits says that.
 */
export function statusLine(
  b: Pick<Briefing, "hasRemote" | "stateAsOf" | "offline" | "error" | "pull">,
  refreshing: boolean,
): { text: string; tone: StatusTone } {
  if (!b.hasRemote) {
    const issues = b.stateAsOf ? ` Issues as of ${b.stateAsOf}.` : "";
    return { text: `Local repository, nothing to update.${issues}`, tone: "ok" };
  }
  const asOf = b.stateAsOf ? `State as of ${b.stateAsOf}` : "State as of an unknown date";
  if (b.offline || b.pull === "failed") {
    const why = b.error ? `: ${b.error}` : "";
    return { text: `${asOf}. Could not update${why}`, tone: "stale" };
  }
  if (b.pull === "skippedDirty") {
    return {
      text: `${asOf}. Not updated: project files have changes that are not committed.`,
      tone: "stale",
    };
  }
  if (refreshing) return { text: `${asOf}. Updating from the remote…`, tone: "ok" };
  if (b.stateAsOf === null) return { text: "Not pulled yet.", tone: "stale" };
  const result = b.pull === "updated" ? " Updated just now." : "";
  return { text: `${asOf}.${result}`, tone: "ok" };
}
