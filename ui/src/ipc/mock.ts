import type { Backend, Unsubscribe } from "./backend";
import { stagesProgress } from "../processing";
import {
  STAGE_MESSAGES,
  demoImage,
  demoJobs,
  demoModels,
  demoParticipants,
  demoProcessingSettings,
  demoTranscript,
  demoVoices,
  stageViews,
  toneWav,
} from "./mock-demo";
import { createCycleStore, type RecordingReport } from "./mock-cycles";
import { createMockDrafts } from "./mock-drafts";
import { createMockPublishing, type MockHooks } from "./mock-publish";
import { fail } from "./mock-util";
import { isLocalHost, missingForKind, urlHost, validateLlmSettings } from "../llm";
import { validateVoiceSettings } from "../speakers";
import type {
  ActiveSource,
  CloudPreview,
  JobView,
  LlmKind,
  MeetingDetails,
  MeetingSummary,
  ModelsProgress,
  ParticipantView,
  ProcessOptions,
  QueueState,
  RecordingState,
  Settings,
  SourceInfo,
  StageName,
  TranscriptData,
  VoiceView,
  VoicesData,
} from "./types";

const MIN = 60_000;

/** What a job does in the language-model stages: run with a backend or skip them with a note. */
interface LlmPlan {
  kind: LlmKind | null;
  skip: string | null;
}
const NO_BACKEND: LlmPlan = {
  kind: null,
  skip: "no language model backend was chosen for this meeting",
};
const LLM_STAGES: StageName[] = ["extract", "reduce"];

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
  /** Start with a newer draft next to the edited one of the demo meeting (the banner). */
  newDraft?: boolean;
  /** Start with the network off: publishing waits until it is back. */
  offline?: boolean;
}

