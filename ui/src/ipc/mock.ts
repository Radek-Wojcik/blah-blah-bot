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
import { createMockBridge } from "./mock-bridge";
import { createMockBriefing } from "./mock-briefing";
import { createMockDetection } from "./mock-detection";
import { createMockDrafts } from "./mock-drafts";
import { createMockPublishing, type MockHooks } from "./mock-publish";
import { mockCheckSetupPath, mockGgufModels, mockSetupStatus } from "./mock-readiness";
import { applySetupScenario, type SetupScenario } from "./mock-setup";
import { fail } from "./mock-util";
import { createMockPicker, type MockPickResults } from "./mock-pick";
import { isLocalHost, missingForKind, urlHost, validateLlmSettings } from "../llm";
import {
  DEFAULT_RECORD_HOTKEY,
  DEFAULT_SCREENSHOT_HOTKEY,
  normalizeHotkey,
  validateHotkey,
} from "../hotkeys";
import { validateVoiceSettings } from "../speakers";
import type {
  ActiveSource,
  CloudPreview,
  CycleDto,
  DetectedModels,
  GlobalVoiceProfile,
  ImportMeetingInput,
  JobView,
  LanguageSetting,
  LlmKind,
  MeetingDetails,
  MeetingSummary,
  ModelsProgress,
  ParticipantView,
  ProcessOptions,
  ProjectPrefs,
  QueueGpuOverride,
  QueueState,
  RecordingLevels,
  RecordingState,
  ReportMode,
  HotkeyStatus,
  Settings,
  SetupProgress,
  SourceInfo,
  StageName,
  TranscriptData,
  VoiceView,
  VoicesData,
  WizardCreateProjectInput,
  WizardCreateResult,
  GithubFallbackReason,
  GithubRepoRequest,
  GithubRepoStatus,
} from "./types";

const MIN = 60_000;
/** The login of the fictional GitHub account the mock creates repositories under. */
const MOCK_GITHUB_LOGIN = "demo-user";

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
  { id: "app:chrome", kind: "browser", label: "Chrome (whole browser)", active: true },
];

/** The hints of the whole browser, as the application words them. */
const HINT_NOT_CONNECTED =
  "The browser extension is not connected. This records all sound from Chrome instead of single tabs.";
const HINT_TAB_FAILED =
  "A tab could not be captured. This records all sound from Chrome, including that tab.";
const HINT_TABS_OK = "Records all sound from Chrome. Choose tabs instead to leave the others out.";

/** Fictional tabs the extension has captured (`tabs=1` in the address). */
const demoTabs: SourceInfo[] = [
  {
    id: "tab:s1",
    kind: "browser_tab",
    label: "Tab: ORBIT weekly sync (meet.example.org)",
    active: true,
    group: "Group 1",
  },
  {
    id: "tab:s2",
    kind: "browser_tab",
    label: "Tab: Thruster plan (docs.example.org)",
    active: true,
    group: "Group 1",
  },
  {
    id: "tab:s3",
    kind: "browser_tab",
    label: "Tab: Budget sheet (sheets.example.org)",
    active: true,
  },
];

function meeting(
  id: string,
  title: string,
  startedUnixMs: number,
  durationMs: number,
  state: MeetingSummary["state"],
  trackCount: number,
  screenshotCount: number,
  shortTitle: string | null = null,
  reportUrl: string | null = null,
  cycleId: string | null = null,
): MeetingSummary {
  return {
    id,
    title,
    startedUnixMs,
    durationMs,
    state,
    trackCount,
    screenshotCount,
    shortTitle,
    reportUrl,
    cycleId,
  };
}

const demoMeetings: MeetingSummary[] = [
  meeting(
    "m5",
    "ORBIT weekly sync",
    DEMO_NOW - 2 * 3600_000,
    47 * MIN,
    "stopped",
    3,
    4,
    "Thruster test 7 pressure flat, budget 2% under",
    "https://github.com/demo-org/nomad-cycles/blob/main/meetings/2026-11-13-m5/report.md",
    "NOMAD",
  ),
  meeting(
    "m4",
    "Thruster review",
    DEMO_NOW - 26 * 3600_000,
    82 * MIN,
    "stopped",
    2,
    1,
    "Chamber valve ripple filter and second test stand",
    null,
    null,
  ),
  meeting(
    "m3",
    "Budget check-in",
    DEMO_NOW - 50 * 3600_000,
    31 * MIN,
    "stopped",
    2,
    0,
    "Q4 procurement underspend and battery order",
    null,
    null,
  ),
  meeting(
    "m2",
    "Ground station planning",
    DEMO_NOW - 98 * 3600_000,
    64 * MIN,
    "stopped",
    3,
    7,
    "Antenna array mount and Thursday dry run schedule",
    "https://github.com/demo-org/nomad-cycles/blob/main/meetings/2026-11-09-m2/report.md",
    null,
  ),
  meeting(
    "m1",
    "Kickoff",
    DEMO_NOW - 170 * 3600_000,
    95 * MIN,
    "recovered",
    3,
    2,
    "Nomad rover subsystem roles and CAN bus architecture",
    "https://github.com/demo-org/nomad-cycles/blob/main/meetings/2026-11-06-m1/report.md",
    null,
  ),
];

function wavBytes(freqHz: number, seconds: number, sampleRate = 8000): number[] {
  const samples = Math.round(sampleRate * seconds);
  const buf = new ArrayBuffer(44 + samples * 2);
  const v = new DataView(buf);
  const text = (at: number, s: string) =>
    [...s].forEach((c, i) => v.setUint8(at + i, c.charCodeAt(0)));
  text(0, "RIFF");
  v.setUint32(4, 36 + samples * 2, true);
  text(8, "WAVE");
  text(12, "fmt ");
  v.setUint32(16, 16, true);
  v.setUint16(20, 1, true);
  v.setUint16(22, 1, true);
  v.setUint32(24, sampleRate, true);
  v.setUint32(28, sampleRate * 2, true);
  v.setUint16(32, 2, true);
  v.setUint16(34, 16, true);
  text(36, "data");
  v.setUint32(40, samples * 2, true);
  const fade = Math.max(1, Math.round(sampleRate * 0.03));
  for (let i = 0; i < samples; i += 1) {
    const gain = Math.min(1, i / fade, (samples - i) / fade);
    const x = Math.sin((2 * Math.PI * freqHz * i) / sampleRate) * 0.25 * gain;
    v.setInt16(44 + i * 2, Math.round(x * 32767), true);
  }
  return Array.from(new Uint8Array(buf));
}

