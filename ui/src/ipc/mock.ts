import type { Backend, Unsubscribe } from "./backend";
import type {
  ActiveSource,
  MeetingDetails,
  MeetingSummary,
  RecordingState,
  Settings,
  SourceInfo,
} from "./types";

const MIN = 60_000;
/** Fixed reference time so demo data and screenshots are reproducible. */
export const DEMO_NOW = Date.UTC(2026, 10, 13, 14, 0, 0);

const demoSources: SourceInfo[] = [
  { id: "mic:default", kind: "microphone", label: "Microphone (USB headset)", active: true },
  { id: "app:discord", kind: "application", label: "Discord", active: true },
  { id: "app:teams", kind: "application", label: "Microsoft Teams", active: false },
  { id: "app:chrome", kind: "application", label: "Google Chrome", active: true },
];

function meeting(
  id: string,
  title: string,
  startedUnixMs: number,
  durationMs: number,
  state: MeetingSummary["state"],
  trackCount: number,
  screenshotCount: number,
): MeetingSummary {
  return { id, title, startedUnixMs, durationMs, state, trackCount, screenshotCount };
}

const demoMeetings: MeetingSummary[] = [
  meeting("m5", "ORBIT weekly sync", DEMO_NOW - 2 * 3600_000, 47 * MIN, "stopped", 3, 4),
  meeting("m4", "Thruster review", DEMO_NOW - 26 * 3600_000, 82 * MIN, "stopped", 2, 1),
  meeting("m3", "Budget check-in", DEMO_NOW - 50 * 3600_000, 31 * MIN, "stopped", 2, 0),
  meeting("m2", "Ground station planning", DEMO_NOW - 98 * 3600_000, 64 * MIN, "stopped", 3, 7),
  meeting("m1", "Kickoff", DEMO_NOW - 170 * 3600_000, 95 * MIN, "recovered", 3, 2),
];

export interface MockOptions {
  /** Start with a recording in progress (used for the widget screenshots). */
  recording?: boolean;
  /** Simulated milliseconds added per real second while recording. */
  clockRate?: number;
}

function fail(code: string, message: string): never {
  throw { code, message };
}

/** In-memory backend that serves the fictional demo cycle. No audio devices, files or network. */
export function createMockBackend(opts: MockOptions = {}): Backend {
  const settings: Settings = {
    recordingsDir: "Documents\\BlahBlahBot\\recordings",
    recordHotkey: "Ctrl+Alt+R",
    screenshotHotkey: "Ctrl+Alt+S",
  };
  let meetings = [...demoMeetings];
  let state: RecordingState = {
    status: "idle",
    meetingId: null,
    elapsedMs: 0,
    sources: [],
    screenshotCount: 0,
  };
  const stateListeners = new Set<(s: RecordingState) => void>();
  const meetingListeners = new Set<() => void>();
  let timer: ReturnType<typeof setInterval> | null = null;
  const rate = opts.clockRate ?? 1000;

  const emit = () => stateListeners.forEach((cb) => cb(structuredClone(state)));
  const emitMeetings = () => meetingListeners.forEach((cb) => cb());

  function active(id: string, offsetMs: number): ActiveSource {
    const src = demoSources.find((s) => s.id === id);
    if (!src) fail("source_not_found", `Unknown source ${id}`);
    return { id, label: src.label, kind: src.kind, status: "recording", offsetMs, level: 0.3 };
  }

  function begin(sourceIds: string[], elapsed = 0) {
    state = {
      status: "recording",
      meetingId: "live",
      elapsedMs: elapsed,
      sources: sourceIds.map((id) => active(id, 0)),
      screenshotCount: 0,
    };
    timer = setInterval(() => {
      const elapsedMs = state.elapsedMs + rate;
      state = {
        ...state,
        elapsedMs,
        sources: state.sources.map((s, i) => ({
          ...s,
          level:
            s.status === "recording" ? 0.15 + 0.6 * Math.abs(Math.sin(elapsedMs / 900 + i)) : 0,
        })),
      };
      emit();
    }, 1000);
    emit();
  }

  if (opts.recording) begin(["mic:default", "app:discord"], 12 * MIN + 41_000);

  const subscribe = <T>(set: Set<T>, cb: T): Promise<Unsubscribe> => {
    set.add(cb);
    return Promise.resolve(() => void set.delete(cb));
  };

  return {
    listMeetings: async () => [...meetings],
    getMeeting: async (id): Promise<MeetingDetails> => {
      const summary = meetings.find((x) => x.id === id);
      if (!summary) fail("internal", "Meeting not found");
      return {
        summary,
        tracks: demoSources.slice(0, summary.trackCount).map((s, i) => ({
          name: s.id.replace(":", "-"),
          source: s.label,
          offsetMs: i * 4000,
          ended: true,
        })),
        screenshots: Array.from({ length: summary.screenshotCount }, (_, i) => ({
          file: `img/${String(i + 1).padStart(4, "0")}.png`,
          offsetMs: (i + 1) * 5 * MIN,
        })),
      };
    },
    deleteMeeting: async (id) => {
      meetings = meetings.filter((x) => x.id !== id);
      emitMeetings();
    },
    recoverMeeting: async (id) => {
      meetings = meetings.map((x) => (x.id === id ? { ...x, state: "recovered" } : x));
      emitMeetings();
    },
    listSources: async () => demoSources.map((s) => ({ ...s })),
    recordingState: async () => structuredClone(state),
    startRecording: async (req) => {
      if (state.status === "recording") fail("already_recording", "Already recording");
      begin(req.sourceIds);
    },
    stopRecording: async () => {
      if (state.status !== "recording") fail("not_recording", "Not recording");
      if (timer) clearInterval(timer);
      meetings = [
        meeting(
          `m${meetings.length + 1}`,
          "Untitled meeting",
          DEMO_NOW,
          state.elapsedMs,
          "stopped",
          state.sources.length,
          state.screenshotCount,
        ),
        ...meetings,
      ];
      state = { status: "idle", meetingId: null, elapsedMs: 0, sources: [], screenshotCount: 0 };
      emit();
      emitMeetings();
    },
    addSource: async (id) => {
      if (state.sources.some((s) => s.id === id && s.status === "recording"))
        fail("source_already_added", "Source already recorded");
      state = {
        ...state,
        sources: [...state.sources.filter((s) => s.id !== id), active(id, state.elapsedMs)],
      };
      emit();
    },
    removeSource: async (id) => {
      state = {
        ...state,
        sources: state.sources.map((s) =>
          s.id === id ? { ...s, status: "ended", level: 0, detail: "Removed by user" } : s,
        ),
      };
      emit();
    },
    takeScreenshot: async () => {
      state = { ...state, screenshotCount: state.screenshotCount + 1 };
      emit();
    },
    getSettings: async () => ({ ...settings }),
    saveSettings: async (s) => void Object.assign(settings, s),
    onRecordingState: (cb) => subscribe(stateListeners, cb),
    onMeetingsChanged: (cb) => subscribe(meetingListeners, cb),
  };
}
