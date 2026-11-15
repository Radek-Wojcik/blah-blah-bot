import type {
  JobState,
  MeetingSummary,
  ScreenshotDescription,
  StageName,
  StageStatus,
  StageView,
  TranscriptSegment,
  TranscriptSpeaker,
} from "./ipc/types";

const STAGE_LABELS: Record<StageName, string> = {
  asr: "Transcription",
  diarize: "Speaker separation",
  merge: "Merge",
  speaker_id: "Speaker recognition",
  clips: "Sample clips",
  audio_cleanup: "Audio cleanup",
  vision: "Screenshot analysis",
};

const STAGE_STATUS_LABELS: Record<StageStatus, string> = {
  pending: "Pending",
  running: "Running",
  done: "Done",
  failed: "Failed",
};

const JOB_STATE_LABELS: Record<JobState, string> = {
  queued: "Queued",
  waiting: "Waiting",
  running: "Processing",
  failed: "Failed",
  done: "Processed",
};

export function stageLabel(stage: StageName): string {
  return STAGE_LABELS[stage];
}

export function stageStatusLabel(status: StageStatus): string {
  return STAGE_STATUS_LABELS[status];
}

export function jobStateLabel(state: JobState): string {
  return JOB_STATE_LABELS[state];
}

/** Whole percent of a 0..1 fraction, clamped. */
export function percent(fraction: number): number {
  if (!Number.isFinite(fraction)) return 0;
  return Math.round(Math.min(1, Math.max(0, fraction)) * 100);
}

/** Progress of one stage in 0..1: done counts fully, a running stage by its live fraction. */
export function stageProgress(stage: Pick<StageView, "status" | "fraction">): number {
  if (stage.status === "done") return 1;
  if (stage.status === "running") return Math.min(1, Math.max(0, stage.fraction ?? 0));
  return 0;
}

/** Overall progress of a list of stages in 0..1 (each stage weighs the same). */
export function stagesProgress(stages: StageView[]): number {
  if (stages.length === 0) return 0;
  return stages.reduce((sum, s) => sum + stageProgress(s), 0) / stages.length;
}

/** A job that is queued, waiting or running blocks a second Process request. */
export function isJobActive(state: JobState | null | undefined): boolean {
  return state === "queued" || state === "waiting" || state === "running";
}

/**
 * Whether Process may be started for a meeting: not while it is being recorded (or still needs
 * recovery) and not while it has a job that is active or failed (a failed job is retried from
 * the queue).
 */
export function canProcess(meeting: MeetingSummary, recordingMeetingId: string | null): boolean {
  if (meeting.state === "recording" || meeting.state === "recoverable") return false;
  if (recordingMeetingId !== null && recordingMeetingId === meeting.id) return false;
  const job = meeting.job ?? null;
  return job === null || job === "done";
}

const SPEAKER_PALETTE = ["#ff2a6d", "#fcee0a", "#05ffa1", "#b388ff", "#ff9e4a", "#4d9fff"];
const USER_COLOR = "#00f0ff";

/** Stable colour per speaker: the user is always cyan, the others cycle through the palette. */
export function speakerColors(speakers: TranscriptSpeaker[]): Record<string, string> {
  const colors: Record<string, string> = {};
  let next = 0;
  for (const s of speakers) {
    if (s.kind === "user") {
      colors[s.id] = USER_COLOR;
    } else {
      colors[s.id] = SPEAKER_PALETTE[next % SPEAKER_PALETTE.length];
      next += 1;
    }
  }
  return colors;
}

export type TimelineItem =
  | { kind: "segment"; segment: TranscriptSegment }
  | { kind: "screenshot"; screenshot: ScreenshotDescription };

/**
 * Segments in time order with every screenshot inserted before the first segment that
 * starts after the screenshot was taken (screenshots after the last segment go last).
 */
export function buildTimeline(
  segments: TranscriptSegment[],
  screenshots: ScreenshotDescription[],
): TimelineItem[] {
  const segs = [...segments].sort((a, b) => a.startMs - b.startMs || a.id - b.id);
  const shots = [...screenshots].sort((a, b) => a.offsetMs - b.offsetMs);
  const items: TimelineItem[] = [];
  let s = 0;
  for (const segment of segs) {
    while (s < shots.length && shots[s].offsetMs <= segment.startMs) {
      items.push({ kind: "screenshot", screenshot: shots[s] });
      s += 1;
    }
    items.push({ kind: "segment", segment });
  }
  for (; s < shots.length; s += 1) items.push({ kind: "screenshot", screenshot: shots[s] });
  return items;
}
