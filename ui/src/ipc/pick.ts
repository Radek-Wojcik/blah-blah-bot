// What the native file dialogs are asked for, and how their answers are read. Shared by the real
// backend (Tauri dialog plugin), the mock backend and the tests.

export const AUDIO_EXTENSIONS = ["mp3", "wav", "m4a", "ogg", "flac", "aac"] as const;
export const VIDEO_EXTENSIONS = ["mkv", "mp4", "webm", "mov", "avi"] as const;

/** `media`: a meeting recording (audio or video). `audio`: a voice sample. */
export type PickFileKind = "media" | "audio";

export interface PickFileRequest {
  kind: PickFileKind;
  title?: string;
}

export interface PickFolderRequest {
  title?: string;
}

export interface SaveFileRequest {
  /** The file name offered in the dialog, with the extension. */
  defaultName: string;
  /** The only extension the dialog accepts, without a dot (`md`). */
  extension: string;
  title?: string;
}

export interface DialogFilter {
  name: string;
  extensions: string[];
}

/** The file type filters of the open dialog; the first one is shown first. */
export function filtersFor(kind: PickFileKind): DialogFilter[] {
  const audio = { name: "Audio", extensions: [...AUDIO_EXTENSIONS] };
  const video = { name: "Video", extensions: [...VIDEO_EXTENSIONS] };
  if (kind === "audio") {
    return [audio, { name: "Video (its audio track is used)", extensions: ["mp4"] }];
  }
  return [
    { name: "Audio and video", extensions: [...AUDIO_EXTENSIONS, ...VIDEO_EXTENSIONS] },
    audio,
    video,
  ];
}

/**
 * The path a dialog answered with: `null` when the user cancelled. Anything else than a
 * non-empty path is an error, so a missing path is never replaced by a guess such as a bare
 * file name.
 */
export function pickedPath(result: unknown): string | null {
  if (result === null || result === undefined) return null;
  const value = Array.isArray(result) && result.length === 1 ? result[0] : result;
  if (typeof value === "string" && value.trim() !== "") return value;
  throw new Error("The file dialog did not return a path. Type the path instead.");
}
