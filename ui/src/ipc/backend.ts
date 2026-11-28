import type { PickFileRequest, PickFolderRequest, SaveFileRequest } from "./pick";
import type {
  AddRepoRequest,
  AnswerOutcome,
  Briefing,
  BridgeStatus,
  CloudPreview,
  Corrections,
  CycleDetails,
  CycleDto,
  CycleListItem,
  CycleTargets,
  CycleTasks,
  CycleRepoView,
  DetectedModels,
  DetectionAnswer,
  DetectionPrompt,
  DetectionSettings,
  DraftItemPatch,
  DraftSection,
  DiscordBotCheck,
  DiscordMember,
  DiscordRef,
  DiscordRole,
  DraftView,
  GithubAccess,
  GlobalVoiceProfile,
  ImportMeetingInput,
  JobView,
  MeetingDetail,
  MeetingDetails,
  MeetingSummary,
  ModelStatus,
  ModelsProgress,
  NotionAccess,
  NotionProperties,
  NotionSchema,
  ProcessOptions,
  GgufFile,
  ProcessingSettings,
  ProjectPrefs,
  ProposedStatus,
  PublicRepoConfirmation,
  PublishMode,
  PublishedCorrections,
  PublishPlan,
  PublishResult,
  PublishTargetName,
  PublishingSecrets,
  OutboxState,
  PullResult,
  QueueGpuOverride,
  QueueState,
  QueueView,
  Region,
  RecordingLevels,
  RecordingState,
  RenderVariant,
  ReportDefaults,
  ReportMode,
  SaveCycleResult,
  SegmentEdit,
  HotkeyStatus,
  Settings,
  SetupPathCheck,
  SetupPathKind,
  SetupProgress,
  SetupStatus,
  SourceInfo,
  StartRecordingRequest,
  StatusDecision,
  TemplateCheck,
  TemplateDto,
  TemplateSummary,
  TranscriptData,
  NewDraftItem,
  ParticipantView,
  VoicesData,
  WebhookCheck,
  GithubRepoRequest,
  GithubRepoStatus,
  WizardCreateProjectInput,
  WizardCreateResult,
} from "./types";

export type Unsubscribe = () => void;

/**
 * Everything the UI may ask of the application. The UI has no direct file, network or
 * database access: it talks to this interface only. There are two implementations, the
 * Tauri one for the real application and a mock one that serves demo data.
 */
export interface Backend {
  listMeetings(): Promise<MeetingSummary[]>;
  getMeeting(id: string): Promise<MeetingDetails>;
  deleteMeeting(id: string): Promise<void>;
  recoverMeeting(id: string): Promise<void>;

  listSources(): Promise<SourceInfo[]>;
  recordingState(): Promise<RecordingState>;
  startRecording(req: StartRecordingRequest): Promise<void>;
  stopRecording(): Promise<void>;
  addSource(sourceId: string): Promise<void>;
  removeSource(sourceId: string): Promise<void>;
  takeScreenshot(): Promise<void>;
  /** The frozen desktop shown by the screenshot overlay, as a URL usable in an img tag. */
  overlayImage(): Promise<string>;
  /**
   * The overlay window is created hidden: call this once the image is decoded (or could not be)
   * to show it, so the live desktop stays visible until the frozen one can take its place.
   */
  overlayReady(): Promise<void>;
  /** Save the chosen region, or only close the overlay when `region` is null. */
  overlayFinish(region: Region | null): Promise<void>;

  getSettings(): Promise<Settings>;
  saveSettings(settings: Settings): Promise<void>;
  /** Unregister the global hotkeys while the user records a new shortcut. Idempotent. */
  suspendHotkeys(): Promise<void>;
  /** Register the saved hotkeys again after a capture; a no-op when not suspended. */
  resumeHotkeys(): Promise<void>;
  /** Which configured hotkeys are not active, with the reason. */
  hotkeyStatus(): Promise<HotkeyStatus>;

  /** The bridge to the Chrome extension: server, pairing code, paired extensions, tab streams. */
  bridgeStatus(): Promise<BridgeStatus>;
  /** Offer a new pairing code (5 minutes, single use). Rejects with `bridge_off` when not listening. */
  startPairing(): Promise<BridgeStatus>;
  cancelPairing(): Promise<BridgeStatus>;
  /** Forget a paired extension and close its connection. */
  unpairClient(clientId: string): Promise<BridgeStatus>;

