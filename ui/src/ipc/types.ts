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
