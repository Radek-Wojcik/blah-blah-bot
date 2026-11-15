import type {
  ClipView,
  JobView,
  ModelStatus,
  ParticipantView,
  ProcessingSettings,
  StageName,
  StageView,
  TranscriptData,
  TranscriptSegment,
  VoiceView,
} from "./types";
import { stagesProgress } from "../processing";

/** Stage names in execution order. */
export const STAGE_ORDER: StageName[] = [
  "asr",
  "diarize",
  "merge",
  "speaker_id",
  "clips",
  "audio_cleanup",
  "vision",
  "extract",
  "reduce",
];

/** Message shown next to the running stage in the mock. */
export const STAGE_MESSAGES: Record<StageName, string> = {
  asr: "Recognizing speech",
  diarize: "Separating speakers",
  merge: "Merging tracks",
  speaker_id: "Matching voices against the profiles",
  clips: "Cutting sample clips",
  audio_cleanup: "Applying the audio retention rule",
  vision: "Describing screenshots",
  extract: "Finding tasks and decisions",
  reduce: "Writing the draft report",
};

export function stageViews(
  statuses: Partial<Record<StageName, Partial<StageView>>> = {},
): StageView[] {
  return STAGE_ORDER.map((stage) => ({
    stage,
    status: "pending",
    attempts: 0,
    error: null,
    fraction: null,
    message: null,
    ...statuses[stage],
  }));
}

export function job(
  meetingId: string,
  title: string,
  state: JobView["state"],
  stages: StageView[],
  waitingReason: string | null = null,
): JobView {
  return { meetingId, title, state, progress: stagesProgress(stages), waitingReason, stages };
}

const done = (): Partial<StageView> => ({ status: "done", attempts: 1 });

/** Fictional queue: one finished, one running, one waiting for the GPU, one failed. */
export function demoJobs(): JobView[] {
  return [
    job(
      "m4",
      "Thruster review",
      "running",
      stageViews({
        asr: done(),
        diarize: {
          status: "running",
          attempts: 1,
          fraction: 0.4,
          message: "Separating speakers",
        },
      }),
    ),
    job(
      "m3",
      "Budget check-in",
      "waiting",
      stageViews(),
      "GPU is busy (62% utilization, limit 40%)",
    ),
    job(
      "m2",
      "Ground station planning",
      "failed",
      stageViews({
        asr: done(),
        diarize: {
          status: "failed",
          attempts: 3,
          error:
            "Speaker model could not be loaded: access to the model was denied (HTTP 401). Check the Hugging Face token and the accepted model terms.",
        },
      }),
    ),
    job(
      "m5",
      "ORBIT weekly sync",
      "done",
      stageViews({
        asr: done(),
        diarize: done(),
        merge: done(),
        speaker_id: done(),
        clips: done(),
        audio_cleanup: done(),
        vision: done(),
        extract: done(),
        reduce: done(),
      }),
    ),
  ];
}

const MIN = 60_000;

function seg(
  id: number,
  speaker: string,
  startMs: number,
  endMs: number,
  text: string,
  extra: Partial<TranscriptSegment> = {},
): TranscriptSegment {
  return {
    id,
    track: speaker === "user" ? "mic-default" : "app-discord",
    speaker,
    startMs,
    endMs,
    text,
    originalText: null,
    excluded: false,
    ...extra,
  };
}

