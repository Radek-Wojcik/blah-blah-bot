import { useCallback, useEffect, useState } from "react";
import { useBackend } from "./backend-context";
import { isIpcError } from "./ipc/tauri";
import type {
  CycleListItem,
  CycleRepoView,
  CycleTasks,
  MeetingSummary,
  OutboxState,
  QueueState,
  RecordingLevels,
  RecordingState,
  SourceInfo,
  Topic,
} from "./ipc/types";

const IDLE_BARS: number[] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

const idleLevels: RecordingLevels = {
  peak: 0,
  rms: 0,
  bars: IDLE_BARS,
  peakHold: 0,
};

const idle: RecordingState = {
  status: "idle",
  meetingId: null,
  elapsedMs: 0,
  sources: [],
  screenshotCount: 0,
};

export function errorMessage(e: unknown): string {
  if (isIpcError(e)) return e.message;
  if (e instanceof Error) return e.message;
  return String(e);
}

/** Live recording state: loaded once, then replaced by every `recording-state` event. */
export function useRecordingState(): RecordingState {
  const backend = useBackend();
  const [state, setState] = useState<RecordingState>(idle);
  useEffect(() => {
    let off: (() => void) | undefined;
    let cancelled = false;
    void backend.recordingState().then((s) => !cancelled && setState(s));
    void backend.onRecordingState(setState).then((u) => {
      if (cancelled) u();
      else off = u;
    });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [backend]);
  return state;
}

/**
 * Live 12-bar audio levels while `active` is true, driven by `recording-levels` events from the
 * backend. Resets to zeroed bars when inactive.
 */
export function useRecordingLevels(active = true): RecordingLevels {
  const backend = useBackend();
  const [levels, setLevels] = useState<RecordingLevels>(idleLevels);
  useEffect(() => {
    if (!active) return;
    let off: (() => void) | undefined;
    let cancelled = false;
    void backend
      .onRecordingLevels((next) => {
        if (!cancelled) setLevels(next);
      })
      .then((u) => {
        if (cancelled) u();
        else off = u;
      });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [backend, active]);
  return active ? levels : idleLevels;
}

export function useMeetings(): { meetings: MeetingSummary[]; reload: () => void } {
  const backend = useBackend();
  const [meetings, setMeetings] = useState<MeetingSummary[]>([]);
  const reload = useCallback(() => {
    void backend.listMeetings().then(setMeetings);
  }, [backend]);
  useEffect(() => {
    reload();
    let off: (() => void) | undefined;
    let cancelled = false;
    let offQueue: (() => void) | undefined;
    void backend.onMeetingsChanged(reload).then((u) => {
      if (cancelled) u();
      else off = u;
    });
    // The job chip of a meeting follows the queue.
    void backend.onQueueChanged(reload).then((u) => {
      if (cancelled) u();
      else offQueue = u;
    });
    return () => {
      cancelled = true;
      off?.();
      offQueue?.();
    };
  }, [backend, reload]);
  return { meetings, reload };
}

/** The processing queue: loaded once, then replaced by every `queue-changed` event. */
export function useQueue(): QueueState | null {
  const backend = useBackend();
  const [queue, setQueue] = useState<QueueState | null>(null);
  useEffect(() => {
    let off: (() => void) | undefined;
    let cancelled = false;
    void backend.queueState().then((q) => !cancelled && setQueue(q));
    void backend.onQueueChanged(setQueue).then((u) => {
      if (cancelled) u();
      else off = u;
    });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [backend]);
  return queue;
}

export function useSources(refreshKey: unknown): SourceInfo[] {
  const backend = useBackend();
  const [sources, setSources] = useState<SourceInfo[]>([]);
  useEffect(() => {
    let cancelled = false;
    void backend.listSources().then((s) => !cancelled && setSources(s));
    return () => {
      cancelled = true;
    };
  }, [backend, refreshKey]);
  return sources;
}

/** The cycles and repositories: loaded once, then again on every `cycles-changed` event. */
export function useCycles(): {
  cycles: CycleListItem[] | null;
  repos: CycleRepoView[];
  reload: () => void;
} {
  const backend = useBackend();
  const [cycles, setCycles] = useState<CycleListItem[] | null>(null);
  const [repos, setRepos] = useState<CycleRepoView[]>([]);
  const reload = useCallback(() => {
    void backend.listCycles().then(setCycles);
    void backend.listCycleRepos().then(setRepos);
  }, [backend]);
  useEffect(() => {
    reload();
    let off: (() => void) | undefined;
    let cancelled = false;
    void backend.onCyclesChanged(reload).then((u) => {
      if (cancelled) u();
      else off = u;
    });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [backend, reload]);
  return { cycles, repos, reload };
}

/** The outbox of publishing: loaded once, then replaced by every `outbox-changed` event. */
export function useOutbox(): OutboxState | null {
  const backend = useBackend();
  const [state, setState] = useState<OutboxState | null>(null);
  useEffect(() => {
    let off: (() => void) | undefined;
    let cancelled = false;
    void backend.outboxState().then((s) => !cancelled && setState(s));
    void backend.onOutboxChanged(setState).then((u) => {
      if (cancelled) u();
      else off = u;
    });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [backend]);
  return state;
}

/**
 * The tasks of a cycle (its GitHub issues as last read) and its topics, again on every
 * `cycles-changed` event: a pull of the repository reads the issues again.
 */
export function useCycleFacts(cycleId: string | undefined): {
  tasks: CycleTasks | null;
  topics: Topic[];
  discordGuildId: string | null;
  reload: () => void;
} {
  const backend = useBackend();
  const [tasks, setTasks] = useState<CycleTasks | null>(null);
  const [topics, setTopics] = useState<Topic[]>([]);
  const [discordGuildId, setDiscordGuildId] = useState<string | null>(null);
  const reload = useCallback(() => {
    if (!cycleId) return;
    void backend
      .cycleTasks(cycleId)
      .then(setTasks)
      .catch(() => setTasks(null));
    void backend
      .getCycle(cycleId)
      .then((d) => {
        setTopics(d.topics);
        setDiscordGuildId(d.cycle.targets.discord?.bot?.guild ?? null);
      })
      .catch(() => {
        setTopics([]);
        setDiscordGuildId(null);
      });
  }, [backend, cycleId]);
  useEffect(() => {
    reload();
    let off: (() => void) | undefined;
    let cancelled = false;
    void backend.onCyclesChanged(reload).then((u) => {
      if (cancelled) u();
      else off = u;
    });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [backend, reload]);
  return { tasks, topics, discordGuildId, reload };
}
