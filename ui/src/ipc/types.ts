// Types shared with the Rust side. Keep in sync with crates/app/src/ipc.rs.

export type MeetingState = "recording" | "stopped" | "recoverable" | "recovered";

export interface MeetingSummary {
  id: string;
  title: string;
  startedUnixMs: number;
  durationMs: number | null;
  state: MeetingState;
  trackCount: number;
  screenshotCount: number;
  /** State of the processing job of this meeting, null when it was never queued. */
  job?: JobState | null;
}

export interface TrackInfo {
  name: string;
  source: string;
  offsetMs: number;
  ended: boolean;
}

export interface ScreenshotInfo {
  file: string;
  offsetMs: number;
}

export interface MeetingDetails {
  summary: MeetingSummary;
  tracks: TrackInfo[];
  screenshots: ScreenshotInfo[];
}

export type SourceKind = "microphone" | "application";

/** A source that can be recorded: a microphone or an application with an audio session. */
export interface SourceInfo {
  id: string;
  kind: SourceKind;
  label: string;
  /** True while the application is producing sound. */
  active: boolean;
}

export type SourceStatus = "starting" | "recording" | "ended" | "error";

export interface ActiveSource {
  id: string;
  label: string;
  kind: SourceKind;
  status: SourceStatus;
  /** Start offset on the session clock. */
  offsetMs: number;
  /** Peak level of the last interval, 0 to 1. */
  level: number;
  /** Human-readable reason when the status is `ended` or `error`. */
  detail?: string;
}

export type RecordingStatus = "idle" | "recording";

export interface RecordingState {
  status: RecordingStatus;
  meetingId: string | null;
  elapsedMs: number;
  sources: ActiveSource[];
  screenshotCount: number;
}

export interface Settings {
  recordingsDir: string;
  recordHotkey: string;
  screenshotHotkey: string;
  /**
   * Registered cycle repositories. Read-only for the settings screen: they change only through
   * `addCycleRepo` and the cycle commands, and a `saveSettings` keeps the stored list.
   */
  cycleRepos?: CycleRepoSetting[];
  /** The report choice of the last recording that started; filled by the application. */
  lastReport?: LastReport | null;
}

export type IpcErrorCode =
  | "already_recording"
  | "not_recording"
  | "source_not_found"
  | "source_already_added"
  | "hotkey_in_use"
  | "not_ready"
  | "already_queued"
  | "job_running"
  | "invalid"
  | "not_found"
  | "duplicate"
  | "io"
  | "internal"
  | "busy"
  | "invalid_state"
  | "secrets"
  // Stage 4: cycles, report modes, language-model backends and drafts.
  /** A recording needs a valid cycle and report mode (also for a bad template or type). */
  | "report_mode_required"
  | "cycle_invalid"
  | "git_unavailable"
  | "git_failed"
  /** Cloud processing without the user's confirmation for this meeting and endpoint. */
  | "cloud_not_confirmed"
  | "cloud_key_missing"
  | "llm_not_configured"
  /** The draft does not exist before the reduce stage has run. */
  | "no_draft"
  | "no_new_draft"
  | "draft_invalid";

export interface IpcError {
  code: IpcErrorCode;
  message: string;
}

export interface StartRecordingRequest {
  title?: string;
  sourceIds: string[];
  /**
   * The cycle and report mode. The backend refuses to start without it (`report_mode_required`)
   * unless `useLastReport` is set and a last report exists.
   */
  report?: ReportChoice;
  /** For the widget: when `report` is absent use the last one (still checked by the backend). */
  useLastReport?: boolean;
}

/** A selection as fractions (0 to 1) of the frozen screenshot shown in the overlay. */
export interface Region {
  x: number;
  y: number;
  w: number;
  h: number;
}

// ---------- Stage 2: processing pipeline ----------

export type StageName =
  | "asr"
  | "diarize"
  | "merge"
  | "speaker_id"
  | "clips"
  | "audio_cleanup"
  | "vision"
  | "extract"
  | "reduce";
export type StageStatus = "pending" | "running" | "done" | "failed";
export type JobState = "queued" | "waiting" | "running" | "failed" | "done";

export interface StageView {
  stage: StageName;
  status: StageStatus;
  attempts: number;
  error: string | null;
  /** Why a stage that is `done` did nothing, e.g. "the meeting has no cycle". */
  note?: string | null;
  /** Live progress of the running stage, 0 to 1. */
  fraction: number | null;
  message: string | null;
}

