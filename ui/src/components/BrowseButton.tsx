import { useState } from "react";
import { useBackend } from "../backend-context";
import { errorMessage } from "../hooks";
import type { PickFileKind } from "../ipc/pick";

export type BrowseTarget = { folder: true } | { file: PickFileKind };

/**
 * A "Browse…" button that opens the native file or folder dialog and hands the chosen full path
 * to `onPicked`. A cancelled dialog changes nothing; a dialog that fails shows its message next
 * to the button, so a path is never guessed.
 */
export function BrowseButton({
  target,
  label,
  title,
  onPicked,
  disabled = false,
}: {
  target: BrowseTarget;
  /** Accessible name, for example "Browse for the audio or video file". */
  label: string;
  /** Title of the dialog window. */
  title?: string;
  onPicked: (path: string) => void;
  disabled?: boolean;
}) {
  const backend = useBackend();
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const browse = async () => {
    setError(null);
    setBusy(true);
    try {
      const path =
        "folder" in target
          ? await backend.pickFolder(title === undefined ? undefined : { title })
          : await backend.pickFile({
              kind: target.file,
              ...(title === undefined ? {} : { title }),
            });
      if (path !== null) onPicked(path);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <button
        type="button"
        className="btn btn-small btn-ghost"
        aria-label={label}
        disabled={disabled || busy}
        onClick={() => void browse()}
      >
        Browse…
      </button>
      {error && (
        <small className="error" role="alert">
          {error}
        </small>
      )}
    </>
  );
}