  /** Meeting detection: which applications raise a prompt and how readily. */
  getDetectionSettings(): Promise<DetectionSettings>;
  /** Returns the settings as stored (names normalized). */
  saveDetectionSettings(settings: DetectionSettings): Promise<DetectionSettings>;
  /** The prompt on offer, for a window that opened after the event. */
  detectionPrompt(): Promise<DetectionPrompt | null>;
  /**
   * Answer the prompt: `record` starts a recording of the microphone and the application with
   * the last report choice (or opens the recorder window when there is none), `not_now` leaves
   * this call alone, `never` stops prompts for the application.
   */
  answerDetection(key: string, answer: DetectionAnswer): Promise<AnswerOutcome>;

  /**
   * Create the processing job of a meeting if missing and start processing. With `options.llm`
   * the backend of this meeting is chosen (and kept); `cloud` rejects with `cloud_not_confirmed`
   * until `confirmCloud` was called. On an existing job a backend chosen now makes the skipped
   * or failed extraction stages run again.
   */
  processMeeting(id: string, options?: ProcessOptions): Promise<void>;
  queueState(): Promise<QueueState>;
  retryJob(id: string): Promise<void>;
  /** Removes a job that is not running. */
  cancelJob(id: string): Promise<void>;
  /** Rejects with the code `not_ready` while the meeting has no transcript. */
  getTranscript(id: string): Promise<TranscriptData>;
  editSegment(id: string, segmentId: number, edit: SegmentEdit): Promise<void>;
  /** A screenshot of a meeting, as a URL usable in an img tag. */
  meetingImage(id: string, file: string): Promise<string>;

  /** The detected voices of a meeting with their match, sample clips and the known participants. */
  getMeetingVoices(id: string): Promise<VoicesData>;
  /** Name a voice after a participant; `null` leaves it deliberately unnamed. */
  assignVoice(id: string, voiceId: string, participantId: string | null): Promise<VoicesData>;
  /** Name a voice after a new participant (or the existing one with the same name). */
  assignVoiceNew(id: string, voiceId: string, name: string): Promise<VoicesData>;
  /** Declare a voice to be the same person as another one; `null` undoes it. */
  mergeVoices(id: string, voiceId: string, into: string | null): Promise<VoicesData>;
  /** Accept every automatic match of a meeting. */
  confirmMatches(id: string): Promise<VoicesData>;
  /** A sample clip as a WAV, as a URL usable for an audio element. Revoke it when done. */
  voiceClip(id: string, voiceId: string, idx: number): Promise<string>;
  /**
   * The participants of `cycleId`, else of the cycle of `meetingId`, else the legacy local list
   * (people named before cycles existed).
   */
  listParticipants(cycleId?: string, meetingId?: string): Promise<ParticipantView[]>;
  /** In a cycle the person is also written to `cycle.yaml`. */
  createParticipant(name: string, cycleId?: string): Promise<ParticipantView>;
  renameParticipant(participantId: string, name: string): Promise<void>;
  deleteParticipant(participantId: string): Promise<void>;
  /** Forget the voice of a participant but keep the name. */
  resetProfile(participantId: string): Promise<void>;
  /** Delete every profile, stored voice embedding and sample clip. */
  deleteVoiceData(): Promise<void>;

  getProcessingSettings(): Promise<ProcessingSettings>;
  saveProcessingSettings(settings: ProcessingSettings): Promise<void>;
  /** The token is write-only: it is never returned, only `hfTokenSet` tells it is stored. */
  setHfToken(token: string): Promise<void>;
  clearHfToken(): Promise<void>;
  modelStatus(): Promise<ModelStatus[]>;
  downloadModels(keys: string[]): Promise<void>;

  /** What is set up and what is not, item by item. Never returns a token. */
  setupStatus(): Promise<SetupStatus>;
  /** `llama-server` programs found on this machine. Detection only. */
  detectLlamaServer(): Promise<string[]>;
  /** GGUF files in `dir`, or in the usual model folders without it. */
  findGgufModels(dir?: string): Promise<GgufFile[]>;
  /** Checks a path before it is saved; reads only. */
  checkSetupPath(kind: SetupPathKind, path: string): Promise<SetupPathCheck>;
  /** Starts creating the processing environment; follow `onSetupProgress`. */
  prepareWorkerEnv(): Promise<void>;
  /** Downloads the pinned llama-server release; follow `onSetupProgress`. */
  downloadLlamaServer(): Promise<void>;
  /** Downloads the default language model GGUF; follow `onSetupProgress`. */
  downloadLanguageModel(): Promise<void>;
  /** Stops the currently running setup task. */
  cancelSetupTask(): Promise<void>;
  /** Marks the first-run setup as completed (default) or to be shown again. */
  setFirstRunDone(done?: boolean): Promise<void>;
  /** Dismisses the one-time notice about a legacy Roaming `worker-venv` folder. */
  dismissLegacyVenvNotice(): Promise<void>;

