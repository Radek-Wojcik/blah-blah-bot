import type {
  AddRepoRequest,
  BridgeStatus,
  CloudPreview,
  CycleDetails,
  CycleDto,
  CycleListItem,
  CycleRepoView,
  DraftItemPatch,
  DraftSection,
  DraftView,
  MeetingDetails,
  MeetingSummary,
  ModelStatus,
  ModelsProgress,
  ProcessOptions,
  ProcessingSettings,
  PullResult,
  QueueState,
  Region,
  RecordingState,
  RenderVariant,
  ReportDefaults,
  ReportMode,
  SaveCycleResult,
  SegmentEdit,
  Settings,
  SourceInfo,
  StartRecordingRequest,
  TemplateCheck,
  TemplateDto,
  TemplateSummary,
  TranscriptData,
  NewDraftItem,
  ParticipantView,
  VoicesData,
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
  /** Save the chosen region, or only close the overlay when `region` is null. */
  overlayFinish(region: Region | null): Promise<void>;

  getSettings(): Promise<Settings>;
  saveSettings(settings: Settings): Promise<void>;

  /** The bridge to the Chrome extension: server, pairing code, paired extensions, tab streams. */
  bridgeStatus(): Promise<BridgeStatus>;
  /** Offer a new pairing code (5 minutes, single use). Rejects with `bridge_off` when not listening. */
  startPairing(): Promise<BridgeStatus>;
  cancelPairing(): Promise<BridgeStatus>;
  /** Forget a paired extension and close its connection. */
  unpairClient(clientId: string): Promise<BridgeStatus>;

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
  listTemplates(cycleId: string): Promise<TemplateSummary[]>;
  getTemplate(cycleId: string, name: string): Promise<TemplateDto>;
  saveTemplate(cycleId: string, template: TemplateDto): Promise<TemplateCheck>;
  validateTemplate(template: TemplateDto): Promise<TemplateCheck>;
  /** Cloning runs in the background; the promise resolves when it is done. */
  addCycleRepo(req: AddRepoRequest): Promise<CycleRepoView>;
  pullCycleRepo(repoId: string): Promise<PullResult>;
  /** The cycle's default mode, template, type and language, the allowed choices and the last choice. */
  reportDefaults(cycleId: string): Promise<ReportDefaults>;

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

  onRecordingState(cb: (state: RecordingState) => void): Promise<Unsubscribe>;
  onMeetingsChanged(cb: () => void): Promise<Unsubscribe>;
  onQueueChanged(cb: (state: QueueState) => void): Promise<Unsubscribe>;
  onModelsProgress(cb: (progress: ModelsProgress) => void): Promise<Unsubscribe>;
  onModelsChanged(cb: () => void): Promise<Unsubscribe>;
  /** Payload: the id of the meeting whose draft changed. */
  onDraftChanged(cb: (meetingId: string) => void): Promise<Unsubscribe>;
  /** A cycle, template or repository changed. */
  onCyclesChanged(cb: () => void): Promise<Unsubscribe>;
  onBridgeChanged(cb: (status: BridgeStatus) => void): Promise<Unsubscribe>;
}
