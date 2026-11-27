import { useState } from "react";
import { useBackend } from "../backend-context";
import { errorMessage } from "../hooks";

export interface DeleteProjectSectionProps {
  cycleId: string;
  projectName: string;
  onDeleted: () => void;
}

/**
 * Danger-zone section for deleting a project from the app database and settings,
 * with explicit name confirmation and options for meetings and the local folder.
 */
export function DeleteProjectSection({
  cycleId,
  projectName,
  onDeleted,
}: DeleteProjectSectionProps) {
  const backend = useBackend();
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [typedName, setTypedName] = useState("");
  const [deleteMeetings, setDeleteMeetings] = useState(false);
  const [deleteFolder, setDeleteFolder] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const expectedName = projectName.trim() || cycleId;
  const confirmed = typedName.trim() === expectedName;

  const resetForm = () => {
    setTypedName("");
    setDeleteMeetings(false);
    setDeleteFolder(false);
    setError(null);
  };

  const handleDelete = async () => {
    if (!confirmed || busy) return;
    setError(null);
    setBusy(true);
    try {
      await backend.deleteProject(cycleId, deleteMeetings, deleteFolder);
      onDeleted();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <fieldset className="danger-zone">
      <legend>Danger zone</legend>
      {!confirmOpen ? (
        <div className="danger-row">
          <p className="section-hint">
            Remove this project from Blah Blah Bot. GitHub repositories are never deleted.
          </p>
          <button
            type="button"
            className="btn btn-small btn-stop"
            onClick={() => {
              resetForm();
              setConfirmOpen(true);
            }}
          >
            Delete project
          </button>
        </div>
      ) : (
        <div className="form" aria-label="Confirm project deletion">
          <p className="section-hint">
            This removes <strong>{expectedName}</strong> from the app database and project list. Any
            GitHub repository stays untouched.
          </p>
          <label className="check">
            <input
              type="checkbox"
              checked={deleteMeetings}
              onChange={(e) => setDeleteMeetings(e.target.checked)}
            />
            Also delete this project&apos;s meetings (if unchecked, meetings stay as standalone)
          </label>
          <label className="check">
            <input
              type="checkbox"
              checked={deleteFolder}
              onChange={(e) => setDeleteFolder(e.target.checked)}
            />
            Also move the local project folder to the Recycle Bin
          </label>
          <label>
            Type <strong>{expectedName}</strong> to confirm
            <input
              aria-label="Confirm project name"
              value={typedName}
              placeholder={expectedName}
              autoComplete="off"
              onChange={(e) => setTypedName(e.target.value)}
            />
          </label>
          {error && (
            <p className="error" role="alert">
              {error}
            </p>
          )}
          <div className="actions">
            <button
              type="button"
              className="btn btn-small btn-ghost"
              disabled={busy}
              onClick={() => {
                resetForm();
                setConfirmOpen(false);
              }}
            >
              Cancel
            </button>
            <button
              type="button"
              className="btn btn-small btn-stop"
              disabled={!confirmed || busy}
              onClick={() => void handleDelete()}
            >
              {busy ? "Deleting…" : "Confirm delete project"}
            </button>
          </div>
        </div>
      )}
    </fieldset>
  );
}