/** Fictional transcript of the demo meeting: three remote voices and the user. */
export function demoTranscript(meetingId: string): TranscriptData {
  return {
    meetingId,
    language: "en",
    speakers: [
      { id: "user", label: "You", kind: "user", track: "mic-default" },
      { id: "alex", label: "Alex", kind: "voice", track: "app-discord" },
      { id: "kasia", label: "Kasia", kind: "voice", track: "app-discord" },
      { id: "jan", label: "Jan", kind: "voice", track: "app-discord" },
    ],
    segments: [
      seg(1, "user", 0, 5_800, "Okay, let's start with the thruster test results."),
      seg(
        2,
        "alex",
        6_000,
        18_500,
        "The last firing ran for ninety seconds and the chamber pressure stayed flat.",
      ),
      seg(
        3,
        "kasia",
        19_000,
        26_000,
        "Good. Did the new valve change anything on the thrust curve?",
      ),
      seg(
        4,
        "alex",
        27_000,
        39_500,
        "Only a small ripple at ignition, nothing we cannot filter out.",
        { originalText: "Only a small ripple at ignition, nothing we cannot fill the out." },
      ),
      seg(5, "jan", 41_000, 52_000, "I will update the ground station schedule for Thursday."),
      seg(6, "user", 55_000, 62_000, "Please share the budget sheet before the review."),
      seg(
        7,
        "kasia",
        2 * MIN + 5_000,
        2 * MIN + 17_000,
        "The budget is on track, we are two percent under.",
      ),
      seg(
        8,
        "alex",
        2 * MIN + 20_000,
        2 * MIN + 31_000,
        "That leaves room for a second test stand.",
      ),
      seg(
        9,
        "jan",
        2 * MIN + 43_000,
        2 * MIN + 55_000,
        "The antenna array arrives next week, the mount is already on site.",
      ),
      seg(
        10,
        "user",
        3 * MIN,
        3 * MIN + 9_000,
        "Great, then we keep the launch window as planned.",
      ),
      seg(
        11,
        "kasia",
        3 * MIN + 16_000,
        3 * MIN + 24_000,
        "Sorry, someone is at the door, please ignore this part.",
        { excluded: true },
      ),
      seg(
        12,
        "alex",
        3 * MIN + 30_000,
        3 * MIN + 41_000,
        "I will send the summary to everyone tomorrow.",
      ),
    ],
    screenshots: [
      {
        file: "img/0001.png",
        offsetMs: 40_000,
        description:
          "Slide titled Thruster test 7 with a line chart of chamber pressure over ninety seconds. The line is flat after a short spike at ignition.",
      },
      {
        file: "img/0002.png",
        offsetMs: 2 * MIN + 30_000,
        description:
          "Spreadsheet with the quarterly budget. The total row shows a two percent underspend; the cell for the second test stand is highlighted.",
      },
    ],
  };
}

export function demoProcessingSettings(): ProcessingSettings {
  return {
    processAfterRecording: "ask",
    audioRetention: { mode: "delete", days: 30 },
    gpuIdleSeconds: 30,
    gpuMaxUtilPercent: 40,
    uvPath: "uv",
    workerDir: "C:\\Program Files\\BlahBlahBot\\worker",
    llamaServerPath: "C:\\Tools\\llama\\llama-server.exe",
    modelsDir: "Documents\\BlahBlahBot\\models",
    hfTokenSet: false,
    voiceAccept: 0.75,
    voiceSuggest: 0.6,
    voiceMargin: 0.1,
    clipMaxDays: 30,
    llm: {
      defaultKind: "managed",
      managed: { modelPath: "D:\\Models\\gemma-4-12b-q4_k_m.gguf", ctx: 16384 },
      external: { baseUrl: "http://127.0.0.1:11434/v1", model: "gemma4:12b" },
      cloud: { baseUrl: "https://api.llm-vendor.example/v1", model: "large-model-1" },
      externalKeySet: false,
      cloudKeySet: true,
    },
  };
}

export function demoModels(): ModelStatus[] {
  return [
    {
      key: "whisper-turbo",
      label: "Whisper turbo (speech recognition)",
      cached: true,
      gated: false,
      sizeMb: 1600,
    },
    {
      key: "whisper-large-v3",
      label: "Whisper large v3 (higher accuracy)",
      cached: false,
      gated: false,
      sizeMb: 3100,
    },
    {
      key: "diarization",
      label: "Speaker separation (pyannote)",
      cached: false,
      gated: true,
      sizeMb: 1200,
    },
    {
      key: "vision",
      label: "Screenshot descriptions (vision model)",
      cached: false,
      gated: false,
      sizeMb: 4800,
    },
  ];
}

/** Fictional people known to the voice profiles. */
export function demoParticipants(): ParticipantView[] {
  return [
    { id: "p-alex", name: "Alex Demo", sampleCount: 4, speechMs: 18 * MIN + 20_000 },
    { id: "p-jan", name: "Jan Demo", sampleCount: 2, speechMs: 7 * MIN + 45_000 },
    { id: "p-kasia", name: "Kasia Demo", sampleCount: 3, speechMs: 11 * MIN + 5_000 },
  ];
}

function clip(idx: number, startMs: number, endMs: number, text: string): ClipView {
  return { idx, startMs, endMs, text };
}

/**
 * The voices detected in the demo meeting: one recognized automatically, one suggested, two
 * unknown (one with three clips, one with two).
 */