  /** API keys are write-only: `ProcessingSettings.llm.cloudKeySet` / `externalKeySet` tell they are stored. */
  setCloudKey(key: string): Promise<void>;
  clearCloudKey(): Promise<void>;
  cloudKeyPresent(): Promise<boolean>;
  setExternalKey(key: string): Promise<void>;
  clearExternalKey(): Promise<void>;
  externalKeyPresent(): Promise<boolean>;
  /** What would be sent to the cloud endpoint for this meeting. Reads only. */
  cloudPreview(meetingId: string): Promise<CloudPreview>;
  /** Stores the user's confirmation. Call it only from the dialog's confirm button. */
  confirmCloud(meetingId: string): Promise<void>;

  /** Cycles of every registered repository, broken ones included (with their issues). */
  listCycles(): Promise<CycleListItem[]>;
  listCycleRepos(): Promise<CycleRepoView[]>;
  getCycle(cycleId: string): Promise<CycleDetails>;
  /** Without `repoId` the cycle goes to the default local repository (created on first use). */
  createCycle(id: string, name: string, repoId?: string): Promise<CycleDetails>;
  /** An invalid cycle is not written: `saved` is false and `issues` says why. */
  saveCycle(cycle: CycleDto): Promise<SaveCycleResult>;
  /** Deletes a project from the database and settings, optionally deleting meetings or recycling the folder. */
  deleteProject(cycleId: string, deleteMeetings: boolean, deleteFolder: boolean): Promise<void>;
  listTemplates(cycleId: string): Promise<TemplateSummary[]>;
  getTemplate(cycleId: string, name: string): Promise<TemplateDto>;
  saveTemplate(cycleId: string, template: TemplateDto): Promise<TemplateCheck>;
  validateTemplate(template: TemplateDto): Promise<TemplateCheck>;
  /** Cloning runs in the background; the promise resolves when it is done. */
  addCycleRepo(req: AddRepoRequest): Promise<CycleRepoView>;
  pullCycleRepo(repoId: string): Promise<PullResult>;
  /** The cycle's default mode, template, type and language, the allowed choices and the last choice. */
  reportDefaults(cycleId: string): Promise<ReportDefaults>;
  /**
   * The briefing of a cycle: open tasks by person and open topics. With `refresh` a cloned
   * repository is pulled first (never over uncommitted cycle edits); offline or after a failed
   * pull the answer is the state of the last successful pull, with its date.
   */
  getBriefing(cycleId: string, refresh: boolean): Promise<Briefing>;
  /** Marks a project task done or open in local storage (and closes GitHub issue via outbox if applicable). */
  setTaskDone(cycleId: string, taskKey: string, done: boolean): Promise<void>;
  /** Show the main window on the briefing of a cycle (for the recorder window). */
  openBriefing(cycleId: string): Promise<void>;

