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
  speakerColors,
  stageLabel,
  stageProgress,
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
