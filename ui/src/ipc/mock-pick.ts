import type { Backend } from "./backend";
import { fail } from "./mock-util";

/**
 * What the mock dialogs answer. A string is the chosen path, `null` is a cancelled dialog, an
 * absent key is the default fake path. Tests preset it with `MockOptions.pick`; a Playwright
 * test sets `window.__bbbPick` before it clicks Browse (the hook wins over the options).
 */
export interface MockPickResults {
  file?: string | null;
  folder?: string | null;
  save?: string | null;
}

declare global {
  interface Window {
    __bbbPick?: MockPickResults;
  }
}

/** Deterministic fake paths used when nothing was preset. */
export const MOCK_PICK_DEFAULTS = {
  media: "C:\\Recordings\\mock-meeting.mp4",
  audio: "C:\\Audio\\mock-sample.wav",
  folder: "C:\\Mock\\Folder",
  saveDir: "Documents\\BlahBlahBot\\notes",
} as const;

type PickMethods = Pick<Backend, "pickFile" | "pickFolder" | "saveFileAs" | "saveMeetingNoteAs">;

/** The file dialogs and the note writer of the mock backend. */
export function createMockPicker(preset: MockPickResults | undefined): PickMethods {
  const answer = (key: keyof MockPickResults): string | null | undefined => {
    const hook = typeof window === "undefined" ? undefined : window.__bbbPick;
    if (hook && key in hook) return hook[key];
    return preset && key in preset ? preset[key] : undefined;
  };
  return {
    pickFile: async (req) => {
      const given = answer("file");
      if (given !== undefined) return given;
      return req.kind === "audio" ? MOCK_PICK_DEFAULTS.audio : MOCK_PICK_DEFAULTS.media;
    },
    pickFolder: async () => {
      const given = answer("folder");
      return given === undefined ? MOCK_PICK_DEFAULTS.folder : given;
    },
    saveFileAs: async (req) => {
      const given = answer("save");
      return given === undefined ? `${MOCK_PICK_DEFAULTS.saveDir}\\${req.defaultName}` : given;
    },
    saveMeetingNoteAs: async (_meetingId, destPath) => {
      const dest = destPath.trim();
      if (!/\.md$/i.test(dest) || /[\\/]\.md$/i.test(dest)) {
        fail("invalid", "choose a full path to a Markdown file that ends in .md");
      }
      return dest;
    },
  };
}