  /** Rejects with `no_draft` until the reduce stage has written the draft. */
  getDraft(meetingId: string): Promise<DraftView>;
  updateDraftItem(meetingId: string, itemId: string, patch: DraftItemPatch): Promise<DraftView>;
  /** `sectionId: null` takes the item out of every section; without `beforeItemId` it goes last. */
  moveDraftItem(
    meetingId: string,
    itemId: string,
    sectionId: string | null,
    beforeItemId?: string,
  ): Promise<DraftView>;
  deleteDraftItem(meetingId: string, itemId: string): Promise<DraftView>;
  addDraftItem(meetingId: string, item: NewDraftItem): Promise<DraftView>;
  /** Rename, add, remove and reorder in one call; a section with an empty id gets one. */
  setDraftSections(meetingId: string, sections: DraftSection[]): Promise<DraftView>;
  /** Layout only: no item is touched. Rejects with `report_mode_required` for an unknown template or type. */
  setReportMode(
    meetingId: string,
    mode: ReportMode,
    template?: string,
    meetingType?: string,
  ): Promise<DraftView>;
  /** Replace the edited draft by the newer one a re-run of reduce left. */
  acceptNewDraft(meetingId: string): Promise<DraftView>;
  /** Keep the edited draft and throw the newer one away. */
  discardNewDraft(meetingId: string): Promise<DraftView>;
  renderReport(meetingId: string, variant: RenderVariant): Promise<string>;
  /**
   * Decide a proposed status change. Nothing is written back before it is `approved`. `comment`
   * absent keeps the text, an empty one clears it. Rejects with `published` while the draft is
   * locked.
   */
  setStatusDecision(
    meetingId: string,
    proposalId: string,
    decision: StatusDecision,
    comment?: string,
  ): Promise<DraftView>;
  /** Add a status change by hand: `#12` for an issue, else the id of a topic. It waits for approval. */
  addStatusProposal(
    meetingId: string,
    target: string,
    status: ProposedStatus,
    comment?: string,
  ): Promise<DraftView>;

  /**
   * Which GitHub tokens and webhooks are set, never their values, with the GitHub hosts of the
   * registered repositories and the `cred:` references of the cycles.
   */
  publishingSecrets(): Promise<PublishingSecrets>;
  /** Write-only. `api` absent means github.com. A token with spaces or line breaks is refused. */
  setGithubToken(token: string, api?: string): Promise<void>;
  clearGithubToken(api?: string): Promise<void>;
  /** Checks the stored token against the GitHub repository of the cycle. A failure is a result, not an error. */
  testGithubAccess(cycleId: string): Promise<GithubAccess>;
  /** Links a registered repository to a GitHub repository (`owner/name`); empty values clear the link. */
  setGithubRepo(repoId: string, githubRepo?: string, githubApi?: string): Promise<CycleRepoView>;
  /**
   * Write-only value of a `cred:` reference. The value is checked against what the cycles use the
   * reference for and rejects with `invalid_webhook` (a Discord webhook), `invalid_bot_token` (a
   * Discord bot token) or `invalid_notion_token` (a Notion integration token); the message never
   * repeats the value.
   */
  setTargetSecret(reference: string, value: string): Promise<void>;
  clearTargetSecret(reference: string): Promise<void>;
  /** Asks Discord whether the stored webhook exists. Nothing is posted. */
  testDiscordWebhook(reference: string): Promise<WebhookCheck>;

  /**
   * Sets when a target of a cycle is published (`targets.<target>.publish` of `cycle.yaml`) and
   * nothing else; returns the cycle's targets as saved. `github` covers the report, the issues
   * and the write-back of approved status changes, `discord` needs a Discord target
   * (`no_target`), and so does `notion`. No value writes a status change back without approval and an explicit
   * publication, and the public-repository check applies to every value.
   */
  setTargetRule(
    cycleId: string,
    target: PublishTargetName,
    mode: PublishMode,
  ): Promise<CycleTargets>;