export interface JobView {
  meetingId: string;
  title: string;
  state: JobState;
  /** Overall progress, 0 to 1. */
  progress: number;
  /** Human-readable reason while the state is `waiting`. */
  waitingReason: string | null;
  /** In execution order. */
  stages: StageView[];
}

export interface QueueState {
  jobs: JobView[];
}

export type LanguageChoice = "auto" | "pl" | "en";

export interface ProcessOptions {
  language?: LanguageChoice;
  maxSpeakers?: number | null;
  /**
   * The language-model backend for this meeting; absent uses `ProcessingSettings.llm.defaultKind`.
   * `cloud` fails with `cloud_not_confirmed` until `confirmCloud` was called for the meeting.
   */
  llm?: { kind: LlmKind };
}

export type SpeakerKind = "user" | "voice" | "source" | "unknown";

export interface TranscriptSpeaker {
  id: string;
  label: string;
  kind: SpeakerKind;
  track?: string;
}

export interface TranscriptSegment {
  id: number;
  track: string;
  speaker: string;
  startMs: number;
  endMs: number;
  text: string;
  /** Set when the user corrected the text: the text as recognized. */
  originalText: string | null;
  excluded: boolean;
}

export interface ScreenshotDescription {
  file: string;
  offsetMs: number;
  description: string;
}

export interface TranscriptData {
  meetingId: string;
  language: string;
  speakers: TranscriptSpeaker[];
  segments: TranscriptSegment[];
  screenshots: ScreenshotDescription[];
}

/** `text: null` restores the recognized text. */
export interface SegmentEdit {
  text: string | null;
  excluded: boolean;
}

export type ProcessAfterRecording = "ask" | "now" | "later";
export type RetentionMode = "keep" | "delete" | "keepDays";

export interface ProcessingSettings {
  processAfterRecording: ProcessAfterRecording;
  audioRetention: { mode: RetentionMode; days: number };
  gpuIdleSeconds: number;
  gpuMaxUtilPercent: number;
  uvPath: string;
  workerDir: string;
  llamaServerPath: string;
  modelsDir: string;
  /** Whether a Hugging Face token is stored. The token itself is never returned. */
  hfTokenSet: boolean;
  /** Cosine score from which a voice is named automatically (0..1). */
  voiceAccept: number;
  /** Cosine score from which a name is only suggested (0..1, not above `voiceAccept`). */
  voiceSuggest: number;
  /** Minimum gap between the best and the second best match (0 or more). */
  voiceMargin: number;
  /** Days after which unconfirmed sample clips are deleted (1 or more). */
  clipMaxDays: number;
  /** Language-model backend settings (always present from the backend). */
  llm?: LlmSettings;
}

export type ModelKey = "whisper-turbo" | "whisper-large-v3" | "diarization" | "vision";

export interface ModelStatus {
  key: ModelKey;
  label: string;
  cached: boolean;
  gated: boolean;
  sizeMb: number | null;
}

export interface ModelsProgress {
  key: string;
  fraction: number;
  message: string;
}

// ---------- Stage 3: speakers and voice profiles ----------

/** How the matcher treated a voice: no match, named automatically, or only a suggestion. */
export type VoiceDecision = "unknown" | "auto" | "suggested";

export interface ClipView {
  idx: number;
  startMs: number;
  endMs: number;
  /** What was said in the clip, from the transcript. */
  text: string;
}

export interface VoiceView {
  voiceId: string;
  track: string;
  /** The label of the diarization, e.g. `SPEAKER_00`. */
  label: string;
  speechMs: number;
  decision: VoiceDecision;
  /** Cosine similarity of the match, 0 to 1. */
  score?: number | null;
  suggestedParticipantId?: string | null;
  assignedParticipantId?: string | null;
  /** True once the user decided (or accepted the automatic match). */
  confirmed: boolean;
  /** The voice this one was declared to be the same person as. */
  mergedInto?: string | null;
  clips: ClipView[];
}

export interface ParticipantView {
  /** The key voices are assigned to; for a cycle participant it includes the cycle. */
  id: string;
  name: string;
  /** The cycle of the participant; absent for the legacy local list. */
  cycleId?: string;
  /** The id in `cycle.yaml`: what an item's `assignee` refers to. */
  cycleParticipantId?: string;
  /** Number of confirmed voice samples the profile was learned from. */
  sampleCount: number;
  speechMs: number;
}

export interface VoicesData {
  meetingId: string;
  voices: VoiceView[];
  participants: ParticipantView[];
  allConfirmed: boolean;
}

// ---------- Stage 4: cycles, report modes, language-model backends, drafts ----------

