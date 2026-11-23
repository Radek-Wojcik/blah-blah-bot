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
  /** The desktop capture of the screenshot overlay is black or fully transparent. */
  | "blank_capture"
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
  | "draft_invalid"
  // Stage 5: publishing.
  /** The draft is read-only: operations for the meeting are in the outbox. */
  | "published"
  /** The repository is public (or its visibility is unknown) and the user has not confirmed it. */
  | "public_repo_unconfirmed"
  | "discord_webhook_missing"
  | "nothing_to_publish"
  /** A newer draft waits for a decision. */
  | "new_draft_pending"
  /** The value is not a Discord webhook; the message never repeats it. */
  | "invalid_webhook"
  /** The value does not have the shape of a Discord bot token; the message never repeats it. */
  | "invalid_bot_token"
  /** The value does not have the shape of a Notion integration token; never repeated. */
  | "invalid_notion_token"
  | "discord_bot_token_missing"
  | "notion_token_missing"
  /** The cycle has no such target (a Discord target needs a webhook or a bot, Notion a token). */
  | "no_target"
  /** A correction needs a published (locked) meeting. */
  | "not_published"
  /** A correction needs the meeting in the repository, and it is not there. */
  | "not_in_repository"
  /** The correction was queued for publishing: it is part of the record and stays. */
  | "correction_queued";

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
  /** `owner/name` of the GitHub repository when it cannot be taken from the remote. */
  githubRepo?: string | null;
  /** API address of a GitHub Enterprise server; absent means github.com. */
  githubApi?: string | null;
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

/** The Discord user or role a participant is mentioned as: exactly one of the two. */
export interface DiscordRef {
  userId?: string;
  roleId?: string;
}

export interface CycleParticipant {
  id: string;
  name: string;
  github?: string;
  discord?: DiscordRef;
}

/** `targets.discord.bot`: a bot that posts and opens a thread per meeting. */
export interface DiscordBotTarget {
  /** A `cred:` reference to the bot token, never the token. */
  token: string;
  /** The server id (digits, kept as text). */
  guild: string;
  /** The text or announcement channel id (digits, kept as text). */
  channel: string;
  /** One thread per meeting under the summary. Default true. */
  thread: boolean;
  /** Edit the summary when a correction is published. Default true. */
  editOnCorrection: boolean;
}

/** The names of the Notion properties the task fields go to. */
export interface NotionProperties {
  /** Type `title`. Default "Name". */
  title: string;
  /** Type `rich_text`, holds the idempotency key. Default "BBB key". */
  key: string;
  assignee?: string;
  due?: string;
  status?: string;
  /** The status a new page gets; required when `status` is set. */
  statusValue?: string;
  meeting?: string;
  cycle?: string;
}

