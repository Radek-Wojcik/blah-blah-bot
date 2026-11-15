import { describe, expect, it } from "vitest";
import type {
  MeetingSummary,
  ScreenshotDescription,
  StageView,
  TranscriptSegment,
  TranscriptSpeaker,
} from "./ipc/types";
import {
  buildTimeline,
  canProcess,
  isJobActive,
  jobStateLabel,
  percent,
  segmentAt,
  speakerColors,
  stageLabel,
  stageProgress,
  stageSkipText,
  stageStatusLabel,
  stagesProgress,
} from "./processing";

const stage = (
  status: StageView["status"],
  fraction: number | null = null,
  name: StageView["stage"] = "asr",
): StageView => ({ stage: name, status, attempts: 1, error: null, fraction, message: null });

describe("labels", () => {
  it("names every stage, stage status and job state", () => {
    expect(stageLabel("diarize")).toBe("Speaker separation");
    expect(stageLabel("audio_cleanup")).toBe("Audio cleanup");
    expect(stageLabel("speaker_id")).toBe("Speaker recognition");
    expect(stageLabel("clips")).toBe("Sample clips");
    expect(stageLabel("extract")).toBe("Extract items");
    expect(stageLabel("reduce")).toBe("Build draft");
    expect(stageStatusLabel("failed")).toBe("Failed");
    expect(jobStateLabel("running")).toBe("Processing");
  });
});

describe("percent", () => {
  it("rounds and clamps", () => {
    expect(percent(0.404)).toBe(40);
    expect(percent(-1)).toBe(0);
    expect(percent(7)).toBe(100);
    expect(percent(Number.NaN)).toBe(0);
  });
});

describe("stage progress", () => {
  it("counts done as 1, running by its fraction and the rest as 0", () => {
    expect(stageProgress(stage("done"))).toBe(1);
    expect(stageProgress(stage("running", 0.4))).toBe(0.4);
    expect(stageProgress(stage("running", null))).toBe(0);
    expect(stageProgress(stage("failed", 0.9))).toBe(0);
    expect(stageProgress(stage("pending"))).toBe(0);
  });
  it("averages over all stages", () => {
    const stages = [stage("done"), stage("running", 0.5), stage("pending"), stage("pending")];
    expect(stagesProgress(stages)).toBeCloseTo(0.375);
    expect(stagesProgress([])).toBe(0);
  });
});

describe("canProcess", () => {
  const meeting = (patch: Partial<MeetingSummary>): MeetingSummary => ({
    id: "m1",
    title: "T",
    startedUnixMs: 0,
    durationMs: 1000,
    state: "stopped",
    trackCount: 1,
    screenshotCount: 0,
    job: null,
    ...patch,
  });
  it("allows a meeting without a job and one that is done", () => {
    expect(canProcess(meeting({}), null)).toBe(true);
    expect(canProcess(meeting({ job: "done" }), null)).toBe(true);
    expect(canProcess(meeting({ job: undefined }), null)).toBe(true);
  });
  it("blocks queued, running, waiting and failed jobs", () => {
    for (const job of ["queued", "waiting", "running", "failed"] as const) {
      expect(canProcess(meeting({ job }), null)).toBe(false);
    }
    expect(isJobActive("waiting")).toBe(true);
    expect(isJobActive("failed")).toBe(false);
  });
  it("blocks the meeting that is being recorded", () => {
    expect(canProcess(meeting({ state: "recording" }), null)).toBe(false);
    expect(canProcess(meeting({ state: "recoverable" }), null)).toBe(false);
    expect(canProcess(meeting({ state: "recovered" }), null)).toBe(true);
    expect(canProcess(meeting({}), "m1")).toBe(false);
    expect(canProcess(meeting({}), "m2")).toBe(true);
  });
});

describe("speakerColors", () => {
  const speakers: TranscriptSpeaker[] = [
    { id: "voice-1", label: "Alex", kind: "voice" },
    { id: "me", label: "You", kind: "user" },
    { id: "voice-2", label: "Kasia", kind: "voice" },
  ];
  it("gives the user a fixed colour and every other speaker its own", () => {
    const colors = speakerColors(speakers);
    expect(colors.me).toBe("#00f0ff");
    expect(new Set(Object.values(colors)).size).toBe(3);
  });
  it("is stable for the same speakers", () => {
    expect(speakerColors(speakers)).toEqual(speakerColors(speakers));
  });
  it("cycles when there are more speakers than colours", () => {
    const many = Array.from({ length: 9 }, (_, i) => ({
      id: `v${i}`,
      label: `V${i}`,
      kind: "voice" as const,
    }));
    const colors = speakerColors(many);
    expect(colors.v0).toBe(colors.v6);
  });
});

describe("buildTimeline", () => {
  const seg = (id: number, startMs: number): TranscriptSegment => ({
    id,
    track: "t",
    speaker: "s",
    startMs,
    endMs: startMs + 1000,
    text: `seg ${id}`,
    originalText: null,
    excluded: false,
  });
  const shot = (offsetMs: number): ScreenshotDescription => ({
    file: `img/${offsetMs}.png`,
    offsetMs,
    description: "d",
  });
  it("inserts screenshots at their time position", () => {
    const items = buildTimeline([seg(2, 5000), seg(1, 0), seg(3, 9000)], [shot(6000), shot(20000)]);
    expect(
      items.map((i) => (i.kind === "segment" ? `s${i.segment.id}` : `p${i.screenshot.offsetMs}`)),
    ).toEqual(["s1", "s2", "p6000", "s3", "p20000"]);
  });
  it("puts a screenshot taken before the first segment first", () => {
    const items = buildTimeline([seg(1, 1000)], [shot(0)]);
    expect(items[0].kind).toBe("screenshot");
  });
  it("handles a transcript without screenshots", () => {
    expect(buildTimeline([seg(1, 0)], [])).toHaveLength(1);
  });
});

describe("stageSkipText", () => {
  it("shows the note of a stage that finished without doing anything", () => {
    const skipped = { status: "done" as const, note: "the meeting has no cycle" };
    expect(stageSkipText(skipped)).toBe("Skipped: the meeting has no cycle");
    expect(stageSkipText({ status: "done", note: null })).toBeNull();
    expect(stageSkipText({ status: "running", note: "x" })).toBeNull();
  });
});

describe("segmentAt", () => {
  const seg = (id: number, startMs: number, endMs: number): TranscriptSegment => ({
    id,
    track: "t",
    speaker: "s",
    startMs,
    endMs,
    text: `s${id}`,
    originalText: null,
    excluded: false,
  });
  const segments = [seg(2, 6_000, 18_500), seg(1, 0, 5_800), seg(3, 19_000, 26_000)];

  it("finds the segment that contains the time", () => {
    expect(segmentAt(segments, 6_000)?.id).toBe(2);
    expect(segmentAt(segments, 18_500)?.id).toBe(2);
    expect(segmentAt(segments, 22_000)?.id).toBe(3);
  });

  it("falls back to the segment before the time, then to the first one", () => {
    expect(segmentAt(segments, 5_900)?.id).toBe(1);
    expect(segmentAt(segments, 99_000)?.id).toBe(3);
    expect(segmentAt([seg(9, 10_000, 12_000)], 1_000)?.id).toBe(9);
  });

  it("has nothing for no segments", () => {
    expect(segmentAt([], 0)).toBeNull();
  });
});
