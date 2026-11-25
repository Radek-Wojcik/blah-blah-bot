import { describe, expect, it } from "vitest";
import type { ParticipantView, VoiceView } from "./ipc/types";
import {
  formatScore,
  groupVoices,
  hasUnconfirmedAuto,
  mergeRoot,
  namingProgress,
  participantName,
  validateVoiceSettings,
  voiceState,
} from "./speakers";

const voice = (voiceId: string, extra: Partial<VoiceView> = {}): VoiceView => ({
  voiceId,
  track: "app-discord",
  label: `SPEAKER_${voiceId}`,
  speechMs: 60_000,
  decision: "unknown",
  score: null,
  suggestedParticipantId: null,
  assignedParticipantId: null,
  confirmed: false,
  mergedInto: null,
  clips: [],
  ...extra,
});

const people: ParticipantView[] = [
  { id: "p1", name: "Alex Demo", sampleCount: 3, speechMs: 1000 },
  { id: "p2", name: "Kasia Demo", sampleCount: 0, speechMs: 0 },
];

describe("groupVoices", () => {
  it("keeps the order and nests merged voices under their target", () => {
    const voices = [
      voice("0"),
      voice("1", { mergedInto: "0" }),
      voice("2"),
      voice("3", { mergedInto: "2" }),
      voice("4", { mergedInto: "0" }),
    ];
    const groups = groupVoices(voices);
    expect(groups.map((g) => g.voice.voiceId)).toEqual(["0", "2"]);
    expect(groups[0].merged.map((v) => v.voiceId)).toEqual(["1", "4"]);
    expect(groups[1].merged.map((v) => v.voiceId)).toEqual(["3"]);
  });

  it("flattens a chain of merges under the last voice", () => {
    const voices = [voice("0"), voice("1", { mergedInto: "0" }), voice("2", { mergedInto: "1" })];
    const groups = groupVoices(voices);
    expect(groups).toHaveLength(1);
    expect(groups[0].merged.map((v) => v.voiceId)).toEqual(["1", "2"]);
    expect(mergeRoot(voices, voices[2]).voiceId).toBe("0");
  });

  it("does not lose voices whose target is missing or that form a loop", () => {
    const orphan = [voice("0", { mergedInto: "gone" })];
    expect(groupVoices(orphan).map((g) => g.voice.voiceId)).toEqual(["0"]);
    const loop = [voice("0", { mergedInto: "1" }), voice("1", { mergedInto: "0" })];
    expect(groupVoices(loop).length).toBeGreaterThan(0);
  });

  it("is empty without voices", () => {
    expect(groupVoices([])).toEqual([]);
  });
});

describe("formatScore", () => {
  it("shows a whole percentage", () => {
    expect(formatScore(0.83)).toBe("83%");
    expect(formatScore(0.614)).toBe("61%");
    expect(formatScore(1.4)).toBe("100%");
  });
  it("is empty without a score", () => {
    expect(formatScore(null)).toBe("");
    expect(formatScore(undefined)).toBe("");
  });
});

describe("participantName", () => {
  it("finds a name or nothing", () => {
    expect(participantName(people, "p2")).toBe("Kasia Demo");
    expect(participantName(people, "nope")).toBeNull();
    expect(participantName(people, null)).toBeNull();
  });
});

describe("voiceState", () => {
  it("never presents an automatic match without its score", () => {
    const s = voiceState(
      voice("0", { decision: "auto", score: 0.83, assignedParticipantId: "p1" }),
      people,
    );
    expect(s).toEqual({
      kind: "auto",
      label: "Recognized automatically · 83%",
      participantId: "p1",
    });
  });

  it("names the person and the score of a suggestion", () => {
    const s = voiceState(
      voice("1", { decision: "suggested", score: 0.61, suggestedParticipantId: "p2" }),
      people,
    );
    expect(s.kind).toBe("suggested");
    expect(s.label).toBe("Suggested: Kasia Demo? 61%");
    expect(s.participantId).toBe("p2");
  });

  it("shows a rejected suggestion as unknown", () => {
    const v = voice("1", { decision: "suggested", score: 0.61, suggestedParticipantId: "p2" });
    expect(voiceState(v, people, true)).toEqual({
      kind: "unknown",
      label: "Unknown",
      participantId: null,
    });
  });

  it("shows a voice without a match as unknown", () => {
    expect(voiceState(voice("2"), people).kind).toBe("unknown");
  });

  it("lets the decision of the user win over the matcher", () => {
    const named = voiceState(
      voice("0", { decision: "auto", score: 0.8, assignedParticipantId: "p1", confirmed: true }),
      people,
    );
    expect(named).toEqual({ kind: "named", label: "Named by you", participantId: "p1" });
    const skipped = voiceState(voice("3", { confirmed: true }), people);
    expect(skipped.kind).toBe("skipped");
    expect(skipped.participantId).toBeNull();
  });
});

