import { useCallback, useState } from "react";
import { useBackend } from "./backend-context";
import { errorMessage } from "./hooks";

const MAX_TITLE_CHARS = 60;

/**
 * The file name offered in the Save as dialog: the title as a lowercase slug, then the date
 * (`YYYY-MM-DD`, left out when empty), then `.md`. Without a usable title the name starts with `meeting-note`.
 */
export function noteFileName(title: string | null | undefined, date: string): string {
  const slug = (title ?? "")
    .normalize("NFKC")
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, MAX_TITLE_CHARS)
    .replace(/-+$/, "");
  return `${[slug === "" ? "meeting-note" : slug, date].filter((p) => p !== "").join("-")}.md`;
}

/** The UTC date of a timestamp as `YYYY-MM-DD`. */
export function isoDateUtc(unixMs: number): string {
  return new Date(unixMs).toISOString().slice(0, 10);
}

export interface SaveNoteState {
  saving: boolean;
  /** The path the note was written to, until the next attempt. */
  savedPath: string | null;
  error: string | null;
  /** Ask for a file with the Save as dialog and write the note there; a cancel changes nothing. */
  save: (defaultName: string) => Promise<void>;
}

/** The "Download note (.md)" flow of one meeting: Save as dialog, then the note is written. */
export function useSaveNote(meetingId: string): SaveNoteState {
  const backend = useBackend();
  const [saving, setSaving] = useState(false);
  const [savedPath, setSavedPath] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const save = useCallback(
    async (defaultName: string) => {
      setError(null);
      setSavedPath(null);
      setSaving(true);
      try {
        const dest = await backend.saveFileAs({
          defaultName,
          extension: "md",
          title: "Save meeting note",
        });
        if (dest === null) return;
        setSavedPath(await backend.saveMeetingNoteAs(meetingId, dest));
      } catch (e) {
        setError(errorMessage(e));
      } finally {
        setSaving(false);
      }
    },
    [backend, meetingId],
  );

  return { saving, savedPath, error, save };
}