export interface CycleTargets {
  /** `issues` (default true): create issues for tasks and write back status changes. */
  github?: { path: string; publish: PublishMode; issues?: boolean } | null;
  /** Exactly one of `webhook` and `bot`. `webhook` is a `cred:` reference, never a URL. */
  discord?: {
    webhook?: string;
    bot?: DiscordBotTarget;
    publish: PublishMode;
    template?: string;
  } | null;
  notion?: {
    /** A `cred:` reference to the integration token, never the token. */
    token: string;
    /** The database id (32 hex digits, dashes allowed). Not a secret. */
    database: string;
    /** Needed when the database has more than one data source. */
    dataSource?: string;
    publish: PublishMode;
    properties: NotionProperties;
  } | null;
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

export type ProposedStatus = "done" | "open";
export type StatusDecision = "proposed" | "approved" | "rejected";

/** A proposed status change of an open issue or topic. Nothing is written back before it is approved. */
export interface StatusProposal {
  /** `s1`, `s2`, ... */
  id: string;
  /** An open issue (`#12`) or a topic id. */
  target: string;
  status: ProposedStatus;
  reason: string;
  /** Posted with the change; null until the user writes one. */
  comment: string | null;
  /** Null for a proposal added by hand. */
  source: DraftSource | null;
  decision: StatusDecision;
  edited: boolean;
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
  /** Absent while the draft has none. */
  statusProposals?: StatusProposal[];
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
  /** The draft is read-only: the meeting is published or operations for it are queued. */
  locked: boolean;
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

// ---------- Stage 5: publishing ----------

/** A GitHub API host and whether a token is stored for it. The token itself is never returned. */
export interface GithubHost {
  host: string;
  api: string;
  tokenSet: boolean;
}

/**
 * A secret a cycle file refers to as `cred:name`: a Discord webhook (`discord`), a Discord bot
 * token (`discord-bot`) or a Notion integration token (`notion`).
 */
export interface TargetSecret {
  reference: string;
  /** The cycle that uses it. */
  cycleId: string;
  kind: "discord" | "discord-bot" | "notion";
  set: boolean;
  /**
   * A form that is safe to show, never the secret: the host and the ends of the webhook id,
   * `bot <first four characters>...` for a bot token, `ntn_...<last four>` for a Notion token.
   */
  masked: string | null;
}

export interface PublishingSecrets {
  github: GithubHost[];
  targets: TargetSecret[];
}

/** An expected failure is a result with a message, not an error. */
export interface GithubAccess {
  ok: boolean;
  message: string;
  repo: string | null;
  host: string | null;
  tokenSet: boolean;
  login: string | null;
  visibility: "public" | "private" | "internal" | null;
  canPush: boolean | null;
  hasIssues: boolean | null;
}

export interface WebhookCheck {
  ok: boolean;
  message: string;
  name: string | null;
}

export type PublishTargetName = "github" | "discord" | "notion";

export interface PublishTargetView {
  target: PublishTargetName;
  /** The rule of the cycle; null when the cycle has no such target. */
  mode: PublishMode | null;
  /** An explicit publication may use it. */
  available: boolean;
}

export interface PlannedIssue {
  itemId: string;
  title: string;
  assignee: string | null;
  due: string | null;
}

/** One task that exporting to Notion creates a page for, or updates when it is already there. */
export interface PlannedNotionPage {
  itemId: string;
  title: string;
  /** The page of an earlier export, when there is one. */
  existingUrl: string | null;
}

export interface PlannedStatus {
  proposalId: string;
  target: string;
  /** True reopens, false closes. */
  open: boolean;
}

export interface PlannedTopicSet {
  id: string;
  /** True reopens, false closes. */
  open: boolean;
}

/** A topic as the publish plan shows it. */
export interface PlannedTopicView extends TopicVersion {
  openedIn: string;
  closedIn: string | null;
}

/** One topic that publishing changes in `topics.yaml`. */
export interface TopicDelta {
  id: string;
  change: "added" | "changed" | "removed";
  /** What the remote has; null for a topic that is added. */
  before: PlannedTopicView | null;
  /** What the commit writes; null for a topic that is removed. */
  after: PlannedTopicView | null;
  /**
   * Somebody changed the topic on the remote since the last pull: the last write wins, so `after`
   * replaces `before`.
   */
  overridesRemote: boolean;
}

/** `topics.yaml` before and after the commit, per topic, as far as the clone can tell. */
export interface TopicsPlan {
  /** Compared with the remote branch as last fetched. False: only with this clone's own branch. */
  againstRemote: boolean;
  changes: TopicDelta[];
  /** Topics the commit leaves as they are. */
  unchanged: number;
}

export interface PlanSummary {
  commit: boolean;
  pushes: boolean;
  files: string[];
  issues: PlannedIssue[];
  statuses: PlannedStatus[];
  topicsAdded: string[];
  topicsSet: PlannedTopicSet[];
  /**
   * `topics.yaml` before and after, per topic. Null or absent when the commit leaves the file
   * alone or the comparison is not possible (the plan says so in its notes); `topicsAdded` and
   * `topicsSet` then name the topics.
   */
  topics?: TopicsPlan | null;
  discord: boolean;
  /** How the message is sent; null when Discord is not part of the plan. */
  discordMode: "webhook" | "bot" | null;
  /** Bot mode: the rest of the message goes into a thread under the summary. */
  discordThread: boolean;
  /** The tasks exported to Notion, one page each. */
  notion: PlannedNotionPage[];
  /** Status changes that were not approved: they are not written back. */
  proposalsPending: number;
}

export type PlanBlockCode =
  | "public_repo_unconfirmed"
  | "discord_webhook_missing"
  | "discord_bot_token_missing"
  | "notion_token_missing"
  | "nothing_to_publish"
  | "not_in_repository";

export interface PlanBlock {
  code: PlanBlockCode;
  message: string;
  /** For `public_repo_unconfirmed`: `public` or `unknown`. */
  visibility: string | null;
}

export type RepoVisibility = "public" | "private" | "unknown";

export interface RepoPublishInfo {
  key: string | null;
  /** `owner/name` of the GitHub repository, when linked. */
  github: string | null;
  hasRemote: boolean;
  visibility: RepoVisibility;
  confirmed: boolean;
}

export type OpTarget = "github" | "issues" | "discord" | "notion";
export type OpKind =
  | "commit_meeting"
  | "create_issue"
  | "set_issue_state"
  | "post_discord"
  | "commit_correction"
  | "comment_correction"
  | "export_notion"
  | "edit_discord";
export type OpState = "queued" | "running" | "done" | "failed" | "cancelled";

/** One operation of the outbox. `result` is what the operation reported, as JSON. */
export interface OutboxOp {
  id: number;
  meetingId: string;
  target: OpTarget;
  kind: OpKind;
  opKey: string;
  /** What the operation does, for a list. */
  label: string;
  state: OpState;
  attempts: number;
  nextAttemptUnixMs: number | null;
  lastError: string | null;
  result: OpResult | null;
  afterId: number | null;
  createdUnixMs: number;
  updatedUnixMs: number;
}

/** A topic as one side of a `topics.yaml` conflict. */
export interface TopicVersion {
  text: string;
  status: "open" | "closed";
}

/** A topic changed on the remote and by us: ours was kept (last write wins per topic). */
export interface TopicConflictDetail {
  id: string;
  /** The remote's version; null when it was removed meanwhile. */
  theirs: TopicVersion | null;
  /** Our version, the one that was kept; null when we removed the topic. */
  ours: TopicVersion | null;
}

/** The fields the operations report (the keys are written as the backend writes them). */
export interface OpResult {
  folder?: string;
  commit?: string;
  branch?: string;
  pushed?: boolean;
  already_published?: boolean;
  report_url?: string | null;
  local?: "updated" | "behind";
  local_note?: string | null;
  topics?: {
    added: number;
    changed: number;
    removed: number;
    /** The ids of the topics where our version replaced a different change of the remote. */
    conflicts: string[];
    /** With the two versions, when the backend reports them. */
    conflict_details?: TopicConflictDetail[];
  } | null;
  number?: number;
  url?: string;
  sent_chunks?: number;
  /** A correction: the name of the new file in the meeting folder (`corrections-2.md`). */
  file?: string;
  /** A correction: the path of the file in the repository. */
  path?: string;
  /** A correction comment: false when an earlier attempt had posted it. */
  commented?: boolean;
  /** A correction comment or a Discord edit that was left out, and why. */
  skipped?: string;
  /** A Notion export: the page, and what happened to it (one of the three is true). */
  page_id?: string;
  created?: boolean;
  updated?: boolean;
  /** The page was trashed by a person: it is left alone and not made again. */
  removed?: boolean;
  /** More than one page had the task's key: the oldest was used and none was deleted. */
  duplicates?: number;
  /** A Discord post: where it went. In bot mode `message_ids` has every message sent. */
  channel_id?: string;
  message_ids?: string[];
  /** Bot mode: the thread opened under the summary, when the cycle asks for one. */
  thread_id?: string | null;
  /** A Discord edit: the summary was edited, and the reply was posted in the thread. */
  edited?: boolean;
  replied?: boolean;
}

export interface OutboxState {
  ops: OutboxOp[];
  /** Targets that did not answer: operations for them wait until the network is back. */
  offline: OpTarget[];
  nowUnixMs: number;
}

export interface PublishPlan {
  meetingId: string;
  cycleId: string;
  targets: PublishTargetView[];
  summary: PlanSummary;
  blocks: PlanBlock[];
  notes: string[];
  repo: RepoPublishInfo;
  /** The draft is read-only because operations exist for it. */
  locked: boolean;
  /** A newer draft waits for a decision: publish after choosing one. */
  newDraftPending: boolean;
  ops: OutboxOp[];
}

export interface PublishResult {
  plan: PublishPlan;
  /** Operations that were queued by this call. */
  queued: number;
}

export interface PublicRepoConfirmation {
  key: string | null;
  visibility: RepoVisibility;
  /** Operations the check had refused that run again. */
  released: number;
}

export interface CycleTask {
  number: number;
  title: string;
  assignee: string | null;
  url: string | null;
  /** The meeting the issue was made in, when it was made from an item. */
  meeting: string | null;
}

export interface CycleTasks {
  cycleId: string;
  /** `owner/name`, when the repository is linked to GitHub. */
  repo: string | null;
  /** `YYYY-MM-DD` of the last successful read of the issues; null when never read. */
  asOf: string | null;
  open: CycleTask[];
  done: CycleTask[];
}

// ----- corrections to a published meeting -------------------------------------------------------

/** One operation of a correction (the new file, or the comment on the issue). */
export interface CorrectionStep {
  opId: number;
  state: "queued" | "running" | "done" | "failed";
  attempts: number;
  nextAttemptUnixMs: number | null;
  lastError: string | null;
  result: OpResult | null;
}

/**
 * Where a correction stands: `local` (written, not queued), `queued`, `running`, `published` (the
 * file is in the repository) or `failed`.
 */
export type CorrectionState = "local" | "queued" | "running" | "published" | "failed";

/** A note written after a meeting was published. The published draft never changes. */
export interface Correction {
  id: string;
  /** `YYYY-MM-DD`, the day it was written. */
  date: string;
  text: string;
  /** The item of the meeting it corrects. */
  item: string | null;
  /** The issue it corrects, `#12`. */
  issue: string | null;
  state: CorrectionState;
  file: CorrectionStep | null;
  comment: CorrectionStep | null;
  /** Bot mode: the edit of the Discord summary (and the reply in its thread). */
  discordEdit?: CorrectionStep | null;
  /** The name of the file in the meeting folder once published. */
  fileName: string | null;
  url: string | null;
  /** Only a correction that was never queued can be deleted. */
  canDelete: boolean;
}

export interface Corrections {
  meetingId: string;
  cycleId: string;
  /** The draft is locked (published or queued): corrections can be added. */
  locked: boolean;
  /** The meeting is in the repository (or its commit is queued): corrections can be published. */
  inRepository: boolean;
  /** The rule of the `github` target, which decides when a correction is published. */
  rule: PublishMode;
  corrections: Correction[];
}

export interface PublishedCorrections {
  corrections: Corrections;
  /** Operations queued by this call. */
  queued: number;
  notes: string[];
}

// ----- Stage 7: Notion and the Discord bot ------------------------------------------------------

/** The Notion property types the field mapping knows. */
export type NotionPropertyType =
  | "title"
  | "rich_text"
  | "select"
  | "multi_select"
  | "date"
  | "status"
  | "url"
  | "number"
  | "checkbox"
  | "people";

/** An expected failure is a result with a message, not an error. */
export interface NotionAccess {
  ok: boolean;
  message: string;
  /** The name of the integration. */
  botName: string | null;
  databaseTitle: string | null;
  dataSources: { id: string; name: string }[];
  tokenSet: boolean;
}

/** One problem of the field mapping; `field` is a key of `NotionProperties`. */
export interface NotionMappingProblem {
  field: keyof NotionProperties;
  message: string;
}

export interface NotionSchema {
  /** The data source the properties belong to. */
  dataSourceId: string;
  properties: { name: string; type: NotionPropertyType }[];
  /** The mapping the properties suggest. */
  proposed: NotionProperties;
  /** The mapping the cycle has now. */
  current: NotionProperties;
  /** What is wrong with `current`: a missing property or one of a type the field cannot use. */
  problems: NotionMappingProblem[];
}

export type DiscordChannelKind = "text" | "announcement" | "forum";

/** An expected failure is a result with a message, not an error. */
export interface DiscordBotCheck {
  ok: boolean;
  message: string;
  botName: string | null;
  guildName: string | null;
  channelName: string | null;
  channelKind: DiscordChannelKind | null;
  /** Names of the permissions the bot lacks in the channel (`CREATE_PUBLIC_THREADS`, ...). */
  missing: string[];
  tokenSet: boolean;
}

export interface DiscordMember {
  userId: string;
  username: string;
  globalName: string | null;
  nick: string | null;
  bot: boolean;
}

export interface DiscordRole {
  roleId: string;
  name: string;
  mentionable: boolean;
}