describe("hasUnconfirmedAuto", () => {
  it("is true only for an automatic match that is not confirmed or merged", () => {
    const auto = voice("0", { decision: "auto", assignedParticipantId: "p1" });
    expect(hasUnconfirmedAuto([auto, voice("1")])).toBe(true);
    expect(hasUnconfirmedAuto([{ ...auto, confirmed: true }])).toBe(false);
    expect(hasUnconfirmedAuto([{ ...auto, mergedInto: "1" }, voice("1")])).toBe(false);
    expect(hasUnconfirmedAuto([])).toBe(false);
  });
});

describe("namingProgress", () => {
  it("counts the confirmed voices of the top level", () => {
    const voices = [
      voice("0", { confirmed: true }),
      voice("1", { confirmed: true }),
      voice("2"),
      voice("3"),
    ];
    expect(namingProgress(voices)).toEqual({ named: 2, total: 4, text: "2 of 4 voices named" });
  });

  it("does not count merged voices", () => {
    const voices = [voice("0", { confirmed: true }), voice("1", { mergedInto: "0" })];
    expect(namingProgress(voices).text).toBe("1 of 1 voice named");
  });

  it("handles a meeting without voices", () => {
    expect(namingProgress([]).text).toBe("0 of 0 voices named");
  });
});

describe("validateVoiceSettings", () => {
  const ok = { voiceAccept: 0.7, voiceSuggest: 0.5, voiceMargin: 0.1, clipMaxDays: 30 };

  it("accepts the defaults and the limits", () => {
    expect(validateVoiceSettings(ok)).toBeNull();
    expect(validateVoiceSettings({ ...ok, voiceAccept: 1, voiceSuggest: 1 })).toBeNull();
    expect(
      validateVoiceSettings({ ...ok, voiceAccept: 0, voiceSuggest: 0, voiceMargin: 0 }),
    ).toBeNull();
    expect(validateVoiceSettings({ ...ok, voiceAccept: 0.5, voiceSuggest: 0.5 })).toBeNull();
    expect(validateVoiceSettings({ ...ok, clipMaxDays: 1 })).toBeNull();
  });

  it("rejects thresholds outside 0..1 or in the wrong order", () => {
    expect(validateVoiceSettings({ ...ok, voiceAccept: 1.01 })).not.toBeNull();
    expect(validateVoiceSettings({ ...ok, voiceSuggest: -0.1 })).not.toBeNull();
    expect(validateVoiceSettings({ ...ok, voiceSuggest: 0.8 })).toMatch(/cannot be higher/);
    expect(validateVoiceSettings({ ...ok, voiceAccept: Number.NaN })).not.toBeNull();
  });

  it("rejects a negative margin and clip days below 1 or not whole", () => {
    expect(validateVoiceSettings({ ...ok, voiceMargin: -0.01 })).not.toBeNull();
    expect(validateVoiceSettings({ ...ok, clipMaxDays: 0 })).not.toBeNull();
    expect(validateVoiceSettings({ ...ok, clipMaxDays: 2.5 })).not.toBeNull();
  });

  it("formats cross-project suggestions with Recognised from <projects>", () => {
    const s = voiceState(
      voice("2", {
        decision: "suggested",
        score: 0.78,
        suggestedParticipantId: "NOMAD:ola",
        suggestedParticipantName: "Ola Cross",
        suggestedFromProjects: ["NOMAD", "ATLAS"],
      }),
      people,
    );
    expect(s).toEqual({
      kind: "suggested",
      label: "Recognised from NOMAD, ATLAS: Ola Cross? 78%",
      participantId: "NOMAD:ola",
    });
  });
});
