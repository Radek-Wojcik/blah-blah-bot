import type {
  MeetingDetails,
  MeetingSummary,
  ModelStatus,
  ModelsProgress,
  ProcessOptions,
  ProcessingSettings,
  QueueState,
  Region,
  RecordingState,
  SegmentEdit,
  Settings,
  SourceInfo,
  StartRecordingRequest,
  TranscriptData,
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

  /** Create the processing job of a meeting if missing and start processing. */
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
  listParticipants(): Promise<ParticipantView[]>;
  createParticipant(name: string): Promise<ParticipantView>;
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

  onRecordingState(cb: (state: RecordingState) => void): Promise<Unsubscribe>;
  onMeetingsChanged(cb: () => void): Promise<Unsubscribe>;
  onQueueChanged(cb: (state: QueueState) => void): Promise<Unsubscribe>;
  onModelsProgress(cb: (progress: ModelsProgress) => void): Promise<Unsubscribe>;
  onModelsChanged(cb: () => void): Promise<Unsubscribe>;
}
