import type { ParticipantView, ProcessingSettings, VoiceView } from "./ipc/types";
import { percent } from "./processing";

/** A voice that is shown as a card, with the voices declared to be the same person. */
export interface VoiceGroup {
  voice: VoiceView;
  merged: VoiceView[];
}

/**
 * The voice a chain of merges ends in. A loop (the backend refuses to create one) or a target
 * that is not in the list ends the walk at the last known voice.
 */
export function mergeRoot(voices: VoiceView[], voice: VoiceView): VoiceView {
  const byId = new Map(voices.map((v) => [v.voiceId, v]));
  let current = voice;
  for (let i = 0; i < voices.length; i += 1) {
    const next = current.mergedInto ? byId.get(current.mergedInto) : undefined;
    if (!next || next.voiceId === voice.voiceId) break;
    current = next;
  }
  return current;
}

/** Top-level voices in their given order, each with the voices merged into it (flattened). */
export function groupVoices(voices: VoiceView[]): VoiceGroup[] {
  const groups = new Map<string, VoiceGroup>();
  const order: string[] = [];
  for (const v of voices) {
    if (mergeRoot(voices, v).voiceId === v.voiceId) {
      groups.set(v.voiceId, { voice: v, merged: [] });
      order.push(v.voiceId);
    }
  }
  for (const v of voices) {
    const root = mergeRoot(voices, v);
    if (root.voiceId === v.voiceId) continue;
    const target = groups.get(root.voiceId);
    if (target) {
      target.merged.push(v);
    } else {
      // A loop of merges (the backend refuses to create one): keep the voice visible.
      groups.set(v.voiceId, { voice: v, merged: [] });
      order.push(v.voiceId);
    }
  }
  return order.map((id) => groups.get(id)!);
}

export function participantName(
  participants: ParticipantView[],
  id: string | null | undefined,
): string | null {
  if (!id) return null;
  return participants.find((p) => p.id === id)?.name ?? null;
}

/** A cosine score (0..1) as a whole percentage, e.g. `83%`. Empty without a score. */
export function formatScore(score: number | null | undefined): string {
  return score === null || score === undefined ? "" : `${percent(score)}%`;
}

export type VoiceStateKind = "auto" | "suggested" | "unknown" | "named" | "skipped";

export interface VoiceState {
  kind: VoiceStateKind;
  /** The text of the state badge. */
  label: string;
  /** The participant the card names the voice after (or suggests), when there is one. */
  participantId: string | null;
}

/**
 * How a voice is presented. A decision of the user (`confirmed`) wins over what the matcher
 * found. An automatic match is never presented as certain: its label always carries the score.
 * `rejected` is a suggestion the user turned down on this screen; the voice counts as unknown.
 */
export function voiceState(
  voice: VoiceView,
  participants: ParticipantView[],
  rejected = false,
): VoiceState {
  if (voice.confirmed) {
    return voice.assignedParticipantId
      ? { kind: "named", label: "Named by you", participantId: voice.assignedParticipantId }
      : { kind: "skipped", label: "Not a person (skipped)", participantId: null };
  }
  const score = formatScore(voice.score);
  if (voice.decision === "auto" && voice.assignedParticipantId) {
    return {
      kind: "auto",
      label: score ? `Recognized automatically · ${score}` : "Recognized automatically",
      participantId: voice.assignedParticipantId,
    };
  }
  if (voice.decision === "suggested" && voice.suggestedParticipantId && !rejected) {
    const inCurrentProject = participants.some((p) => p.id === voice.suggestedParticipantId);
    const name =
      voice.suggestedParticipantName ??
      participantName(participants, voice.suggestedParticipantId) ??
      "Unknown name";
    const fromProjects = inCurrentProject ? [] : (voice.suggestedFromProjects ?? []);
    const label =
      fromProjects.length > 0
        ? `Recognised from ${fromProjects.join(", ")}: ${name}?${score ? ` ${score}` : ""}`
        : `Suggested: ${name}?${score ? ` ${score}` : ""}`;
    return {
      kind: "suggested",
      label,
      participantId: voice.suggestedParticipantId,
    };
  }
  return { kind: "unknown", label: "Unknown", participantId: null };
}

/** True while an automatic match waits for the user's confirmation. */
export function hasUnconfirmedAuto(voices: VoiceView[]): boolean {
  return voices.some((v) => !v.mergedInto && v.decision === "auto" && !v.confirmed);
}

export interface NamingProgress {
  named: number;
  total: number;
  text: string;
}

/** "3 of 4 voices named": merged voices follow their target and are not counted. */
export function namingProgress(voices: VoiceView[]): NamingProgress {
  const top = groupVoices(voices).map((g) => g.voice);
  const named = top.filter((v) => v.confirmed).length;
  const noun = top.length === 1 ? "voice" : "voices";
  return { named, total: top.length, text: `${named} of ${top.length} ${noun} named` };
}

type Thresholds = Pick<
  ProcessingSettings,
  "voiceAccept" | "voiceSuggest" | "voiceMargin" | "clipMaxDays"
>;

/** The rules the backend enforces: 0 <= suggest <= accept <= 1, margin >= 0, clip days >= 1. */
export function validateVoiceSettings(s: Thresholds): string | null {
  const unit = (n: number) => Number.isFinite(n) && n >= 0 && n <= 1;
  if (!unit(s.voiceSuggest) || !unit(s.voiceAccept))
    return "The match thresholds must be between 0 and 1.";
  if (s.voiceSuggest > s.voiceAccept)
    return "The suggestion threshold cannot be higher than the automatic match threshold.";
  if (!Number.isFinite(s.voiceMargin) || s.voiceMargin < 0) return "The margin must be 0 or more.";
  if (!Number.isInteger(s.clipMaxDays) || s.clipMaxDays < 1)
    return "Keep sample clips for a whole number of days, at least 1.";
  return null;
}
