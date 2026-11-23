import type { Unsubscribe } from "./backend";
import { fail } from "./mock-util";
import { appProblem, normalizeApp } from "../detection";
import type { AnswerOutcome, DetectionAnswer, DetectionPrompt, DetectionSettings } from "./types";

export interface MockDetectionOptions {
  /** Raise a prompt for a Discord call this many milliseconds after the backend was created. */
  promptAfterMs?: number;
  /** Start a recording of these sources with the last report; rejects like `startRecording`. */
  record: (sourceIds: string[]) => Promise<void>;
  /** A recording is running. */
  recording: () => boolean;
}

/** The demo call the mock detects. */
const DISCORD: DetectionPrompt = {
  key: "discord",
  app: "discord",
  display: "Discord",
  source: "audio",
};

/** Meeting detection, simulated: settings and one prompt for a fictional Discord call. */
export function createMockDetection(opts: MockDetectionOptions) {
  let settings: DetectionSettings = {
    enabled: true,
    sensitivity: "balanced",
    knownApps: ["discord", "discordptb", "ms-teams", "teams", "zoom", "slack"],
    optedOut: [],
    meet: true,
  };
  let prompt: DetectionPrompt | null = null;
  const prompts = new Set<{ cb: (p: DetectionPrompt) => void }>();
  const cleared = new Set<{ cb: (key: string) => void }>();

  const clear = () => {
    if (!prompt) return;
    const key = prompt.key;
    prompt = null;
    cleared.forEach((l) => l.cb(key));
  };
  const raise = () => {
    const watched =
      settings.enabled &&
      settings.knownApps.includes(DISCORD.app) &&
      !settings.optedOut.includes(DISCORD.app);
    if (!watched || prompt || opts.recording()) return;
    prompt = { ...DISCORD };
    prompts.forEach((l) => l.cb({ ...DISCORD }));
  };
  if (opts.promptAfterMs !== undefined) setTimeout(raise, opts.promptAfterMs);

  return {
    methods: {
      getDetectionSettings: async () => structuredClone(settings),
      saveDetectionSettings: async (next: DetectionSettings) => {
        const clean = (list: string[]) => {
          const out: string[] = [];
          for (const a of list) {
            if (appProblem(a, out) === null) out.push(normalizeApp(a));
            else if (normalizeApp(a) !== "" && !out.includes(normalizeApp(a)))
              fail("invalid", `\`${a}\` is not an executable name`);
          }
          return out;
        };
        if (!["low", "balanced", "high"].includes(next.sensitivity))
          fail("invalid", "unknown sensitivity");
        const optedOut = clean(next.optedOut).filter((a) => !(next.meet && a === "meet"));
        settings = {
          ...next,
          knownApps: clean(next.knownApps),
          optedOut,
        };
        if (
          prompt &&
          (!settings.enabled ||
            (prompt.source === "meet"
              ? !settings.meet || settings.optedOut.includes("meet")
              : settings.optedOut.includes(prompt.app) || !settings.knownApps.includes(prompt.app)))
        )
          clear();
        return structuredClone(settings);
      },
      detectionPrompt: async () => (prompt ? { ...prompt } : null),
      answerDetection: async (key: string, answer: DetectionAnswer): Promise<AnswerOutcome> => {
        if (!prompt || prompt.key !== key) return { started: false, needsReport: false };
        const app = prompt.app;
        let outcome: AnswerOutcome = { started: false, needsReport: false };
        if (answer === "never") {
          if (prompt.source === "meet" || app === "meet") {
            settings = {
              ...settings,
              meet: false,
              optedOut: settings.optedOut.filter((x) => x !== "meet"),
            };
          } else if (!settings.optedOut.includes(app)) {
            settings = { ...settings, optedOut: [...settings.optedOut, app] };
          }
        }
        if (answer === "record") {
          try {
            await opts.record(["mic:default", `app:${app}`]);
            outcome = { started: true, needsReport: false };
          } catch (e) {
            if ((e as { code?: string }).code !== "report_mode_required") throw e;
            outcome = { started: false, needsReport: true };
          }
        }
        clear();
        return outcome;
      },
      onDetectionPrompt: (cb: (p: DetectionPrompt) => void): Promise<Unsubscribe> => {
        const entry = { cb };
        prompts.add(entry);
        return Promise.resolve(() => void prompts.delete(entry));
      },
      onDetectionCleared: (cb: (key: string) => void): Promise<Unsubscribe> => {
        const entry = { cb };
        cleared.add(entry);
        return Promise.resolve(() => void cleared.delete(entry));
      },
    },
    /** The detection part of `Settings`, as the backend returns it. */
    settings: () => structuredClone(settings),
    /** Raise the demo prompt now (tests). */
    raise,
  };
}