export type ReportMode = "fixed" | "template" | "auto" | "by-type";
export type LanguageSetting = "auto" | "pl" | "en";
export type PublishMode = "auto" | "on-approval" | "off";
export type RepoKind = "local" | "clone";
export type LlmKind = "managed" | "external" | "cloud";

/** A registered cycle repository as stored in the settings. */
export interface CycleRepoSetting {
  id: string;
  path: string;
  remote: string | null;
  kind: RepoKind;
}

export interface LastReport {
  cycleId: string;
  mode: ReportMode;
  template?: string | null;
  meetingType?: string | null;
}

/** A registered repository with what is known about its state. */
export interface CycleRepoView extends CycleRepoSetting {
  /** The folder exists. */
  available: boolean;
  /** `YYYY-MM-DD` of the last successful pull or clone; null for a local repository. */
  stateAsOf: string | null;
  /** The last attempt to update the clone failed, so what is shown may be stale. */
  offline: boolean;
  lastError: string | null;
}

export type PullOutcome = "updated" | "upToDate" | "noRemote" | "failed";

export interface PullResult {
  repo: CycleRepoView;
  outcome: PullOutcome;
  error: string | null;
  /**
   * Problems after an update (a cycle that no longer parses, a participant that could not be
   * linked to its voice profile). The update itself stays. With uncommitted cycle edits nothing
   * is pulled and the call fails with the error code `repo_dirty` instead.
   */
  warnings: string[];
}

/** `clone` needs `url`, `local` needs `path` (an existing folder). */
export interface AddRepoRequest {
  kind: RepoKind;
  url?: string;
  path?: string;
}

/** A problem found in a file, with the path of the offending value (e.g. `participants[2].id`). */
export interface CycleIssue {
  path: string;
  message: string;
}

export interface CycleListItem {
  id: string;
  name: string;
  participantsCount: number;
  /** Null when `cycle.yaml` could not be read at all. */
  defaultMode: ReportMode | null;
  /** Usable for a recording: no issues. */
  valid: boolean;
  issues: CycleIssue[];
  repoId: string;
  stateAsOf: string | null;
  offline: boolean;
}

export interface CycleParticipant {
  id: string;
  name: string;
  github?: string;
  discord?: { userId?: string; roleId?: string };
}

export interface CycleTargets {
  github?: { path: string; publish: PublishMode } | null;
  /** `webhook` is a `cred:` reference, never a URL. */
  discord?: { webhook: string; publish: PublishMode; template?: string } | null;
  notion?: { database: string; publish: PublishMode } | null;
}

/** `cycle.yaml` as the editor sees it. */
export interface CycleDto {
  id: string;
  name: string;
  languages: { asr: LanguageSetting; output: LanguageSetting };
  participants: CycleParticipant[];
  glossary: { term: string; hint?: string }[];
  report: {
    defaultMode: ReportMode;
    defaultTemplate: string;
    instruction: string;
    /** Meeting type to template name. */
    byType: Record<string, string>;
  };
  targets: CycleTargets;
}

export interface TemplateSection {
  id: string;
  title: string;
  description: string;
}

export interface TemplateSummary {
  name: string;
  sections: TemplateSection[];
}

export interface TemplateDto {
  name: string;
  sections: TemplateSection[];
  body: string;
}

export interface TemplateCheck {
  valid: boolean;
  /** True when the file was written (only `saveTemplate`). */
  saved: boolean;
  issues: CycleIssue[];
  /** The placeholders a body may use. */
  placeholders: string[];
}

export interface Topic {
  id: string;
  text: string;
  status: "open" | "closed";
  openedIn: string;
  closedIn?: string;
}

export interface CycleDetails {
  cycle: CycleDto;
  repoId: string;
  valid: boolean;
  issues: CycleIssue[];
  templates: TemplateSummary[];
  topics: Topic[];
  /** The meeting types of `cycle.report.byType`. */
  meetingTypes: string[];
  stateAsOf: string | null;
  offline: boolean;
}

export interface SaveCycleResult {
  /** False when the cycle is invalid: nothing was written and `issues` says why. */
  saved: boolean;
  issues: CycleIssue[];
  /** Problems after a successful write (voice profiles that could not be linked). */
  warnings: string[];
  details: CycleDetails | null;
}

/** What a recording starts with. Every field but `cycleId` and `mode` is optional. */
export interface ReportChoice {
  cycleId: string;
  mode: ReportMode;
  /** For `template` mode; absent uses the cycle's default template. */
  template?: string | null;
  /** For `by-type` mode; must be one of the cycle's meeting types. */
  meetingType?: string | null;
  outputLanguage?: LanguageSetting | null;
}