export function demoVoices(
  personId: (key: string) => string | null = (k) => `p-${k}`,
): VoiceView[] {
  const base = { track: "app-discord", confirmed: false, mergedInto: null };
  return [
    {
      ...base,
      voiceId: "v0",
      label: "SPEAKER_00",
      speechMs: 14 * MIN + 10_000,
      decision: "auto",
      score: 0.83,
      suggestedParticipantId: null,
      assignedParticipantId: personId("alex"),
      clips: [
        clip(
          0,
          6_000,
          18_500,
          "The last firing ran for ninety seconds and the chamber pressure stayed flat.",
        ),
        clip(1, 2 * MIN + 20_000, 2 * MIN + 31_000, "That leaves room for a second test stand."),
      ],
    },
    {
      ...base,
      voiceId: "v1",
      label: "SPEAKER_01",
      speechMs: 9 * MIN + 30_000,
      decision: "suggested",
      score: 0.61,
      suggestedParticipantId: personId("kasia"),
      assignedParticipantId: null,
      clips: [
        clip(0, 19_000, 26_000, "Good. Did the new valve change anything on the thrust curve?"),
        clip(
          1,
          2 * MIN + 5_000,
          2 * MIN + 17_000,
          "The budget is on track, we are two percent under.",
        ),
      ],
    },
    {
      ...base,
      voiceId: "v2",
      label: "SPEAKER_02",
      speechMs: 6 * MIN + 5_000,
      decision: "unknown",
      score: null,
      suggestedParticipantId: null,
      assignedParticipantId: null,
      clips: [
        clip(0, 41_000, 52_000, "I will update the ground station schedule for Thursday."),
        clip(
          1,
          2 * MIN + 43_000,
          2 * MIN + 55_000,
          "The antenna array arrives next week, the mount is already on site.",
        ),
        clip(2, 4 * MIN + 2_000, 4 * MIN + 9_000, "Thursday works for the dry run as well."),
      ],
    },
    {
      ...base,
      voiceId: "v3",
      label: "SPEAKER_03",
      speechMs: 2 * MIN + 40_000,
      decision: "unknown",
      score: null,
      suggestedParticipantId: null,
      assignedParticipantId: null,
      clips: [
        clip(0, 5 * MIN + 12_000, 5 * MIN + 20_000, "Can everyone see my screen now?"),
        clip(
          1,
          5 * MIN + 48_000,
          5 * MIN + 58_000,
          "I can take the action item for the test stand.",
        ),
      ],
    },
  ];
}

/**
 * A short mono 16-bit WAV with a sine tone, so that the demo clips are audible in a browser.
 * Each voice gets its own pitch.
 */
export function toneWav(freqHz: number, seconds: number, sampleRate = 8000): Blob {
  const samples = Math.round(sampleRate * seconds);
  const buf = new ArrayBuffer(44 + samples * 2);
  const v = new DataView(buf);
  const text = (at: number, s: string) =>
    [...s].forEach((c, i) => v.setUint8(at + i, c.charCodeAt(0)));
  text(0, "RIFF");
  v.setUint32(4, 36 + samples * 2, true);
  text(8, "WAVE");
  text(12, "fmt ");
  v.setUint32(16, 16, true);
  v.setUint16(20, 1, true); // PCM
  v.setUint16(22, 1, true); // mono
  v.setUint32(24, sampleRate, true);
  v.setUint32(28, sampleRate * 2, true);
  v.setUint16(32, 2, true);
  v.setUint16(34, 16, true);
  text(36, "data");
  v.setUint32(40, samples * 2, true);
  const fade = Math.round(sampleRate * 0.03);
  for (let i = 0; i < samples; i += 1) {
    // Short fades at both ends avoid clicks.
    const gain = Math.min(1, i / fade, (samples - i) / fade);
    const x = Math.sin((2 * Math.PI * freqHz * i) / sampleRate) * 0.25 * gain;
    v.setInt16(44 + i * 2, Math.round(x * 32767), true);
  }
  return new Blob([buf], { type: "audio/wav" });
}

function bar(x: number, h: number, color: string): string {
  return `<rect x="${x}" y="${220 - h}" width="40" height="${h}" fill="${color}" opacity=".75"/>`;
}

/** A deterministic stand-in picture for a meeting screenshot (an SVG data URL). */
export function demoImage(file: string): string {
  const first = file.includes("0001");
  const bars = first
    ? [60, 150, 130, 128, 130, 129, 131, 130, 128, 130]
        .map((h, i) => bar(30 + i * 52, h, "#00f0ff"))
        .join("")
    : [90, 110, 140, 100, 160, 120, 150, 70].map((h, i) => bar(30 + i * 62, h, "#ff2a6d")).join("");
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="560" height="315" viewBox="0 0 560 315"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#101a33"/><stop offset="1" stop-color="#3a1030"/></linearGradient></defs><rect width="560" height="315" fill="url(#g)"/><text x="30" y="40" fill="#dfe9ff" font-family="monospace" font-size="20">${first ? "Thruster test 7" : "Quarterly budget"}</text>${bars}<rect x="20" y="224" width="520" height="2" fill="#6f7ea8"/></svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}