  /**
   * Asks Notion whether the stored token can reach the database of the cycle's Notion target.
   * Nothing is written. An expected failure is a result with a message, not an error; a cycle
   * without a Notion target rejects with `no_target`.
   */
  testNotionAccess(cycleId: string): Promise<NotionAccess>;
  /**
   * The properties of the Notion database, the mapping they suggest and what is wrong with the
   * mapping the cycle has. Rejects with `no_target`, or `secrets` when no token is stored.
   */
  notionSchema(cycleId: string): Promise<NotionSchema>;
  /**
   * Writes the data source and the property names of the cycle's Notion target and nothing else
   * (`null` leaves no data source set); returns the cycle's targets as saved. Rejects with
   * `no_target`.
   */
  setNotionMapping(
    cycleId: string,
    dataSource: string | null,
    properties: NotionProperties,
  ): Promise<CycleTargets>;
  /**
   * Checks the bot token against the guild and channel of the cycle's Discord bot target and
   * lists the permissions it lacks in the channel. Nothing is posted. An expected failure is a
   * result with a message, not an error.
   */
  testDiscordBot(cycleId: string): Promise<DiscordBotCheck>;
  /**
   * Members of the server whose user name or nickname starts with `query` (2 characters at
   * least, 10 results at most). Needs a bot target and a stored token.
   */
  searchDiscordMembers(cycleId: string, query: string): Promise<DiscordMember[]>;
  /** The roles of the server (without @everyone), for a participant that is a role. */
  listDiscordRoles(cycleId: string): Promise<DiscordRole[]>;
  /**
   * Writes `participants[].discord` of one participant and nothing else; `null` removes the
   * mapping. Returns the cycle as saved.
   */
  setParticipantDiscord(
    cycleId: string,
    participantId: string,
    discord: DiscordRef | null,
  ): Promise<CycleDto>;
  /**
   * Exports the tasks of a published meeting to Notion again: the mapped values are updated, the
   * status a person set in Notion is kept, and a page they trashed is not made again. Rejects with
   * `not_published`, `nothing_to_publish` (the rule is Off) and the blocks of the plan.
   */
  reexportNotion(meetingId: string): Promise<PublishResult>;
  /** What publishing to `targets` (all of the cycle's when absent) would do. Changes nothing. */
  publishPlan(meetingId: string, targets?: string[]): Promise<PublishPlan>;
  /**
   * Queue the publication. Rejects with `public_repo_unconfirmed` until `confirmPublicRepo`, with
   * `new_draft_pending` while a newer draft waits, with `nothing_to_publish`. From then on the draft is read-only.
   */
  publishMeeting(meetingId: string, targets: string[]): Promise<PublishResult>;
  /** The user accepts publishing to the repository of the cycle although it is (or may be) public. */
  confirmPublicRepo(cycleId: string): Promise<PublicRepoConfirmation>;
  outboxState(): Promise<OutboxState>;
  /** Retries one operation now, or (without an id) everything that failed or waits for its backoff. */
  retryOutboxOp(id?: number): Promise<OutboxState>;
  /** Drops a queued or failed operation. Cancelling all operations of a meeting unlocks its draft. */
  cancelOutboxOp(id: number): Promise<OutboxState>;
  /** Open and done tasks of a cycle as the last read of its GitHub issues has them. */
  cycleTasks(cycleId: string): Promise<CycleTasks>;

  /** The corrections written for a meeting after it was published, and where each stands. */
  listCorrections(meetingId: string): Promise<Corrections>;
  /**
   * Write a correction note for a published meeting: text, the item and/or issue it is about, and
   * the day (today by default). Rejects with `not_published` for a meeting that is not locked.
   * Under an Automatic rule of the `github` target it is queued at once.
   */
  addCorrection(
    meetingId: string,
    text: string,
    item?: string,
    issue?: string,
    date?: string,
  ): Promise<Corrections>;
  /**
   * Queue corrections (all that are not queued when `ids` is absent): a new file in the meeting's
   * folder of the repository and a comment on the issue they are about. Same rule and checks as
   * the meeting: rejects with `public_repo_unconfirmed`, `not_in_repository` and
   * `nothing_to_publish` (Off).
   */
  publishCorrections(meetingId: string, ids?: string[]): Promise<PublishedCorrections>;
  /** Deletes a correction that was never queued (`correction_queued` otherwise). */
  deleteCorrection(meetingId: string, id: string): Promise<Corrections>;