export interface ReportDefaults {
  cycleId: string;
  defaultMode: ReportMode;
  defaultTemplate: string;
  defaultMeetingType: string | null;
  outputLanguage: LanguageSetting;
  templates: TemplateSummary[];
  meetingTypes: { name: string; template: string }[];
  /** The last choice, when it was made for this cycle. */
  last: LastReport | null;
}

export interface ManagedLlmSettings {
  /** Path to the GGUF file. */
  modelPath: string;
  /** Context size in tokens (2048 or more). */
  ctx: number;
}

export interface EndpointSettings {
  baseUrl: string;
  model: string;
}

export interface LlmSettings {
  /** `managed` or `external`; the cloud is always chosen per meeting. */
  defaultKind: Exclude<LlmKind, "cloud">;
  managed: ManagedLlmSettings;
  external: EndpointSettings;
  cloud: EndpointSettings;
  /** Whether a key is stored in the Credential Manager. Keys are never returned. */
  externalKeySet: boolean;
  cloudKeySet: boolean;
  /**
   * Read only: the external server is the default model and is not on this machine or in a
   * private network, so every meeting of a cycle is sent to it without a confirmation.
   */
  externalHostIsRemote: boolean;
}

/** What the cloud confirmation dialog shows. */
export interface CloudPreview {
  endpointHost: string;
  model: string;
  /** Words of the transcript that would be sent, excluded passages left out. */
  wordCount: number;
  screenshotDescriptions: number;
  /**
   * Items (text, assignee, due date) of the cycle's earlier meetings that go along with the
   * request, whichever model those meetings used. At most 30.
   */
  previousItemCount: number;
  includesAudio: false;
  includesImages: false;
  /** A readable statement of what will be sent where. */
  summaryText: string;
  /** The meeting already has a confirmation for this endpoint. */
  confirmed: boolean;
  /** False while the meeting has no transcript yet (the word count is then 0). */
  transcriptReady: boolean;
}

export type ItemType = "task" | "decision" | "topic" | "note" | "idea" | "budget";
export type CertaintyLevel = "stated" | "inferred" | "unknown";

export interface DraftSection {
  /** Empty for a new section: the application makes an id from the title. */
  id: string;
  title: string;
  description: string;
}

export interface DraftSource {
  track: string;
  speaker: string;
  tStartMs: number;
  tEndMs: number;
  quote: string;
  verified: boolean;
  /** Match score of the quote against the transcript, 0 to 1. */
  match: number;
}

export interface DraftItem {
  id: string;
  type: ItemType;
  text: string;
  /** A participant id of the cycle. */
  assignee: string | null;
  /** `YYYY-MM-DD`. */
  due: string | null;
  certainty: { assignee: CertaintyLevel; due: CertaintyLevel };
  section: string | null;
  relatesTo: string | null;
  /** Null for an item added by hand. */
  source: DraftSource | null;
  edited: boolean;
}

export type DraftWarningCode =
  "window_skipped" | "quote_unverified" | "assignee_unknown" | "summary_failed";

export interface DraftWarning {
  code: DraftWarningCode;
  message: string;
  tStartMs?: number;
  tEndMs?: number;
}

export interface Draft {
  meetingId: string;
  cycle: string;
  date: string;
  language: "pl" | "en";
  mode: ReportMode;
  template: string | null;
  meetingType: string | null;
  sections: DraftSection[];
  summary: string;
  items: DraftItem[];
  warnings: DraftWarning[];
}

/** Everything the draft editor shows. Every draft operation returns a new one. */
export interface DraftView {
  draft: Draft;
  /** The report rendered with the meeting's template, in the language of the draft. */
  reportMarkdown: string;
  /** A re-run of `reduce` left a newer draft next to the edited one. */
  hasNewDraft: boolean;
  /** The participants of the cycle, to pick assignees from. */
  participants: { id: string; name: string }[];
  transcriptPresent: boolean;
  /** Set when the template could not be used and the built-in layout was rendered instead. */
  reportWarning: string | null;
}

/** Absent fields stay as they are; `null` clears `assignee` and `due`. */
export interface DraftItemPatch {
  text?: string;
  assignee?: string | null;
  due?: string | null;
}

export interface NewDraftItem {
  type: ItemType;
  text: string;
  assignee?: string | null;
  due?: string | null;
  section?: string | null;
}

/** `markdown`: GitHub (task checkboxes). `chat`: Discord and Notion (plain bullets). */
export type RenderVariant = "markdown" | "chat";
