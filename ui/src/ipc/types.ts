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
  | "internal";

export interface IpcError {
  code: IpcErrorCode;
  message: string;
}

export interface StartRecordingRequest {
  title?: string;
  sourceIds: string[];
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
  "asr" | "diarize" | "merge" | "speaker_id" | "clips" | "audio_cleanup" | "vision";
export type StageStatus = "pending" | "running" | "done" | "failed";
export type JobState = "queued" | "waiting" | "running" | "failed" | "done";

export interface StageView {
  stage: StageName;
  status: StageStatus;
  attempts: number;
  error: string | null;
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
  id: string;
  name: string;
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