function computeMockLevels(elapsedMs: number, activeSources: ActiveSource[]): RecordingLevels {
  const live = activeSources.filter((s) => s.status === "recording");
  if (live.length === 0) {
    return { peak: 0, rms: 0, bars: Array<number>(12).fill(0), peakHold: 0 };
  }
  const bars = Array.from({ length: 12 }, (_, i) => {
    const wave =
      0.18 +
      0.48 * Math.abs(Math.sin(elapsedMs / 240 + i * 0.55)) +
      0.24 * Math.abs(Math.cos(elapsedMs / 150 - i * 0.4));
    return Math.min(1, Math.max(0.06, Number(wave.toFixed(3))));
  });
  const peak = Math.max(...bars);
  const rms = Number(Math.sqrt(bars.reduce((acc, b) => acc + b * b, 0) / bars.length).toFixed(3));
  const peakHold = Math.min(1, Number((peak + 0.06).toFixed(3)));
  return { peak, rms, bars, peakHold };
}

export interface MockOptions {
  /** Answers of the file dialogs (a path, or null for a cancelled dialog); default fake paths otherwise. */
  pick?: MockPickResults;
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
  /**
   * Make the screenshot overlay fail the way a bad capture would: `blank` refuses the image
   * like the real backend does, `black` serves an all-black picture, `broken` serves bytes that
   * are no image.
   */
  overlayFault?: "blank" | "black" | "broken";
  /**
   * A setup scenario for the first-run wizard (see `mock-setup.ts`). Without it the mock
   * reports a finished setup and does not check it before processing.
   */
  setup?: SetupScenario;
  /** Simulate an extension entering the pairing code this many milliseconds after pairing starts. */
  bridgePairMs?: number;
  /** Raise a prompt for a fictional Discord call this many milliseconds after start. */
  detectMs?: number;
  /**
   * The extension has captured tabs: `ok`, or `failed` when a tab of the group could not be
   * captured. Without it the extension is not connected and only the whole browser is offered.
   */
  browserTabs?: "ok" | "failed";
  /** Start without a saved GitHub token: the wizard falls back to a local-only project. */
  noGithubToken?: boolean;
  /**
   * Make creating a GitHub repository fail the way GitHub would: `name-taken` is a 422,
   * `offline` a lost connection. The wizard then falls back to a local-only project.
   */
  githubCreateFault?: "name-taken" | "offline";
  /** Hotkeys simulated as already taken by another application. */
  takenHotkeys?: string[];
  /** Called when the UI suspends (true) or resumes (false) the global hotkeys. */
  onHotkeysSuspended?: (suspended: boolean) => void;
}

