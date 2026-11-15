import type { Backend, Unsubscribe } from "./backend";
import { stagesProgress } from "../processing";
import {
  STAGE_MESSAGES,
  demoImage,
  demoJobs,
  demoModels,
  demoProcessingSettings,
  demoTranscript,
  stageViews,
} from "./mock-demo";
import type {
  ActiveSource,
  JobView,
  MeetingDetails,
  MeetingSummary,
  ModelsProgress,
  QueueState,
  RecordingState,
  Settings,
  SourceInfo,
  TranscriptData,
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
  /** Milliseconds a processing stage (and a model download) takes. Default 1200. */
  jobStepMs?: number;
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
  const stateListeners = new Set<{ cb: (s: RecordingState) => void }>();
  const meetingListeners = new Set<{ cb: () => void }>();
  let timer: ReturnType<typeof setInterval> | null = null;
  const rate = opts.clockRate ?? 1000;

  // ----- processing pipeline -----
  const stepMs = Math.max(40, opts.jobStepMs ?? 1200);
  const processing = demoProcessingSettings();
  const models = demoModels();
  let jobs: JobView[] = demoJobs();
  const transcripts = new Map<string, TranscriptData>([["m5", demoTranscript("m5")]]);
  /** Jobs advanced by the timer, in order. The seeded demo jobs stay frozen. */
  const driven: string[] = [];
  let currentJob: string | null = null;
  let jobTimer: ReturnType<typeof setInterval> | null = null;
  const queueListeners = new Set<{ cb: (s: QueueState) => void }>();
  const progressListeners = new Set<{ cb: (p: ModelsProgress) => void }>();
  const modelsListeners = new Set<{ cb: () => void }>();

  const emit = () => stateListeners.forEach((l) => l.cb(structuredClone(state)));
  const emitMeetings = () => meetingListeners.forEach((l) => l.cb());
  const emitQueue = () => queueListeners.forEach((l) => l.cb({ jobs: structuredClone(jobs) }));

  const withJob = (m: MeetingSummary): MeetingSummary => ({
    ...m,
    job: jobs.find((j) => j.meetingId === m.id)?.state ?? null,
  });

  function patchJob(id: string, fn: (j: JobView) => JobView) {
    jobs = jobs.map((j) => (j.meetingId === id ? fn(j) : j));
  }

  function enqueue(id: string) {
    if (!driven.includes(id)) driven.push(id);
    if (!jobTimer) jobTimer = setInterval(tick, stepMs / 4);
  }

  /** Advance the job that is being worked on by one step, or start the next queued one. */
  function tick() {
    if (currentJob === null) {
      const next = driven.shift();
      if (next === undefined) {
        if (jobTimer) clearInterval(jobTimer);
        jobTimer = null;
        return;
      }
      currentJob = next;
    }
    const id = currentJob;
    patchJob(id, (j) => {
      const stages = j.stages.map((s) => ({ ...s }));
      let run = stages.find((s) => s.status === "running");
      if (!run) {
        run = stages.find((s) => s.status === "pending");
        if (run) {
          run.status = "running";
          run.attempts += 1;
          run.fraction = 0;
        }
      }
      if (run) {
        run.fraction = Math.min(1, (run.fraction ?? 0) + 0.25);
        run.message = STAGE_MESSAGES[run.stage];
        if (run.fraction >= 1) {
          run.status = "done";
          run.fraction = null;
          run.message = null;
        }
      }
      const finished = stages.every((s) => s.status === "done");
      return {
        ...j,
        stages,
        state: finished ? "done" : "running",
        waitingReason: null,
        progress: stagesProgress(stages),
      };
    });
    const j = jobs.find((x) => x.meetingId === id);
    if (j?.state === "done") {
      transcripts.set(id, demoTranscript(id));
      currentJob = null;
      emitMeetings();
    }
    emitQueue();
  }

  function active(id: string, offsetMs: number): ActiveSource {
    const src = demoSources.find((s) => s.id === id);
    if (!src) fail("source_not_found", `Unknown source ${id}`);
    return { id, label: src.label, kind: src.kind, status: "recording", offsetMs, level: 0.3 };
  }

  function begin(sourceIds: string[], elapsed = 0) {
    state = {
      status: "recording",
      meetingId: `m${meetings.length + 1}`,
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

  // Every subscription is its own entry, even when the same callback is registered twice
  // (React StrictMode does that): removing one must not remove the other.
  const subscribe = <T>(set: Set<{ cb: T }>, cb: T): Promise<Unsubscribe> => {
    const entry = { cb };
    set.add(entry);
    return Promise.resolve(() => void set.delete(entry));
  };

  return {
    listMeetings: async () => meetings.map(withJob),
    getMeeting: async (id): Promise<MeetingDetails> => {
      const found = meetings.find((x) => x.id === id);
      if (!found) fail("internal", "Meeting not found");
      const summary = withJob(found);
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
          state.meetingId ?? `m${meetings.length + 1}`,
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
    overlayImage: async () => {
      const svg =
        '<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="900"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#101a33"/><stop offset="1" stop-color="#3a1030"/></linearGradient></defs><rect width="1600" height="900" fill="url(#g)"/><g fill="#00f0ff" opacity=".5"><rect x="120" y="120" width="560" height="320"/><rect x="760" y="200" width="680" height="480"/></g></svg>';
      return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
    },
    overlayFinish: async (region) => {
      if (region && state.status === "recording") {
        state = { ...state, screenshotCount: state.screenshotCount + 1 };
        emit();
      }
    },
    processMeeting: async (id) => {
      const m = meetings.find((x) => x.id === id);
      if (!m) fail("internal", "Meeting not found");
      const existing = jobs.find((j) => j.meetingId === id);
      if (existing && existing.state !== "done")
        fail("already_queued", "This meeting is already queued or has a failed job");
      const fresh: JobView = {
        meetingId: id,
        title: m.title,
        state: "queued",
        progress: 0,
        waitingReason: null,
        stages: stageViews(),
      };
      jobs = existing ? jobs.map((j) => (j.meetingId === id ? fresh : j)) : [...jobs, fresh];
      transcripts.delete(id);
      enqueue(id);
      emitQueue();
      emitMeetings();
    },
    queueState: async () => ({ jobs: structuredClone(jobs) }),
    retryJob: async (id) => {
      const j = jobs.find((x) => x.meetingId === id);
      if (!j) fail("internal", "Job not found");
      if (j.state !== "failed") fail("internal", "Only a failed job can be retried");
      patchJob(id, (x) => ({
        ...x,
        state: "queued",
        stages: x.stages.map((s) =>
          s.status === "failed" ? { ...s, status: "pending", error: null } : s,
        ),
      }));
      enqueue(id);
      emitQueue();
      emitMeetings();
    },
    cancelJob: async (id) => {
      const j = jobs.find((x) => x.meetingId === id);
      if (!j) fail("internal", "Job not found");
      if (j.state === "running") fail("job_running", "A running job cannot be cancelled");
      jobs = jobs.filter((x) => x.meetingId !== id);
      const at = driven.indexOf(id);
      if (at >= 0) driven.splice(at, 1);
      emitQueue();
      emitMeetings();
    },
    getTranscript: async (id) => {
      const t = transcripts.get(id);
      if (!t) fail("not_ready", "This meeting has no transcript yet");
      return structuredClone(t);
    },
    editSegment: async (id, segmentId, edit) => {
      const t = transcripts.get(id);
      const seg = t?.segments.find((x) => x.id === segmentId);
      if (!seg) fail("internal", "Segment not found");
      if (edit.text === null) {
        if (seg.originalText !== null) seg.text = seg.originalText;
        seg.originalText = null;
      } else if (edit.text !== seg.text) {
        const original = seg.originalText ?? seg.text;
        seg.text = edit.text;
        seg.originalText = edit.text === original ? null : original;
      }
      seg.excluded = edit.excluded;
    },
    meetingImage: async (id, file) => {
      if (!transcripts.has(id)) fail("not_ready", "This meeting has no screenshots to show");
      return demoImage(file);
    },
    getProcessingSettings: async () => structuredClone(processing),
    saveProcessingSettings: async (s) => {
      // The token flag is owned by set/clear, as in the real application.
      Object.assign(processing, s, { hfTokenSet: processing.hfTokenSet });
    },
    setHfToken: async (token) => {
      if (token.trim() === "") fail("internal", "The token is empty");
      processing.hfTokenSet = true;
    },
    clearHfToken: async () => void (processing.hfTokenSet = false),
    modelStatus: async () => structuredClone(models),
    downloadModels: async (keys) => {
      for (const key of keys) {
        const model = models.find((m) => m.key === key);
        if (!model) fail("internal", `Unknown model ${key}`);
        if (model.gated && !processing.hfTokenSet)
          fail(
            "internal",
            "This model needs a Hugging Face token and accepted model terms. Store a token first.",
          );
      }
      for (const key of keys) {
        let step = 0;
        const timer = setInterval(() => {
          step += 1;
          const fraction = Math.min(1, step / 5);
          progressListeners.forEach((l) =>
            l.cb({ key, fraction, message: `Downloading ${Math.round(fraction * 100)}%` }),
          );
          if (fraction >= 1) {
            clearInterval(timer);
            const model = models.find((m) => m.key === key);
            if (model) model.cached = true;
            modelsListeners.forEach((l) => l.cb());
          }
        }, stepMs / 4);
      }
    },
    getSettings: async () => ({ ...settings }),
    saveSettings: async (s) => void Object.assign(settings, s),
    onRecordingState: (cb) => subscribe(stateListeners, cb),
    onMeetingsChanged: (cb) => subscribe(meetingListeners, cb),
    onQueueChanged: (cb) => subscribe(queueListeners, cb),
    onModelsProgress: (cb) => subscribe(progressListeners, cb),
    onModelsChanged: (cb) => subscribe(modelsListeners, cb),
  };
}
