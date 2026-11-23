import type { DetectionSettings, Sensitivity } from "./ipc/types";

/** What each sensitivity means, in the words of the settings screen. */
export const SENSITIVITIES: { value: Sensitivity; label: string; text: string }[] = [
  {
    value: "low",
    label: "Low",
    text: "Asks after 20 seconds of microphone use while the application also plays sound. Fewest prompts, a longer wait.",
  },
  {
    value: "balanced",
    label: "Balanced",
    text: "Asks after 8 seconds of microphone use when the application has a playback session. The default.",
  },
  {
    value: "high",
    label: "High",
    text: "Asks after 3 seconds of microphone use. Earliest prompt; a short microphone check may also ask.",
  },
];

/** Executable name as the settings keep it: lowercase, no folder, no `.exe`. */
export function normalizeApp(text: string): string {
  const name = text.trim().toLowerCase();
  return name.endsWith(".exe") ? name.slice(0, -4) : name;
}

/** Why `text` cannot be added to a list of applications, or null. */
export function appProblem(text: string, list: string[]): string | null {
  const name = normalizeApp(text);
  if (name === "") return "Enter the name of the program, for example zoom.";
  if (/[\\/:]/.test(name)) return "Enter the program name only, without a folder.";
  if (name.length > 64) return "The name is too long.";
  if (list.includes(name)) return `${name} is in the list already.`;
  return null;
}

/** The list with `text` added, or the list unchanged when it cannot be added. */
export function withApp(list: string[], text: string): string[] {
  return appProblem(text, list) === null ? [...list, normalizeApp(text)] : list;
}

/** A readable name for the list: `ms-teams` is Microsoft Teams. */
export function appLabel(app: string): string {
  switch (app) {
    case "discord":
      return "Discord";
    case "discordptb":
      return "Discord PTB";
    case "ms-teams":
    case "teams":
      return "Microsoft Teams";
    case "zoom":
      return "Zoom";
    case "slack":
      return "Slack";
    case "meet":
      return "Google Meet";
    default:
      return app;
  }
}

/** One line about the state of detection for the settings screen. */
export function detectionSummary(s: DetectionSettings): string {
  if (!s.enabled) return "Off: no prompt is raised.";
  const watched = s.knownApps.filter((a) => !s.optedOut.includes(a)).length + (s.meet ? 1 : 0);
  return watched === 0
    ? "On, but no application is watched."
    : `On, watching ${watched} ${watched === 1 ? "application" : "applications"}.`;
}