/** In-memory backend that serves the fictional demo cycle. No audio devices, files or network. */
export function createMockBackend(opts: MockOptions = {}): Backend {
  const startedAt = Date.now();
  /** Mirrors the application: hotkeys are unregistered while the UI records a shortcut. */
  let hotkeysSuspended = false;
  const setSuspended = (next: boolean) => {
    if (hotkeysSuspended === next) return;
    hotkeysSuspended = next;
    opts.onHotkeysSuspended?.(next);
  };
  const settings: Settings = {
    recordingsDir: "Documents\\BlahBlahBot\\recordings",
    recordHotkey: DEFAULT_RECORD_HOTKEY,
    screenshotHotkey: DEFAULT_SCREENSHOT_HOTKEY,
    mouseButtons: "normal",
    localSaveEnabled: false,
    localSaveDir: "",
    lastSourceKeys: [],
  };
  const projectPrefs = new Map<string, ProjectPrefs>([
    [
      "NOMAD",
      {
        cycleId: "NOMAD",
        localSaveEnabled: false,
        localSaveDir: "Documents\\BlahBlahBot\\notes",
        llmKindOverride: null,
        llmModelOverride: null,
        audioRetention: null,
      },
    ],
  ]);
  // ----- the bridge to the Chrome extension -----
  const bridge = createMockBridge({
    now: () => Date.now(),
    demoPairedUnixMs: DEMO_NOW - 3 * 24 * 3600_000,
    ...(opts.bridgePairMs === undefined ? {} : { pairAfterMs: opts.bridgePairMs }),
  });
  // ----- cycles, report settings and drafts -----
  const cycles = createCycleStore();
  const briefing = createMockBriefing(cycles);
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
  const levelsListeners = new Set<{ cb: (l: RecordingLevels) => void }>();
  const meetingListeners = new Set<{ cb: () => void }>();
  let timer: ReturnType<typeof setInterval> | null = null;
  let levelsTimer: ReturnType<typeof setInterval> | null = null;
  const rate = opts.clockRate ?? 1000;

  // ----- processing pipeline -----
  const stepMs = Math.max(40, opts.jobStepMs ?? 1200);
  const processing = demoProcessingSettings();
  const models = demoModels();
  const world = applySetupScenario(opts.setup, processing, models);
  // The steps of a setup scenario are slower than a queue tick so that their progress can be seen.
  const setupMs = world.active ? Math.max(stepMs / 4, 60) : stepMs / 4;
  let preparing = false;
  /** The readiness report; without a scenario the demo setup counts as finished. */
  const readiness = () =>
    mockSetupStatus(
      processing,
      world.active ? models : models.map((m) => ({ ...m, cached: true })),
      world.llamaCandidates,
      world.workerEnv,
      preparing ? "workerEnv" : null,
    );
  let jobs: JobView[] = demoJobs().map((j) => ({
    ...j,
    scheduledUnixMs: null,
    etaSeconds: j.state === "running" ? 42 : null,
    // A finished demo job reports when it finished, like the real queue does.
    finishedUnixMs: j.state === "done" ? Date.now() : null,
  }));
  let gpuOverride: QueueGpuOverride | null = null;
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
  if (opts.noGithubToken) void publishing.methods.clearGithubToken();
  (globalThis as { __bbbMock?: MockHooks }).__bbbMock = publishing.hooks;
  drafts.create("m5", reports.get("m5")!);
  if (opts.newDraft) drafts.leaveNewDraft("m5");
  /** The backend each job was started with; a job without an entry has none. */
  const plans = new Map<string, LlmPlan>();
  /** Pre-schedule state and waiting reason per job so clearing a schedule restores them. */
  const preSchedule = new Map<string, { state: JobView["state"]; waitingReason: string | null }>();
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
  const extraProjects = new Map<string, Set<string>>();
  const removedProjects = new Map<string, Set<string>>();
  const mergedDeleted = new Set<string>();
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
  const participantDisplayName = (pid: string | null | undefined): string | null => {
    if (!pid) return null;
    const local = participants.find((p) => p.id === pid);
    if (local) return local.name;
    const inCycle = splitKey(pid);
    if (inCycle) {
      return cycles.participants(inCycle.cycleId).find((p) => p.id === inCycle.id)?.name ?? null;
    }
    return null;
  };
  /** Like the backend: a meeting whose voices are all decided loses its sample clips. */
  function voicesData(id: string): VoicesData {
    const list = voicesOf(id);
    const allConfirmed = list.length > 0 && list.every((v) => v.confirmed || v.mergedInto);
    if (allConfirmed) list.forEach((v) => (v.clips = []));
    return {
      meetingId: id,
      cycleId: cycleOf(id),
      voices: structuredClone(list).map((v) => ({
        ...v,
        assignedParticipantName:
          v.assignedParticipantName ?? participantDisplayName(v.assignedParticipantId),
      })),
      participants: participantsFor(undefined, id),
      allConfirmed,
    };
  }
  function assign(id: string, voiceId: string, participantId: string | null): VoicesData {
    const v = findVoice(id, voiceId);
    if (participantId !== null) {
      const knownInMeeting = participantsFor(undefined, id).some((p) => p.id === participantId);
      const knownGlobal = participants.some((p) => p.id === participantId);
      const knownOtherCycle = cycles
        .listCycles()
        .some((c) => cycles.participants(c.id).some((p) => keyOf(c.id, p.id) === participantId));
      if (!knownInMeeting && !knownGlobal && !knownOtherCycle) {
        fail("not_found", "participant not found");
      }
      const cid = cycleOf(id);
      if (cid && !knownInMeeting) {
        const extra = extraProjects.get(participantId) ?? new Set<string>();
        extra.add(cid);
        extraProjects.set(participantId, extra);
      }
    }
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
  const setupListeners = new Set<{ cb: (p: SetupProgress) => void }>();

  const emit = () => {
    stateListeners.forEach((l) => l.cb(structuredClone(state)));
    const levels =
      state.status === "recording"
        ? computeMockLevels(state.elapsedMs, state.sources)
        : { peak: 0, rms: 0, bars: Array<number>(12).fill(0) };
    levelsListeners.forEach((l) => l.cb(levels));
  };
  const emitMeetings = () => meetingListeners.forEach((l) => l.cb());
  const queueSnapshot = (): QueueState => ({
    jobs: structuredClone(jobs),
    gpuOverride: structuredClone(gpuOverride),
  });
  const emitQueue = () => queueListeners.forEach((l) => l.cb(queueSnapshot()));

  const withJob = (m: MeetingSummary): MeetingSummary => ({
    ...m,
    job: jobs.find((j) => j.meetingId === m.id)?.state ?? null,
    cycleId: m.cycleId !== undefined ? m.cycleId : cycleOf(m.id),
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
      const progress = stagesProgress(stages);
      return {
        ...j,
        stages,
        state: finished ? "done" : "running",
        finishedUnixMs: finished ? (j.finishedUnixMs ?? Date.now()) : null,
        waitingReason: null,
        progress,
        scheduledUnixMs: finished ? null : (j.scheduledUnixMs ?? null),
        etaSeconds: finished ? null : Math.max(1, Math.round((1 - progress) * 60)),
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
      if (
        driven.length === 0 ||
        !jobs.some((x) => x.state === "running" || x.state === "queued" || x.state === "waiting")
      ) {
        gpuOverride = null;
      }
      emitMeetings();
    }
    emitQueue();
  }

  /** The sources the picker offers, with the hint of the whole browser. */
  function sourceList(): SourceInfo[] {
    const hint =
      opts.browserTabs === "ok"
        ? HINT_TABS_OK
        : opts.browserTabs === "failed"
          ? HINT_TAB_FAILED
          : HINT_NOT_CONNECTED;
    return [
      ...demoSources.map((s) => (s.kind === "browser" ? { ...s, hint } : { ...s })),
      ...(opts.browserTabs ? demoTabs.map((s) => ({ ...s })) : []),
    ];
  }

  function active(id: string, offsetMs: number): ActiveSource {
    const src = sourceList().find((s) => s.id === id);
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
    if (rate > 0) {
      let frame = 0;
      levelsTimer = setInterval(() => {
        frame += 1;
        const levels = computeMockLevels(state.elapsedMs + frame * 100, state.sources);
        levelsListeners.forEach((l) => l.cb(levels));
      }, 100);
    }
    emit();
  }

  if (opts.recording) begin(["mic:default", "app:discord"], 12 * MIN + 41_000);

  // ----- meeting detection -----
  const detection = createMockDetection({
    ...(opts.detectMs === undefined ? {} : { promptAfterMs: opts.detectMs }),
    recording: () => state.status === "recording",
    // The same gate as `startRecording` with the last report; a recording that runs already
    // gets the sources added.
    record: async (sourceIds) => {
      const report = cycles.resolveChoice(undefined, true);
      if (state.status === "recording") {
        sourceIds
          .filter((id) => !state.sources.some((s) => s.id === id))
          .forEach(
            (id) =>
              (state = { ...state, sources: [...state.sources, active(id, state.elapsedMs)] }),
          );
        emit();
        return;
      }
      begin(sourceIds);
      if (state.meetingId !== null) reports.set(state.meetingId, report);
    },
  });

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

  // ----- creating a GitHub repository from the project wizard -----
  const createdRepos = new Set<string>();
  const githubStatus = async (): Promise<GithubRepoStatus> => {
    const secrets = await publishing.methods.publishingSecrets();
    const host = "api.github.com";
    return { tokenSet: secrets.github.some((h) => h.host === host && h.tokenSet), host };
  };

  const backend: Backend = {
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
    listSources: async () => sourceList(),
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
      if (levelsTimer) clearInterval(levelsTimer);
      levelsTimer = null;
      const stoppedId = state.meetingId ?? `m${meetings.length + 1}`;
      meetings = [
        meeting(
          stoppedId,
          "Untitled meeting",
          DEMO_NOW,
          state.elapsedMs,
          "stopped",
          state.sources.length,
          state.screenshotCount,
          null,
          null,
          cycleOf(stoppedId),
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
      if (opts.overlayFault === "blank") {
        fail("blank_capture", "The screen capture came back empty (all black).");
      }
      if (opts.overlayFault === "black") {
        const black =
          '<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="900"><rect width="1600" height="900" fill="#000"/></svg>';
        return `data:image/svg+xml;utf8,${encodeURIComponent(black)}`;
      }
      if (opts.overlayFault === "broken") return "data:image/png;base64,MTIzLDgwLDc4";
      const svg =
        '<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="900"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#101a33"/><stop offset="1" stop-color="#3a1030"/></linearGradient></defs><rect width="1600" height="900" fill="url(#g)"/><g fill="#00f0ff" opacity=".5"><rect x="120" y="120" width="560" height="320"/><rect x="760" y="200" width="680" height="480"/></g></svg>';
      return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
    },
    overlayReady: async () => {},
    overlayFinish: async (region) => {
      if (region && state.status === "recording") {
        state = { ...state, screenshotCount: state.screenshotCount + 1 };
        emit();
      }
    },
    processMeeting: async (id, options?: ProcessOptions) => {
      const m = meetings.find((x) => x.id === id);
      if (!m) fail("internal", "Meeting not found");
      if (world.active) {
        // The same refusal as the real application: the item's problem and its fix.
        const blocker = readiness().items.find(
          (i) => i.code !== undefined && i.state === "missing",
        );
        if (blocker?.code)
          fail(blocker.code, `${blocker.message}${blocker.fixHint ? ` ${blocker.fixHint}` : ""}`);
      }
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
        scheduledUnixMs: null,
        etaSeconds: 45,
      };
      jobs = existing ? jobs.map((j) => (j.meetingId === id ? fresh : j)) : [...jobs, fresh];
      transcripts.delete(id);
      voices.delete(id);
      enqueue(id);
      emitQueue();
      emitMeetings();
    },
    queueState: async () => queueSnapshot(),
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
      preSchedule.delete(id);
      jobs = jobs.filter((x) => x.meetingId !== id);
      const at = driven.indexOf(id);
      if (at >= 0) driven.splice(at, 1);
      if (!jobs.some((x) => x.state === "running" || x.state === "queued" || x.state === "waiting"))
        gpuOverride = null;
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
      const cid = cycleOf(id);
      const inCycle = participantsFor(undefined, id);
      for (const v of voicesOf(id)) {
        if (v.decision === "auto" && !v.confirmed && v.assignedParticipantId) {
          if (cid && !inCycle.some((p) => p.id === v.assignedParticipantId)) {
            const extra = extraProjects.get(v.assignedParticipantId) ?? new Set<string>();
            extra.add(cid);
            extraProjects.set(v.assignedParticipantId, extra);
          }
          v.confirmed = true;
        }
      }
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
        const legacy = participants.find((x) => x.id === `p-${inCycle.id}`);
        if (legacy) {
          legacy.name = n;
          participants.sort((a, b) => a.name.localeCompare(b.name));
        }
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
        participants = participants.filter((p) => p.id !== `p-${inCycle.id}`);
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
      const inCycle = splitKey(participantId);
      const legacyId = inCycle ? `p-${inCycle.id}` : participantId;
      const p = participants.find((x) => x.id === legacyId);
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
      // The token flag and the first-run flag are owned by their own commands.
      Object.assign(processing, s, {
        hfTokenSet: processing.hfTokenSet,
        firstRunDone: processing.firstRunDone,
      });
      processing.llm = { ...(s.llm ?? llm()), ...flags };
    },
    setHfToken: async (token) => {
      if (token.trim() === "") fail("internal", "The token is empty");
      processing.hfTokenSet = true;
    },
    clearHfToken: async () => void (processing.hfTokenSet = false),
    modelStatus: async () =>
      structuredClone(models).map((m) => ({
        ...m,
        status: m.status ?? (m.cached ? "ready" : "missing"),
      })),
    setupStatus: async () => readiness(),
    detectLlamaServer: async () => [...world.llamaCandidates],
    findGgufModels: async (dir) => mockGgufModels(world.ggufs, dir),
    checkSetupPath: async (kind, path) =>
      mockCheckSetupPath(kind, path, world.active ? world.knownFiles : undefined),
    prepareWorkerEnv: async () => {
      if (preparing) fail("busy", "The environment is already being prepared.");
      preparing = true;
      const lines: Array<[string, number | null]> = [
        ["Resolving packages", 0.1],
        ["Downloading packages", 0.45],
        ["Installing packages", 0.8],
      ];
      const send = (line: string, fraction: number | null, end?: string | null) =>
        setupListeners.forEach((l) =>
          l.cb({
            key: "workerEnv",
            line,
            fraction,
            done: end !== undefined,
            ok: end === undefined ? null : end === null,
            error: end ?? null,
          }),
        );
      lines.forEach(([line, fraction], n) => setTimeout(() => send(line, fraction), n * setupMs));
      setTimeout(() => {
        preparing = false;
        if (world.envFailures > 0) {
          world.envFailures -= 1;
          send("uv sync failed", null, "uv sync failed: no network connection.");
          return;
        }
        world.workerEnv = true;
        send("The processing environment is ready", 1, null);
      }, lines.length * setupMs);
    },
    setFirstRunDone: async (done = true) => void (processing.firstRunDone = done),
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
        if ((world.downloadFailures[key] ?? 0) > 0) {
          // Like the real application: the failure arrives as an event, not as a rejection.
          world.downloadFailures[key] -= 1;
          setTimeout(() => {
            progressListeners.forEach((l) =>
              l.cb({
                key,
                fraction: 0,
                message: "Failed: the download was interrupted (network error).",
              }),
            );
            modelsListeners.forEach((l) => l.cb());
          }, setupMs);
          continue;
        }
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
            if (model) {
              model.cached = true;
              model.status = "ready";
            }
            modelsListeners.forEach((l) => l.cb());
          }
        }, setupMs);
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
      bridge: bridge.settings(),
      detection: detection.settings(),
    }),
    // The repository list, the last report and the paired extensions are not saved from the
    // settings screen.
    suspendHotkeys: async () => {
      setSuspended(true);
    },
    resumeHotkeys: async () => {
      setSuspended(false);
    },
    hotkeyStatus: async (): Promise<HotkeyStatus> => {
      const taken = new Set((opts.takenHotkeys ?? []).map(normalizeHotkey));
      const failure = (hotkey: string) =>
        taken.has(normalizeHotkey(hotkey)) || validateHotkey(hotkey) !== null
          ? `\`${hotkey}\` cannot be used as a hotkey (already taken or not a valid shortcut)`
          : null;
      return {
        recordError: failure(settings.recordHotkey),
        screenshotError: failure(settings.screenshotHotkey),
      };
    },
    saveSettings: async (s) => {
      setSuspended(false);
      for (const hk of [s.recordHotkey, s.screenshotHotkey]) {
        const problem = validateHotkey(hk);
        if (problem) fail("invalid", `\`${hk}\` ${problem}`);
      }
      const recNorm = normalizeHotkey(s.recordHotkey);
      const scrNorm = normalizeHotkey(s.screenshotHotkey);
      if (recNorm !== "" && scrNorm !== "" && recNorm === scrNorm) {
        fail("invalid", "Record and region screenshot hotkeys must be different");
      }
      const taken = new Set((opts.takenHotkeys ?? []).map(normalizeHotkey));
      if (recNorm !== "" && taken.has(recNorm)) {
        fail(
          "hotkey_in_use",
          `\`${s.recordHotkey}\` cannot be used as a hotkey (already taken or not a valid shortcut)`,
        );
      }
      if (scrNorm !== "" && taken.has(scrNorm)) {
        fail(
          "hotkey_in_use",
          `\`${s.screenshotHotkey}\` cannot be used as a hotkey (already taken or not a valid shortcut)`,
        );
      }
      bridge.save(s.bridge);
      Object.assign(settings, {
        recordingsDir: s.recordingsDir,
        recordHotkey: s.recordHotkey,
        screenshotHotkey: s.screenshotHotkey,
        ...(s.mouseButtons === undefined ? {} : { mouseButtons: s.mouseButtons }),
        ...(s.localSaveEnabled === undefined ? {} : { localSaveEnabled: s.localSaveEnabled }),
        ...(s.localSaveDir === undefined ? {} : { localSaveDir: s.localSaveDir }),
        ...(s.lastSourceKeys === undefined ? {} : { lastSourceKeys: [...s.lastSourceKeys] }),
      });
    },
    saveLastSources: async (keys: string[]) => {
      settings.lastSourceKeys = [...keys];
      return [...settings.lastSourceKeys];
    },
    setQueueGpuOverride: async (overrideCfg: QueueGpuOverride | null) => {
      gpuOverride = overrideCfg ? structuredClone(overrideCfg) : null;
      if (
        !jobs.some((x) => x.state === "running" || x.state === "queued" || x.state === "waiting")
      ) {
        gpuOverride = null;
      }
      emitQueue();
      return queueSnapshot();
    },
    forceProcess: async (meetingId: string) => {
      const m = meetings.find((x) => x.id === meetingId);
      if (!m) fail("not_found", "Meeting not found");
      preSchedule.delete(meetingId);
      if (!plans.has(meetingId)) {
        plans.set(meetingId, planLlm(meetingId, undefined));
      }
      const existing = jobs.find((j) => j.meetingId === meetingId);
      if (existing) {
        patchJob(meetingId, (j) => ({
          ...j,
          state: "running",
          waitingReason: null,
          scheduledUnixMs: null,
          etaSeconds: j.etaSeconds ?? 30,
          stages: j.stages.map((s) =>
            s.status === "failed" ? { ...s, status: "pending", error: null } : s,
          ),
        }));
      } else {
        const fresh: JobView = {
          meetingId,
          title: m.title,
          state: "running",
          progress: 0,
          waitingReason: null,
          stages: stageViews(),
          scheduledUnixMs: null,
          etaSeconds: 45,
        };
        jobs = [...jobs, fresh];
      }
      enqueue(meetingId);
      emitQueue();
      emitMeetings();
      return structuredClone(jobs.find((j) => j.meetingId === meetingId)!);
    },
    scheduleJob: async (meetingId: string, scheduledUnixMs: number | null) => {
      const m = meetings.find((x) => x.id === meetingId);
      if (!m) fail("not_found", "Meeting not found");
      const existing = jobs.find((j) => j.meetingId === meetingId);
      if (!existing) {
        plans.set(meetingId, planLlm(meetingId, undefined));
        jobs = [
          ...jobs,
          {
            meetingId,
            title: m.title,
            state: scheduledUnixMs !== null ? "waiting" : "queued",
            progress: 0,
            waitingReason:
              scheduledUnixMs !== null
                ? `Scheduled for ${new Date(scheduledUnixMs).toISOString()}`
                : null,
            stages: stageViews(),
            scheduledUnixMs,
            etaSeconds: null,
          },
        ];
      } else if (scheduledUnixMs !== null) {
        if (!preSchedule.has(meetingId)) {
          preSchedule.set(meetingId, {
            state: existing.state,
            waitingReason: existing.waitingReason,
          });
        }
        patchJob(meetingId, (j) => ({
          ...j,
          scheduledUnixMs,
          state: "waiting",
          waitingReason: `Scheduled for ${new Date(scheduledUnixMs).toISOString()}`,
        }));
      } else {
        const prev = preSchedule.get(meetingId);
        preSchedule.delete(meetingId);
        patchJob(meetingId, (j) => ({
          ...j,
          scheduledUnixMs: null,
          state: prev
            ? prev.state
            : j.state === "waiting" && j.waitingReason?.startsWith("Scheduled")
              ? "queued"
              : j.state,
          waitingReason: prev
            ? prev.waitingReason
            : j.waitingReason?.startsWith("Scheduled")
              ? null
              : j.waitingReason,
        }));
      }
      if (scheduledUnixMs !== null) {
        const at = driven.indexOf(meetingId);
        if (at >= 0) driven.splice(at, 1);
      } else {
        const target = jobs.find((j) => j.meetingId === meetingId);
        if (target?.state === "queued") enqueue(meetingId);
      }
      emitQueue();
      emitMeetings();
      return structuredClone(jobs.find((j) => j.meetingId === meetingId)!);
    },
    getProjectPrefs: async (cycleId: string) => {
      const prefs = projectPrefs.get(cycleId) ?? {
        cycleId,
        localSaveEnabled: false,
        localSaveDir: "",
        llmKindOverride: null,
        llmModelOverride: null,
        audioRetention: null,
      };
      return structuredClone(prefs);
    },
    saveProjectPrefs: async (prefs: ProjectPrefs) => {
      const saved: ProjectPrefs = {
        cycleId: prefs.cycleId,
        localSaveEnabled: Boolean(prefs.localSaveEnabled),
        localSaveDir: prefs.localSaveDir ?? "",
        llmKindOverride: prefs.llmKindOverride ?? null,
        llmModelOverride: prefs.llmModelOverride ?? null,
        audioRetention: prefs.audioRetention ?? null,
      };
      projectPrefs.set(saved.cycleId, structuredClone(saved));
      return structuredClone(saved);
    },
    wizardCreateProject: async (req: WizardCreateProjectInput): Promise<CycleDto> => {
      const id = req.id.trim();
      const name = req.name.trim();
      let repoId: string | undefined;
      if (req.mode === "existing") {
        const remoteUrl = (req.remoteUrl ?? "").trim();
        const repoPath = (req.repoPath ?? "").trim();
        if (remoteUrl !== "") {
          const existingRepo = cycles
            .listRepos()
            .find((r) => r.remote?.toLowerCase() === remoteUrl.toLowerCase());
          const repo = existingRepo ?? cycles.addRepo({ kind: "clone", url: remoteUrl });
          repoId = repo.id;
        } else if (repoPath !== "") {
          const existingRepo = cycles
            .listRepos()
            .find((r) => r.path.toLowerCase() === repoPath.toLowerCase());
          const repo = existingRepo ?? cycles.addRepo({ kind: "local", path: repoPath });
          repoId = repo.id;
        } else {
          fail("invalid", "choose an existing folder or enter a remote repository address");
        }
      } else {
        const repoPath = (req.repoPath ?? "").trim();
        if (repoPath !== "") {
          const existingRepo = cycles
            .listRepos()
            .find((r) => r.path.toLowerCase() === repoPath.toLowerCase());
          const repo = existingRepo ?? cycles.addRepo({ kind: "local", path: repoPath });
          repoId = repo.id;
        }
      }
      const existingCycle = cycles.cycle(id);
      const details = existingCycle ? cycles.getCycle(id) : cycles.createCycle(id, name, repoId);
      const cycle = structuredClone(details.cycle);
      if (name !== "") cycle.name = name;
      if (
        req.outputLanguage === "auto" ||
        req.outputLanguage === "pl" ||
        req.outputLanguage === "en"
      ) {
        cycle.languages.output = req.outputLanguage as LanguageSetting;
      }
      if (
        req.defaultReportMode === "fixed" ||
        req.defaultReportMode === "template" ||
        req.defaultReportMode === "auto" ||
        req.defaultReportMode === "by-type"
      ) {
        cycle.report.defaultMode = req.defaultReportMode as ReportMode;
      }
      if (req.githubRepo && req.githubRepo.trim() !== "") {
        cycles.setGithubRepo(details.repoId, req.githubRepo.trim());
        cycle.targets.github = { path: "meetings", publish: "on-approval", issues: true };
      }
      if (req.discordWebhookRef && req.discordWebhookRef.trim() !== "") {
        cycle.targets.discord = {
          webhook: req.discordWebhookRef.trim(),
          publish: "on-approval",
        };
      }
      const saved = cycles.saveCycle(cycle);
      if (!saved.saved) {
        const first = saved.issues[0];
        fail(
          "invalid",
          first ? `${first.path}: ${first.message}` : "invalid project configuration",
        );
      }
      projectPrefs.set(id, {
        cycleId: id,
        localSaveEnabled: Boolean(req.localSaveEnabled),
        localSaveDir: req.localSaveDir ?? "",
        llmKindOverride: null,
        llmModelOverride: null,
        audioRetention: null,
      });
      emitCycles();
      return structuredClone(saved.details!.cycle);
    },
    githubRepoStatus: async (): Promise<GithubRepoStatus> => githubStatus(),
    wizardCreateProjectWithGithub: async (
      req: WizardCreateProjectInput,
      github: GithubRepoRequest,
    ): Promise<WizardCreateResult> => {
      if (req.mode !== "auto")
        fail(
          "invalid",
          "a GitHub repository can only be created together with a new project folder",
        );
      const name = github.name.trim();
      const owner = (github.owner ?? "").trim();
      const localOnly = async (
        reason: GithubFallbackReason,
        message: string,
      ): Promise<WizardCreateResult> => ({
        cycle: await backend.wizardCreateProject({ ...req, githubRepo: null }),
        github: { status: "local_only", reason, message, repo: null, url: null },
      });
      if (!/^[A-Za-z0-9_.-]+$/.test(name) || name.endsWith(".git") || /^\.+$/.test(name))
        return localOnly(
          "invalid_name",
          "The GitHub repository name may contain only letters, digits, - _ and ., so only a local project folder was created.",
        );
      if (!github.confirmed)
        return localOnly(
          "not_confirmed",
          "The GitHub repository was not confirmed, so only a local project folder was created.",
        );
      if (!(await githubStatus()).tokenSet)
        return localOnly(
          "no_token",
          "No GitHub token is saved, so only a local project folder was created. Save a token in Settings > Connectors to create the repository next time.",
        );
      const full = `${owner || MOCK_GITHUB_LOGIN}/${name}`;
      if (opts.githubCreateFault === "offline")
        return localOnly(
          "failed",
          "GitHub could not be reached (cannot reach api.github.com). Only a local project folder was created.",
        );
      if (opts.githubCreateFault === "name-taken" || createdRepos.has(full.toLowerCase()))
        return localOnly(
          "failed",
          `${owner ? `the organization ${owner}` : "your GitHub account"} already has a repository named ${name}, or GitHub does not accept that name. Only a local project folder was created.`,
        );
      createdRepos.add(full.toLowerCase());
      return {
        cycle: await backend.wizardCreateProject({ ...req, githubRepo: full }),
        github: {
          status: "created",
          reason: null,
          message: `Created the private GitHub repository ${full} and linked it to the project.`,
          repo: full,
          url: `https://github.com/${full}`,
        },
      };
    },
    importMeeting: async (req: ImportMeetingInput): Promise<MeetingDetails> => {
      const filePath = req.filePath.trim();
      if (filePath === "") fail("invalid", "choose an audio or video file to import");
      const cycleId = req.cycleId.trim();
      if (cycleId === "") fail("report_mode_required", "choose a project before importing");
      cycles.getCycle(cycleId);
      const stem =
        filePath
          .split(/[/\\]/)
          .pop()
          ?.replace(/\.[^.]+$/, "") || "Imported recording";
      const id = `m${meetings.length + 1}`;
      const screenshotCount = req.mediaMode === "audio_and_slides" ? 4 : 0;
      const summary = meeting(
        id,
        stem,
        DEMO_NOW,
        24 * MIN,
        "stopped",
        1,
        screenshotCount,
        `${stem} (${req.mediaMode === "audio_and_slides" ? "slides extracted" : "audio import"})`,
        null,
        cycleId,
      );
      meetings = [summary, ...meetings];
      const mode: ReportMode =
        req.reportMode === "fixed" ||
        req.reportMode === "template" ||
        req.reportMode === "auto" ||
        req.reportMode === "by-type"
          ? req.reportMode
          : "auto";
      reports.set(id, {
        cycleId,
        mode,
        template: null,
        meetingType: req.meetingType ?? null,
        outputLanguage: req.outputLanguage ?? null,
      });
      plans.set(id, planLlm(id, undefined));
      const newJob: JobView = {
        meetingId: id,
        title: stem,
        state: req.processNow ? "queued" : "waiting",
        progress: 0,
        waitingReason: req.processNow ? null : "Added to processing queue",
        stages: stageViews(),
        scheduledUnixMs: null,
        etaSeconds: req.processNow ? 45 : null,
      };
      jobs = [newJob, ...jobs];
      if (req.processNow) enqueue(id);
      emitMeetings();
      emitQueue();
      return {
        summary: withJob(summary),
        tracks: [
          {
            name: "imported",
            source: filePath.split(/[/\\]/).pop() ?? filePath,
            offsetMs: 0,
            ended: true,
          },
        ],
        screenshots: Array.from({ length: screenshotCount }, (_, i) => ({
          file: `img/slide-${String(i + 1).padStart(4, "0")}-${(i + 1) * 30_000}.png`,
          offsetMs: (i + 1) * 30_000,
        })),
      };
    },
    ...createMockPicker(opts.pick),
    exportMeetingMarkdown: async (meetingId: string, destPath?: string | null) => {
      const found = meetings.find((x) => x.id === meetingId);
      if (!found) fail("not_found", "Meeting not found");
      if (destPath && destPath.trim() !== "") return destPath.trim();
      const cycleId = cycleOf(meetingId) ?? "NOMAD";
      const dir =
        projectPrefs.get(cycleId)?.localSaveDir ||
        settings.localSaveDir ||
        "Documents\\BlahBlahBot\\notes";
      return `${dir}\\${cycleId}\\2026-11-13-${meetingId}\\report.md`;
    },
    autoDetectModels: async (): Promise<DetectedModels> => ({
      llamaServerPath: processing.llamaServerPath || "C:\\Tools\\llama\\llama-server.exe",
      llmModels: [
        "D:\\Models\\gemma-4-12b-q4_k_m.gguf",
        "D:\\Models\\Qwen2.5-14B-Instruct-Q4_K_M.gguf",
      ],
      visionModels: ["D:\\Models\\Qwen3-VL-4B-Instruct-Q4_K_M.gguf"],
      visionPairs: [
        {
          modelPath: "D:\\Models\\Qwen3-VL-4B-Instruct-Q4_K_M.gguf",
          mmprojPath: "D:\\Models\\mmproj-Qwen3-VL-4B-Instruct-F16.gguf",
        },
      ],
      defaultLocalSaveDir: "Documents\\BlahBlahBot\\notes",
    }),
    listAllVoiceProfiles: async (): Promise<GlobalVoiceProfile[]> => {
      const allCycles = cycles.listCycles();
      const out: GlobalVoiceProfile[] = [];
      const seenIds = new Set<string>();
      for (const c of allCycles.filter((x) => x.valid)) {
        for (const p of cycles.participants(c.id)) {
          const fullId = keyOf(c.id, p.id);
          if (mergedDeleted.has(fullId)) continue;
          seenIds.add(fullId);
          const st = stats.get(fullId);
          const sampleCount = st?.sampleCount ?? 0;
          const projSet = new Set<string>();
          if (!removedProjects.get(fullId)?.has(c.id)) {
            projSet.add(c.id);
          }
          for (const extra of extraProjects.get(fullId) ?? []) {
            projSet.add(extra);
          }
          const projects = [...projSet].sort();
          out.push({
            id: fullId,
            name: p.name,
            cycleId: projects[0] ?? null,
            projects,
            sampleCount,
            updatedUnixMs: DEMO_NOW - 3600_000,
            hasSampleClip: sampleCount > 0,
          });
        }
      }
      const legacyShadowIds = new Set(["p-alex", "p-jan", "p-kasia"]);
      for (const p of participants) {
        if (mergedDeleted.has(p.id) || seenIds.has(p.id)) continue;
        if (!legacyShadowIds.has(p.id)) {
          const projects = [...(extraProjects.get(p.id) ?? [])]
            .filter((cid) => !removedProjects.get(p.id)?.has(cid))
            .sort();
          out.push({
            id: p.id,
            name: p.name,
            cycleId: projects[0] ?? null,
            projects,
            sampleCount: p.sampleCount,
            updatedUnixMs: DEMO_NOW - 7200_000,
            hasSampleClip: p.sampleCount > 0,
          });
        }
      }
      return out.sort((a, b) => a.name.localeCompare(b.name));
    },
    voiceProfileSampleClip: async (participantId: string): Promise<number[]> => {
      const inCycle = splitKey(participantId);
      if (inCycle) {
        const exists = cycles.participants(inCycle.cycleId).some((p) => p.id === inCycle.id);
        if (!exists) fail("not_found", "participant not found");
        if ((stats.get(participantId)?.sampleCount ?? 0) === 0) {
          fail("not_found", "no sample clip available for this participant");
        }
      } else {
        const found = participants.find((p) => p.id === participantId);
        if (!found) fail("not_found", "participant not found");
        if (found.sampleCount === 0) {
          fail("not_found", "no sample clip available for this participant");
        }
      }
      return wavBytes(260, 0.5, 8000);
    },
    enrollVoiceProfile: async (
      name: string,
      cycleId: string | null,
      audioPath: string,
    ): Promise<GlobalVoiceProfile> => {
      const n = cleanName(name);
      if (audioPath.trim() === "") fail("invalid", "choose an audio file for voice enrollment");
      const allCycles = cycles.listCycles();
      const projectsForName = (personName: string): string[] =>
        allCycles
          .filter((c) => cycles.participants(c.id).some((p) => sameName(p.name, personName)))
          .map((c) => c.id)
          .sort();
      if (cycleId && cycleId.trim() !== "") {
        const cid = cycleId.trim();
        const pid = cycles.addParticipant(cid, n);
        const fullId = keyOf(cid, pid);
        const prev = stats.get(fullId) ?? { sampleCount: 0, speechMs: 0 };
        const sampleCount = prev.sampleCount + 1;
        stats.set(fullId, { sampleCount, speechMs: prev.speechMs + 20_000 });
        emitCycles();
        return {
          id: fullId,
          name: n,
          cycleId: cid,
          projects: projectsForName(n),
          sampleCount,
          updatedUnixMs: DEMO_NOW,
          hasSampleClip: true,
        };
      }
      let p = participants.find((x) => sameName(x.name, n));
      if (!p) {
        p = { id: `p-new-${nextParticipant++}`, name: n, sampleCount: 1, speechMs: 20_000 };
        participants = [...participants, p].sort((a, b) => a.name.localeCompare(b.name));
      } else {
        p.sampleCount += 1;
        p.speechMs += 20_000;
      }
      return {
        id: p.id,
        name: p.name,
        cycleId: null,
        projects: [...(extraProjects.get(p.id) ?? [])].sort(),
        sampleCount: p.sampleCount,
        updatedUnixMs: DEMO_NOW,
        hasSampleClip: true,
      };
    },
    addProfileToProject: async (participantId, cycleId) => {
      const removed = removedProjects.get(participantId);
      removed?.delete(cycleId);
      const extra = extraProjects.get(participantId) ?? new Set<string>();
      extra.add(cycleId);
      extraProjects.set(participantId, extra);
      emitCycles();
      const all = cycles.listCycles().flatMap((c) =>
        cycles.participants(c.id).map((p) => ({
          id: keyOf(c.id, p.id),
          name: p.name,
          cycleId: c.id,
        })),
      );
      const foundCycle = all.find((x) => x.id === participantId);
      const foundLocal = participants.find((x) => x.id === participantId);
      if (!foundCycle && !foundLocal) fail("not_found", "participant not found");
      const name = foundCycle?.name ?? foundLocal!.name;
      const projSet = new Set<string>(extra);
      if (foundCycle && !removed?.has(foundCycle.cycleId)) projSet.add(foundCycle.cycleId);
      const sampleCount = foundCycle
        ? (stats.get(participantId)?.sampleCount ?? 0)
        : foundLocal!.sampleCount;
      return {
        id: participantId,
        name,
        cycleId: [...projSet].sort()[0] ?? null,
        projects: [...projSet].sort(),
        sampleCount,
        updatedUnixMs: DEMO_NOW,
        hasSampleClip: sampleCount > 0,
      };
    },
    removeProfileFromProject: async (participantId, cycleId) => {
      extraProjects.get(participantId)?.delete(cycleId);
      const removed = removedProjects.get(participantId) ?? new Set<string>();
      removed.add(cycleId);
      removedProjects.set(participantId, removed);
      emitCycles();
      const inCycle = splitKey(participantId);
      const foundLocal = participants.find((x) => x.id === participantId);
      const cycleName = inCycle
        ? cycles.participants(inCycle.cycleId).find((p) => p.id === inCycle.id)?.name
        : undefined;
      if (!cycleName && !foundLocal) fail("not_found", "participant not found");
      const projSet = new Set<string>(extraProjects.get(participantId) ?? []);
      if (inCycle && !removed.has(inCycle.cycleId)) projSet.add(inCycle.cycleId);
      const sampleCount = inCycle
        ? (stats.get(participantId)?.sampleCount ?? 0)
        : foundLocal!.sampleCount;
      return {
        id: participantId,
        name: cycleName ?? foundLocal!.name,
        cycleId: [...projSet].sort()[0] ?? null,
        projects: [...projSet].sort(),
        sampleCount,
        updatedUnixMs: DEMO_NOW,
        hasSampleClip: sampleCount > 0,
      };
    },
    mergeParticipants: async (keepId, dropId) => {
      if (keepId === dropId) fail("invalid", "cannot merge a participant with itself");
      if (mergedDeleted.has(keepId) || mergedDeleted.has(dropId)) {
        fail("not_found", "participant not found");
      }
      const keepInCycle = splitKey(keepId);
      const dropInCycle = splitKey(dropId);
      const keepLocal = participants.find((x) => x.id === keepId);
      const dropLocal = participants.find((x) => x.id === dropId);
      const keepCycleEntry = keepInCycle
        ? cycles.participants(keepInCycle.cycleId).find((p) => p.id === keepInCycle.id)
        : undefined;
      const dropCycleEntry = dropInCycle
        ? cycles.participants(dropInCycle.cycleId).find((p) => p.id === dropInCycle.id)
        : undefined;
      if (!keepLocal && !keepCycleEntry) fail("not_found", "participant not found");
      if (!dropLocal && !dropCycleEntry) fail("not_found", "participant not found");
      const keepProjs = new Set<string>(extraProjects.get(keepId) ?? []);
      if (keepInCycle && !removedProjects.get(keepId)?.has(keepInCycle.cycleId)) {
        keepProjs.add(keepInCycle.cycleId);
      }
      const dropProjs = new Set<string>(extraProjects.get(dropId) ?? []);
      if (dropInCycle && !removedProjects.get(dropId)?.has(dropInCycle.cycleId)) {
        dropProjs.add(dropInCycle.cycleId);
      }
      for (const proj of dropProjs) {
        if (keepProjs.has(proj)) {
          fail("invalid", `cannot merge participants that both belong to project ${proj}`);
        }
      }
      const keepSt = stats.get(keepId) ?? {
        sampleCount: keepLocal?.sampleCount ?? 0,
        speechMs: keepLocal?.speechMs ?? 0,
      };
      const dropSt = stats.get(dropId) ?? {
        sampleCount: dropLocal?.sampleCount ?? 0,
        speechMs: dropLocal?.speechMs ?? 0,
      };
      const totalSamples = Math.min(20, keepSt.sampleCount + dropSt.sampleCount);
      const totalSpeech = keepSt.speechMs + dropSt.speechMs;
      stats.set(keepId, { sampleCount: totalSamples, speechMs: totalSpeech });
      stats.delete(dropId);
      if (keepLocal) {
        keepLocal.sampleCount = totalSamples;
        keepLocal.speechMs = totalSpeech;
      }
      const mergedProjs = new Set([...keepProjs, ...dropProjs]);
      const keepRemoved = removedProjects.get(keepId);
      if (keepRemoved) {
        for (const proj of dropProjs) keepRemoved.delete(proj);
      }
      extraProjects.set(keepId, mergedProjs);
      extraProjects.delete(dropId);
      removedProjects.delete(dropId);
      mergedDeleted.add(dropId);
      participants = participants.filter((p) => p.id !== dropId);
      for (const list of voices.values()) {
        for (const v of list) {
          if (v.assignedParticipantId === dropId) v.assignedParticipantId = keepId;
          if (v.suggestedParticipantId === dropId) v.suggestedParticipantId = keepId;
        }
      }
      emitCycles();
      const keepName = keepCycleEntry?.name ?? keepLocal!.name;
      return {
        id: keepId,
        name: keepName,
        cycleId: [...mergedProjs].sort()[0] ?? null,
        projects: [...mergedProjs].sort(),
        sampleCount: totalSamples,
        updatedUnixMs: DEMO_NOW,
        hasSampleClip: totalSamples > 0,
      };
    },
    undoVoiceDecision: async (id, voiceId, unlinkCycleId = null) => {
      const v = findVoice(id, voiceId);
      const prev = v.assignedParticipantId;
      v.assignedParticipantId = undefined;
      v.confirmed = false;
      if (unlinkCycleId && prev) {
        extraProjects.get(prev)?.delete(unlinkCycleId);
        const removed = removedProjects.get(prev) ?? new Set<string>();
        removed.add(unlinkCycleId);
        removedProjects.set(prev, removed);
      }
      return voicesData(id);
    },
    ...bridge.methods,
    ...detection.methods,
    onRecordingState: (cb) => subscribe(stateListeners, cb),
    onRecordingLevels: (cb) => {
      const sub = subscribe(levelsListeners, cb);
      if (state.status === "recording") {
        queueMicrotask(() => cb(computeMockLevels(state.elapsedMs, state.sources)));
      }
      return sub;
    },
    onMeetingsChanged: (cb) => subscribe(meetingListeners, cb),
    onQueueChanged: (cb) => subscribe(queueListeners, cb),
    onModelsProgress: (cb) => subscribe(progressListeners, cb),
    onModelsChanged: (cb) => subscribe(modelsListeners, cb),
    onSetupProgress: (cb) => subscribe(setupListeners, cb),

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
    setTargetRule: async (cycleId, target, mode) => {
      const targets = cycles.setTargetRule(cycleId, target, mode);
      emitCycles();
      return targets;
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
    ...briefing,

    // ----- drafts -----
    ...drafts.methods,

    // ----- publishing -----
    ...publishing.methods,
  };
  return backend;
}
