/**
 * The briefing of the mock backend: fictional tasks for the demo cycle NOMAD, grouped like
 * `crates/cycle-store/src/briefing.rs` does (a GitHub login maps to a participant, an unknown
 * login and the unassigned tasks get their own groups, tasks are sorted by due date, overdue is
 * counted against the mock's today). A pull of a clone moves its "state as of" date; a clone that
 * cannot be pulled shows the old date and the reason.
 */
import type { Unsubscribe } from "./backend";
import { MOCK_TODAY, type CycleStore } from "./mock-cycles";
import { fail } from "./mock-util";
import type { Briefing, BriefingGroup, BriefingTask, CycleParticipant, Topic } from "./types";

/** A task as a source reports it: before grouping. */
interface RawTask {
  id: string;
  text: string;
  source: "issue" | "meeting";
  github?: string;
  participant?: string;
  due?: string;
  issue?: number;
  meeting?: string;
}

const NOMAD_OPEN: RawTask[] = [
  {
    id: "#12",
    text: "Order the battery pack",
    source: "issue",
    github: "jan-demo",
    due: "2026-11-10",
    issue: 12,
  },
  {
    id: "#14",
    text: "Review the CAN bus wiring diagram",
    source: "issue",
    github: "alex-demo",
    due: "2026-11-20",
    issue: 14,
  },
  {
    id: "#15",
    text: "Book the test field for the first drive",
    source: "issue",
    github: "octo-guest",
    due: "2026-11-27",
    issue: 15,
  },
  {
    id: "2026-11-06/i1",
    text: "Design the monitor mount",
    source: "meeting",
    participant: "alex",
    due: "2026-11-18",
    meeting: "2026-11-06",
  },
  {
    id: "2026-11-06/i2",
    text: "Prepare the test plan for the field trip",
    source: "meeting",
    participant: "kasia",
    due: "2026-11-12",
    meeting: "2026-11-06",
  },
  {
    id: "2026-11-06/i3",
    text: "Check the GNSS antenna placement",
    source: "meeting",
    participant: "piotr",
    meeting: "2026-11-06",
  },
  {
    id: "2026-11-06/i4",
    text: "Ask the supplier about the delivery date",
    source: "meeting",
    meeting: "2026-11-06",
  },
];

const DEMO_BRIEFING_TOPIC: Topic = {
  id: "t-4",
  text: "Agree on the date of the first field test",
  status: "open",
  openedIn: "2026-11-13",
};

const NOMAD_DONE: RawTask[] = [
  {
    id: "2026-11-06/i5",
    text: "Order the battery",
    source: "meeting",
    participant: "jan",
    meeting: "2026-11-06",
  },
];

function entry(t: RawTask): BriefingTask {
  const due = t.due ?? null;
  return {
    id: t.id,
    text: t.text,
    due,
    overdue: due !== null && due < MOCK_TODAY,
    source: t.source,
    issue: t.issue ?? null,
    url: null,
    meeting: t.meeting ?? null,
  };
}

const byDue = (a: BriefingTask, b: BriefingTask) =>
  a.due === b.due
    ? a.text.localeCompare(b.text)
    : a.due === null
      ? 1
      : b.due === null
        ? -1
        : a.due.localeCompare(b.due);

function groupRaw(tasks: RawTask[], people: CycleParticipant[]): BriefingGroup[] {
  const owner = (t: RawTask): Pick<BriefingGroup, "kind" | "participantId" | "name" | "github"> => {
    const login = t.github?.toLowerCase();
    const p =
      (login && people.find((x) => x.github?.toLowerCase() === login)) ||
      people.find((x) => x.id === t.participant);
    if (p)
      return { kind: "participant", participantId: p.id, name: p.name, github: p.github ?? null };
    if (t.github)
      return { kind: "other", participantId: null, name: `@${t.github}`, github: t.github };
    if (t.participant)
      return { kind: "other", participantId: null, name: t.participant, github: null };
    return { kind: "unassigned", participantId: null, name: "Unassigned", github: null };
  };
  const groups = new Map<string, BriefingGroup>();
  for (const t of tasks) {
    const o = owner(t);
    const key = `${o.kind}:${o.participantId ?? o.name}`;
    const g = groups.get(key) ?? { ...o, overdueCount: 0, tasks: [] };
    groups.set(key, g);
    g.tasks.push(entry(t));
  }
  const rank = (g: BriefingGroup) =>
    g.kind === "participant"
      ? people.findIndex((p) => p.id === g.participantId)
      : g.kind === "other"
        ? 1000
        : 2000;
  return [...groups.values()]
    .map((g) => {
      const sorted = g.tasks.sort(byDue);
      return { ...g, tasks: sorted, overdueCount: sorted.filter((t) => t.overdue).length };
    })
    .sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name));
}

export function createMockBriefing(cycles: CycleStore) {
  const openers = new Set<(cycleId: string) => void>();
  return {
    /**
     * The recorder window asks the main window to show a briefing. The mock has one page per
     * window, so without a main window listening the page itself becomes the main window.
     */
    openBriefing: async (cycleId: string): Promise<void> => {
      if (openers.size > 0) {
        openers.forEach((cb) => cb(cycleId));
        return;
      }
      const params = new URLSearchParams(window.location.search);
      params.set("view", "main");
      params.set("page", "briefing");
      params.set("cycle", cycleId);
      window.location.assign(`${window.location.pathname}?${params.toString()}`);
    },
    onOpenBriefing: async (cb: (cycleId: string) => void): Promise<Unsubscribe> => {
      openers.add(cb);
      return () => void openers.delete(cb);
    },
    getBriefing: async (cycleId: string, refresh: boolean): Promise<Briefing> => {
      const details = cycles.getCycle(cycleId);
      const repo = cycles.listRepos().find((r) => r.id === details.repoId);
      if (!repo) fail("not_found", "this cycle repository is not registered");
      let pull: Briefing["pull"] = null;
      let error = repo.lastError;
      let current = repo;
      if (refresh && repo.kind === "clone") {
        const result = cycles.pullRepo(repo.id);
        pull =
          result.outcome === "failed"
            ? "failed"
            : result.outcome === "updated"
              ? "updated"
              : "upToDate";
        error = result.error ?? (result.repo.offline ? result.repo.lastError : null);
        current = result.repo;
      }
      const demo = cycleId === "NOMAD";
      const groups = groupRaw(demo ? NOMAD_OPEN : [], details.cycle.participants);
      const warnings = details.issues
        .slice(0, 1)
        .map(
          (i) =>
            `the cycle has problems, so people may be missing from the groups: ${i.path}: ${i.message}`,
        );
      return {
        cycleId,
        cycleName: details.cycle.name,
        today: MOCK_TODAY,
        groups,
        openCount: groups.reduce((n, g) => n + g.tasks.length, 0),
        overdueCount: groups.reduce((n, g) => n + g.overdueCount, 0),
        done: demo ? groupRaw(NOMAD_DONE, details.cycle.participants).flatMap((g) => g.tasks) : [],
        // The demo cycle has one more open topic here than in the cycle store, which the
        // publishing mock shares and whose topic list its tests fix.
        topics: [
          ...details.topics.filter((t) => t.status === "open"),
          ...(demo ? [DEMO_BRIEFING_TOPIC] : []),
        ],
        hasRemote: current.remote !== null,
        stateAsOf: current.stateAsOf,
        offline: current.offline,
        fromCache: current.offline,
        pull,
        error,
        warnings,
      };
    },
  };
}