/** In-memory backend that serves the fictional demo cycle. No audio devices, files or network. */
export function createMockBackend(opts: MockOptions = {}): Backend {
  const startedAt = Date.now();
  const settings: Settings = {
    recordingsDir: "Documents\\BlahBlahBot\\recordings",
    recordHotkey: "Ctrl+Alt+R",
    screenshotHotkey: "Ctrl+Alt+S",
  };
  // ----- cycles, report settings and drafts -----
  const cycles = createCycleStore();
  /** What each meeting was recorded with; a meeting without an entry has no cycle. */
  const reports = new Map<string, RecordingReport>([
    [
      "m5",
      { cycleId: "NOMAD", mode: "auto", template: null, meetingType: null, outputLanguage: null },
    ],
  ]);
  const cycleListeners = new Set<{ cb: () => void }>();
  const emitCycles = () => cycleListeners.forEach((l) => l.cb());
  const cycleOf = (meetingId: string): string | null => reports.get(meetingId)?.cycleId ?? null;
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
  const lock: { is: (meetingId: string) => boolean } = { is: () => false };
  const drafts = createMockDrafts(
    cycles,
    (id) => transcripts.has(id),
    (id) => lock.is(id),
  );
  const publishing = createMockPublishing({
    cycles,
    drafts,
    stepMs,
    now: () => DEMO_NOW + (Date.now() - startedAt),
    screenshotCount: (id) => meetings.find((m) => m.id === id)?.screenshotCount ?? 0,
    onUnlocked: (id) => drafts.touch(id),
  });
  lock.is = publishing.isLocked;
  if (opts.offline) publishing.hooks.setOffline(true);
  (globalThis as { __bbbMock?: MockHooks }).__bbbMock = publishing.hooks;
  drafts.create("m5", reports.get("m5")!);
  if (opts.newDraft) drafts.leaveNewDraft("m5");
  /** The backend each job was started with; a job without an entry has none. */
  const plans = new Map<string, LlmPlan>();
  /** Meetings the user confirmed sending to the cloud endpoint, with the host confirmed. */
  const confirmed = new Map<string, string>();
  // ----- speakers and voice profiles -----
  const keyOf = (cycleId: string, id: string) => `${cycleId}:${id}`;
  const demoVoicesOf = (cycleId: string | null): VoiceView[] =>
    demoVoices((k) =>
      cycleId === null
        ? `p-${k}`
        : cycles.participants(cycleId).some((p) => p.id === k)
          ? keyOf(cycleId, k)
          : null,
    );
  const voices = new Map<string, VoiceView[]>([["m5", demoVoicesOf("NOMAD")]]);
  /** Legacy participants (named before cycles existed). */
  let participants = demoParticipants();
  let nextParticipant = 1;
  /** Voice profile figures of cycle participants, by `CYCLE:id`. */
  const stats = new Map<string, { sampleCount: number; speechMs: number }>([
    [keyOf("NOMAD", "alex"), { sampleCount: 4, speechMs: 18 * MIN + 20_000 }],
    [keyOf("NOMAD", "jan"), { sampleCount: 2, speechMs: 7 * MIN + 45_000 }],
    [keyOf("NOMAD", "kasia"), { sampleCount: 3, speechMs: 11 * MIN + 5_000 }],
  ]);
  /** The participants of a cycle as the voice screens see them. */
  const cycleViews = (cycleId: string): ParticipantView[] =>
    cycles
      .participants(cycleId)
      .map((p) => ({
        id: keyOf(cycleId, p.id),
        name: p.name,
        cycleId,
        cycleParticipantId: p.id,
        sampleCount: stats.get(keyOf(cycleId, p.id))?.sampleCount ?? 0,
        speechMs: stats.get(keyOf(cycleId, p.id))?.speechMs ?? 0,
      }))
      .sort((a, b) => a.name.localeCompare(b.name));
  /** `cycleId`, else the cycle of `meetingId`, else the legacy list. */
  const participantsFor = (cycleId?: string, meetingId?: string): ParticipantView[] => {
    const cycle = cycleId ?? (meetingId ? cycleOf(meetingId) : null);
    return cycle ? cycleViews(cycle) : structuredClone(participants);
  };
  /** The cycle and participant id inside a stored participant key, when it is one. */
  const splitKey = (key: string): { cycleId: string; id: string } | null => {
    const at = key.indexOf(":");
    return at > 0 ? { cycleId: key.slice(0, at), id: key.slice(at + 1) } : null;
  };

  const voicesOf = (id: string): VoiceView[] => voices.get(id) ?? [];
  const findVoice = (id: string, voiceId: string): VoiceView => {
    const v = voicesOf(id).find((x) => x.voiceId === voiceId);
    if (!v) fail("not_found", "voice not found");
    return v;
  };
  const cleanName = (name: string): string => {
    const n = name.trim();
    if (n === "" || [...n].length > 80) fail("invalid", "enter a name of 1 to 80 characters");
    return n;
  };
  const sameName = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();
  /** Like the backend: a meeting whose voices are all decided loses its sample clips. */
  function voicesData(id: string): VoicesData {
    const list = voicesOf(id);
    const allConfirmed = list.length > 0 && list.every((v) => v.confirmed || v.mergedInto);
    if (allConfirmed) list.forEach((v) => (v.clips = []));
    return {
      meetingId: id,
      voices: structuredClone(list),
      participants: participantsFor(undefined, id),
      allConfirmed,
    };
  }
  function assign(id: string, voiceId: string, participantId: string | null): VoicesData {
    const v = findVoice(id, voiceId);
    if (
      participantId !== null &&
      !participantsFor(undefined, id).some((p) => p.id === participantId)
    )
      fail("not_found", "participant not found");
    v.assignedParticipantId = participantId;
    v.confirmed = true;
    return voicesData(id);
  }
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
        const skip =
          run && LLM_STAGES.includes(run.stage) ? (plans.get(id) ?? NO_BACKEND).skip : null;
        if (run && skip !== null) {
          // Skipped stages finish at once, with the reason.
          run.status = "done";
          run.note = skip;
          run = undefined;
        } else if (run) {
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
      if (!transcripts.has(id)) {
        transcripts.set(id, demoTranscript(id));
        voices.set(id, demoVoicesOf(cycleOf(id)));
      }
      // `reduce` wrote a draft when the backend ran for a meeting with a cycle.
      const report = reports.get(id);
      if (report?.cycleId && (plans.get(id) ?? NO_BACKEND).skip === null && !drafts.has(id))
        drafts.create(id, report);
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

  const llm = () => processing.llm!;

  /**
   * The backend a job starts with (`choose_llm` in `llm.rs`): a meeting without a cycle skips the
   * language-model stages; without an explicit choice an unconfigured default skips them too; an
   * explicit choice that is not configured is an error; the cloud needs its key and the user's
   * confirmation for this meeting and endpoint.
   */
  function planLlm(id: string, requested: LlmKind | undefined): LlmPlan {
    if (cycleOf(id) === null) return { kind: null, skip: "the meeting has no cycle" };
    const settings = llm();
    const kind = requested ?? settings.defaultKind;
    const missing = missingForKind(settings, kind);
    if (missing) {
      if (requested === undefined) return NO_BACKEND;
      fail("llm_not_configured", `the ${kind} language model has no ${missing} in the settings`);
    }
    if (kind === "cloud") {
      if (!settings.cloudKeySet)
        fail("cloud_key_missing", "store the API key of the cloud endpoint in the settings first");
      if (confirmed.get(id) !== urlHost(settings.cloud.baseUrl))
        fail("cloud_not_confirmed", "confirm sending this meeting to the cloud endpoint first");
    }
    return { kind, skip: null };
  }

  function cloudHost(): string {
    const missing = missingForKind(llm(), "cloud");
    if (missing)
      fail("llm_not_configured", `the cloud language model has no ${missing} in the settings`);
    return (
      urlHost(llm().cloud.baseUrl) ?? fail("llm_not_configured", "the cloud address has no host")
    );
  }

  /** A backend chosen for a job that exists: the skipped or failed extraction stages run again. */
  function applyLlmToJob(id: string, plan: LlmPlan) {
    const job = jobs.find((j) => j.meetingId === id)!;
    const running = job.stages.some((s) => LLM_STAGES.includes(s.stage) && s.status === "running");
    if (running) fail("busy", "the report is being extracted; wait for it to finish");
    plans.set(id, plan);
    patchJob(id, (j) => {
      const stages = j.stages.map((s) =>
        LLM_STAGES.includes(s.stage) && (s.status === "failed" || (s.status === "done" && s.note))
          ? { ...s, status: "pending" as const, note: null, error: null }
          : s,
      );
      const again = stages.some((s, i) => s !== j.stages[i]);
      return {
        ...j,
        stages,
        state: again ? "queued" : j.state,
        progress: stagesProgress(stages),
      };
    });
    if (jobs.find((j) => j.meetingId === id)?.state === "queued") enqueue(id);
  }

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
      // The gate comes first, like in `start_recording`: no cycle and mode, no recording.
      const report = cycles.resolveChoice(req.report, req.useLastReport ?? false);
      if (state.status === "recording") fail("already_recording", "Already recording");
      begin(req.sourceIds);
      if (state.meetingId !== null) reports.set(state.meetingId, report);
      cycles.setLastReport({
        cycleId: report.cycleId!,
        mode: report.mode!,
        template: report.template,
        meetingType: report.meetingType,
      });
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
    processMeeting: async (id, options?: ProcessOptions) => {
      const m = meetings.find((x) => x.id === id);
      if (!m) fail("internal", "Meeting not found");
      const plan = planLlm(id, options?.llm?.kind);
      const existing = jobs.find((j) => j.meetingId === id);
      if (existing && options?.llm) {
        // A backend chosen now for a job that exists reruns what was skipped or failed.
        applyLlmToJob(id, plan);
        emitQueue();
        emitMeetings();
        return;
      }
      if (existing && existing.state !== "done")
        fail("already_queued", "This meeting is already queued or has a failed job");
      plans.set(id, plan);
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
      voices.delete(id);
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
    getMeetingVoices: async (id) => voicesData(id),
    assignVoice: async (id, voiceId, participantId) => assign(id, voiceId, participantId),
    assignVoiceNew: async (id, voiceId, name) => {
      const n = cleanName(name);
      findVoice(id, voiceId);
      const cycleId = cycleOf(id);
      if (cycleId) {
        const key = keyOf(cycleId, cycles.addParticipant(cycleId, n));
        emitCycles();
        return assign(id, voiceId, key);
      }
      let p = participants.find((x) => sameName(x.name, n));
      if (!p) {
        p = { id: `p-new-${nextParticipant++}`, name: n, sampleCount: 0, speechMs: 0 };
        participants = [...participants, p].sort((a, b) => a.name.localeCompare(b.name));
      }
      return assign(id, voiceId, p.id);
    },
    mergeVoices: async (id, voiceId, into) => {
      const v = findVoice(id, voiceId);
      if (into !== null) {
        const list = voicesOf(id);
        const bad = () =>
          fail("invalid", "these voices cannot be merged (unknown voice, itself, or a loop)");
        if (into === voiceId || !list.some((x) => x.voiceId === into)) bad();
        // Walking up from the target must not reach this voice.
        let current: string | null | undefined = into;
        for (let i = 0; i <= list.length && current; i += 1) {
          if (current === voiceId) bad();
          current = list.find((x) => x.voiceId === current)?.mergedInto;
        }
      }
      v.mergedInto = into;
      return voicesData(id);
    },
    confirmMatches: async (id) => {
      for (const v of voicesOf(id))
        if (v.decision === "auto" && !v.confirmed && v.assignedParticipantId) v.confirmed = true;
      return voicesData(id);
    },
    voiceClip: async (id, voiceId, idx) => {
      const v = findVoice(id, voiceId);
      const c = v.clips.find((x) => x.idx === idx);
      if (!c) fail("not_found", "clip not found");
      const pitch = 220 + 110 * voicesOf(id).indexOf(v);
      const seconds = Math.min(4, Math.max(1.5, (c.endMs - c.startMs) / 1000));
      return URL.createObjectURL(toneWav(pitch, seconds));
    },
    listParticipants: async (cycleId, meetingId) => participantsFor(cycleId, meetingId),
    createParticipant: async (name, cycleId) => {
      const n = cleanName(name);
      if (cycleId) {
        if (cycleViews(cycleId).some((x) => sameName(x.name, n)))
          fail("duplicate", "a participant with this name already exists");
        const id = cycles.addParticipant(cycleId, n);
        emitCycles();
        return cycleViews(cycleId).find((x) => x.id === keyOf(cycleId, id))!;
      }
      if (participants.some((x) => sameName(x.name, n)))
        fail("duplicate", "a participant with this name already exists");
      const p = { id: `p-new-${nextParticipant++}`, name: n, sampleCount: 0, speechMs: 0 };
      participants = [...participants, p].sort((a, b) => a.name.localeCompare(b.name));
      return structuredClone(p);
    },
    renameParticipant: async (participantId, name) => {
      const n = cleanName(name);
      const inCycle = splitKey(participantId);
      if (inCycle) {
        cycles.renameParticipant(inCycle.cycleId, inCycle.id, n);
        emitCycles();
        return;
      }
      const p = participants.find((x) => x.id === participantId);
      if (!p) fail("not_found", "participant not found");
      if (participants.some((x) => x.id !== participantId && sameName(x.name, n)))
        fail("duplicate", "a participant with this name already exists");
      p.name = n;
      participants.sort((a, b) => a.name.localeCompare(b.name));
    },
    deleteParticipant: async (participantId) => {
      const inCycle = splitKey(participantId);
      if (inCycle) {
        cycles.removeParticipant(inCycle.cycleId, inCycle.id);
        stats.delete(participantId);
        emitCycles();
      }
      participants = participants.filter((p) => p.id !== participantId);
      for (const list of voices.values())
        for (const v of list) {
          if (v.assignedParticipantId === participantId) v.assignedParticipantId = null;
          if (v.suggestedParticipantId === participantId) v.suggestedParticipantId = null;
        }
    },
    resetProfile: async (participantId) => {
      stats.delete(participantId);
      const p = participants.find((x) => x.id === participantId);
      if (p) {
        p.sampleCount = 0;
        p.speechMs = 0;
      }
    },
    deleteVoiceData: async () => {
      // Participants stay; profiles, detected voices and clips go.
      voices.clear();
      stats.clear();
      participants = participants.map((p) => ({ ...p, sampleCount: 0, speechMs: 0 }));
    },
    getProcessingSettings: async () => {
      const s = structuredClone(processing);
      if (s.llm) {
        const host = urlHost(s.llm.external.baseUrl);
        s.llm.externalHostIsRemote =
          s.llm.defaultKind === "external" && host !== null && !isLocalHost(host);
      }
      return s;
    },
    saveProcessingSettings: async (s) => {
      const invalid = validateVoiceSettings(s) ?? (s.llm ? validateLlmSettings(s.llm) : null);
      if (invalid) fail("invalid", invalid);
      // The token and key flags are owned by set/clear, as in the real application.
      const flags = { externalKeySet: llm().externalKeySet, cloudKeySet: llm().cloudKeySet };
      Object.assign(processing, s, { hfTokenSet: processing.hfTokenSet });
      processing.llm = { ...(s.llm ?? llm()), ...flags };
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
    getSettings: async () => ({
      ...settings,
      cycleRepos: cycles.listRepos().map((r) => ({
        id: r.id,
        path: r.path,
        remote: r.remote,
        kind: r.kind,
        githubRepo: r.githubRepo ?? null,
        githubApi: r.githubApi ?? null,
      })),
      lastReport: cycles.lastReport(),
    }),
    // The repository list and the last report are not saved from the settings screen.
    saveSettings: async (s) =>
      void Object.assign(settings, {
        recordingsDir: s.recordingsDir,
        recordHotkey: s.recordHotkey,
        screenshotHotkey: s.screenshotHotkey,
      }),
    onRecordingState: (cb) => subscribe(stateListeners, cb),
    onMeetingsChanged: (cb) => subscribe(meetingListeners, cb),
    onQueueChanged: (cb) => subscribe(queueListeners, cb),
    onModelsProgress: (cb) => subscribe(progressListeners, cb),
    onModelsChanged: (cb) => subscribe(modelsListeners, cb),

    // ----- language-model keys and the cloud confirmation -----
    setCloudKey: async (key) => {
      if (key.trim() === "") fail("internal", "The key is empty");
      llm().cloudKeySet = true;
    },
    clearCloudKey: async () => void (llm().cloudKeySet = false),
    cloudKeyPresent: async () => llm().cloudKeySet,
    setExternalKey: async (key) => {
      if (key.trim() === "") fail("internal", "The key is empty");
      llm().externalKeySet = true;
    },
    clearExternalKey: async () => void (llm().externalKeySet = false),
    externalKeyPresent: async () => llm().externalKeySet,
    cloudPreview: async (meetingId): Promise<CloudPreview> => {
      if (!meetings.some((x) => x.id === meetingId)) fail("not_found", "meeting not found");
      const host = cloudHost();
      const model = llm().cloud.model.trim();
      const transcript = transcripts.get(meetingId);
      const transcriptReady = transcript !== undefined;
      const wordCount = (transcript?.segments ?? [])
        .filter((x) => !x.excluded)
        .reduce((n, x) => n + x.text.split(/\s+/).filter(Boolean).length, 0);
      const screenshotDescriptions = transcript?.screenshots.length ?? 0;
      const cycleId = cycleOf(meetingId);
      const previousItemCount = Math.min(
        30,
        cycleId === null ? 0 : drafts.itemsOfCycle(cycleId, meetingId),
      );
      const previous =
        previousItemCount === 0
          ? "no items"
          : previousItemCount === 1
            ? "1 item"
            : `up to ${previousItemCount} items`;
      const amount = transcriptReady
        ? `the text of the transcript (${wordCount} words; passages you excluded are left out) and ${screenshotDescriptions} screenshot descriptions`
        : "the text of the transcript and the screenshot descriptions (the amount is known once the meeting is transcribed)";
      return {
        endpointHost: host,
        model,
        wordCount,
        screenshotDescriptions,
        previousItemCount,
        includesAudio: false,
        includesImages: false,
        summaryText: `This will send ${amount} to ${host} (model ${model}), together with the names, glossary, instruction and open topics of the cycle, and ${previous} (text, assignee and due date) from earlier meetings of the cycle, including meetings you processed only locally. No audio and no images are sent. Nothing is sent without this confirmation.`,
        confirmed: confirmed.get(meetingId) === host,
        transcriptReady,
      };
    },
    confirmCloud: async (meetingId) => {
      const host = cloudHost();
      if (!meetings.some((x) => x.id === meetingId)) fail("not_found", "meeting not found");
      confirmed.set(meetingId, host);
    },

    // ----- cycles -----
    listCycles: async () => cycles.listCycles(),
    listCycleRepos: async () => cycles.listRepos(),
    getCycle: async (cycleId) => cycles.getCycle(cycleId),
    createCycle: async (id, name, repoId) => {
      const details = cycles.createCycle(id, name, repoId);
      emitCycles();
      return details;
    },
    saveCycle: async (cycle) => {
      const result = cycles.saveCycle(cycle);
      if (result.saved) emitCycles();
      return result;
    },
    listTemplates: async (cycleId) => cycles.listTemplates(cycleId),
    getTemplate: async (cycleId, name) => cycles.getTemplate(cycleId, name),
    saveTemplate: async (cycleId, template) => {
      const check = cycles.saveTemplate(cycleId, template);
      if (check.saved) emitCycles();
      return check;
    },
    validateTemplate: async (template) => cycles.validateTemplate(template),
    addCycleRepo: async (req) => {
      const repo = cycles.addRepo(req);
      emitCycles();
      return repo;
    },
    pullCycleRepo: async (repoId) => {
      const result = cycles.pullRepo(repoId);
      // A pull also reads the state of the issues again.
      publishing.refreshIssues(repoId);
      emitCycles();
      return result;
    },
    reportDefaults: async (cycleId) => cycles.reportDefaults(cycleId),
    onCyclesChanged: (cb) => subscribe(cycleListeners, cb),

    // ----- drafts -----
    ...drafts.methods,

    // ----- publishing -----
    ...publishing.methods,
  };
}