  /** Persist last-selected stable source keys in settings.json. */
  saveLastSources(keys: string[]): Promise<string[]>;
  /** Set or clear the temporary in-memory GPU threshold override for the processing queue. */
  setQueueGpuOverride(overrideCfg: QueueGpuOverride | null): Promise<QueueView>;
  /** Bypass the GPU guard and start processing a meeting immediately. */
  forceProcess(meetingId: string): Promise<JobView>;
  /** Schedule a queued job for a future timestamp (Unix ms), or clear the schedule with null. */
  scheduleJob(meetingId: string, scheduledUnixMs: number | null): Promise<JobView>;
  /** Read per-project local preferences (local markdown export and model overrides). */
  getProjectPrefs(cycleId: string): Promise<ProjectPrefs>;
  /** Save per-project local preferences. */
  saveProjectPrefs(prefs: ProjectPrefs): Promise<ProjectPrefs>;
  /** Guided project creation wizard (auto-managed local repo or existing folder/remote). */
  wizardCreateProject(req: WizardCreateProjectInput): Promise<CycleDto>;
  /** Whether a GitHub token is saved (a boolean only): decides if the wizard offers repository creation. */
  githubRepoStatus(): Promise<GithubRepoStatus>;
  /**
   * Create a new project folder and, when `github.confirmed` and a token is saved, a PRIVATE GitHub
   * repository that becomes its remote. No token, a refused request or no connection is not an
   * error: the local project is created and `github` carries the reason and a message.
   */
  wizardCreateProjectWithGithub(
    req: WizardCreateProjectInput,
    github: GithubRepoRequest,
  ): Promise<WizardCreateResult>;
  /** Import an external audio or video recording into a project and enqueue processing. */
  importMeeting(req: ImportMeetingInput): Promise<MeetingDetail>;
  /** Export a standalone Markdown report note to disk and return the written path. */
  exportMeetingMarkdown(meetingId: string, destPath?: string | null): Promise<string>;
  /** Write the note of a meeting to a `.md` file chosen in the Save as dialog; returns the path. */
  saveMeetingNoteAs(meetingId: string, destPath: string): Promise<string>;
  /** Native open dialog for one file; the full path, or null when the user cancelled. */
  pickFile(req: PickFileRequest): Promise<string | null>;
  /** Native folder dialog; the full path, or null when the user cancelled. */
  pickFolder(req?: PickFolderRequest): Promise<string | null>;
  /** Native Save as dialog; the full path of the file to write, or null when cancelled. */
  saveFileAs(req: SaveFileRequest): Promise<string | null>;
  /** Scan common local folders for llama-server, GGUF models, and default local save path. */
  autoDetectModels(): Promise<DetectedModels>;
  /** List all voice profiles across all projects with project memberships and sample clip state. */
  listAllVoiceProfiles(): Promise<GlobalVoiceProfile[]>;
  /** Read a participant's preserved sample clip WAV bytes. */
  voiceProfileSampleClip(participantId: string): Promise<number[]>;
  /** Manually enroll a 15-30 s voice sample for a participant. */
  enrollVoiceProfile(
    name: string,
    cycleId: string | null,
    audioPath: string,
  ): Promise<GlobalVoiceProfile>;
  /** Link an existing voice profile to a project/cycle. */
  addProfileToProject(participantId: string, cycleId: string): Promise<GlobalVoiceProfile>;
  /** Unlink a voice profile from a project/cycle (keeps global profile & samples intact). */
  removeProfileFromProject(participantId: string, cycleId: string): Promise<GlobalVoiceProfile>;
  /** Merge two voice profiles (`dropId` into `keepId`). */
  mergeParticipants(keepId: string, dropId: string): Promise<GlobalVoiceProfile>;
  /** Undo a naming decision on a voice, optionally unlinking from `unlinkCycleId`. */
  undoVoiceDecision(
    meetingId: string,
    voiceId: string,
    unlinkCycleId?: string | null,
  ): Promise<VoicesData>;

  onRecordingState(cb: (state: RecordingState) => void): Promise<Unsubscribe>;
  onRecordingLevels(cb: (levels: RecordingLevels) => void): Promise<Unsubscribe>;
  onMeetingsChanged(cb: () => void): Promise<Unsubscribe>;
  onQueueChanged(cb: (state: QueueState) => void): Promise<Unsubscribe>;
  onModelsProgress(cb: (progress: ModelsProgress) => void): Promise<Unsubscribe>;
  onSetupProgress(cb: (progress: SetupProgress) => void): Promise<Unsubscribe>;
  onModelsChanged(cb: () => void): Promise<Unsubscribe>;
  /** Payload: the id of the meeting whose draft changed. */
  onDraftChanged(cb: (meetingId: string) => void): Promise<Unsubscribe>;
  /** A cycle, template or repository changed. */
  onCyclesChanged(cb: () => void): Promise<Unsubscribe>;
  /** Payload: the whole state of the outbox, after every change. */
  onOutboxChanged(cb: (state: OutboxState) => void): Promise<Unsubscribe>;
  /** Payload: the cycle whose briefing `openBriefing` asked to show. */
  onOpenBriefing(cb: (cycleId: string) => void): Promise<Unsubscribe>;
  onBridgeChanged(cb: (status: BridgeStatus) => void): Promise<Unsubscribe>;
  onDetectionPrompt(cb: (prompt: DetectionPrompt) => void): Promise<Unsubscribe>;
  /** Payload: the key of the call whose prompt was withdrawn. */
  onDetectionCleared(cb: (key: string) => void): Promise<Unsubscribe>;
}
