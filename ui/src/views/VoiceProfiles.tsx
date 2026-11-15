import { useCallback, useEffect, useState } from "react";
import { useBackend } from "../backend-context";
import { formatDuration } from "../format";
import { errorMessage } from "../hooks";
import type { ParticipantView } from "../ipc/types";

type Confirm =
  | { kind: "reset"; participant: ParticipantView }
  | { kind: "delete"; participant: ParticipantView }
  | { kind: "all" };

/** Participants and their voice profiles: rename, forget a voice, delete a person or all voice data. */
export function VoiceProfiles() {
  const backend = useBackend();
  const [people, setPeople] = useState<ParticipantView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<{ id: string; name: string } | null>(null);
  const [confirm, setConfirm] = useState<Confirm | null>(null);

  const load = useCallback(() => {
    backend
      .listParticipants()
      .then(setPeople)
      .catch((e) => setError(errorMessage(e)));
  }, [backend]);
  useEffect(load, [load]);

  /** Run a change, then reload the list. */
  const act = (p: Promise<unknown>) => {
    setError(null);
    return p.then(load).catch((e) => setError(errorMessage(e)));
  };

  useEffect(() => {
    if (confirm === null) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setConfirm(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [confirm]);

  const run = () => {
    if (confirm === null) return;
    const c = confirm;
    setConfirm(null);
    if (c.kind === "reset") void act(backend.resetProfile(c.participant.id));
    else if (c.kind === "delete") void act(backend.deleteParticipant(c.participant.id));
    else void act(backend.deleteVoiceData());
  };

  return (
    <>
      <h3 className="sub-head">Voice profiles</h3>
      <div className="panel quiet" style={{ maxWidth: 620 }}>
        <div className="panel-body">
          <p className="notice">
            A profile is a numeric fingerprint of a voice, learned from the samples you confirmed.
            It stays on this computer and is only used to recognize the person in new meetings.
          </p>
          {error && (
            <p className="error" role="alert">
              {error}
            </p>
          )}
          {people && people.length === 0 && (
            <p className="empty">
              No participants yet. They appear when you name a voice in a processed meeting.
            </p>
          )}
          <ul className="people" aria-label="Participants">
            {(people ?? []).map((p) => (
              <li key={p.id} className="person">
                {renaming?.id === p.id ? (
                  <form
                    className="person-rename"
                    onSubmit={(e) => {
                      e.preventDefault();
                      const name = renaming.name;
                      void act(backend.renameParticipant(p.id, name)).then(() =>
                        setRenaming((cur) => (cur?.id === p.id ? null : cur)),
                      );
                    }}
                  >
                    <input
                      className="voice-input"
                      aria-label={`New name for ${p.name}`}
                      maxLength={80}
                      autoFocus
                      value={renaming.name}
                      onChange={(e) => setRenaming({ id: p.id, name: e.target.value })}
                      onKeyDown={(e) => {
                        if (e.key === "Escape") {
                          e.stopPropagation();
                          setRenaming(null);
                        }
                      }}
                    />
                    <button
                      className="btn btn-small"
                      type="submit"
                      disabled={renaming.name.trim() === ""}
                    >
                      Save
                    </button>
                    <button
                      className="btn btn-small btn-ghost"
                      type="button"
                      onClick={() => setRenaming(null)}
                    >
                      Cancel
                    </button>
                  </form>
                ) : (
                  <>
                    <div className="person-main">
                      <span className="person-name">{p.name}</span>
                      <span className="person-meta">
                        {p.sampleCount === 0
                          ? "No voice learned yet"
                          : `${p.sampleCount} ${p.sampleCount === 1 ? "sample" : "samples"} · ${formatDuration(p.speechMs)} of speech`}
                      </span>
                    </div>
                    <div className="person-actions">
                      <button
                        className="btn btn-small btn-ghost"
                        aria-label={`Rename ${p.name}`}
                        onClick={() => setRenaming({ id: p.id, name: p.name })}
                      >
                        Rename
                      </button>
                      <button
                        className="btn btn-small btn-stop"
                        aria-label={`Reset the voice of ${p.name}`}
                        disabled={p.sampleCount === 0}
                        onClick={() => setConfirm({ kind: "reset", participant: p })}
                      >
                        Reset voice
                      </button>
                      <button
                        className="btn btn-small btn-record"
                        aria-label={`Delete ${p.name}`}
                        onClick={() => setConfirm({ kind: "delete", participant: p })}
                      >
                        Delete participant
                      </button>
                    </div>
                  </>
                )}
              </li>
            ))}
          </ul>
          <div className="actions">
            <button className="btn btn-record" onClick={() => setConfirm({ kind: "all" })}>
              Delete all voice data
            </button>
          </div>
        </div>
      </div>

      {confirm && (
        <div className="modal-backdrop">
          <div
            className="panel modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby="voice-confirm-title"
          >
            <div className="panel-body">
              <h3 id="voice-confirm-title" className="modal-title">
                {confirm.kind === "reset" && "Reset this voice?"}
                {confirm.kind === "delete" && "Delete this participant?"}
                {confirm.kind === "all" && "Delete all voice data?"}
              </h3>
              <p className="modal-text">
                {confirm.kind === "reset" &&
                  `The learned voice of ${confirm.participant.name} is forgotten, so ${confirm.participant.name} is no longer recognized automatically. The name is kept.`}
                {confirm.kind === "delete" &&
                  `${confirm.participant.name} and their voice profile are deleted. Voices in meetings that were named after them become unnamed. This cannot be undone.`}
                {confirm.kind === "all" &&
                  "This removes all voice profiles, all stored voice embeddings of your meetings and all sample clips. Participant names are kept, but nobody is recognized automatically until you name voices again. This cannot be undone."}
              </p>
              <div className="actions">
                <button className="btn btn-ghost" autoFocus onClick={() => setConfirm(null)}>
                  Cancel
                </button>
                <button className="btn btn-record" onClick={run}>
                  {confirm.kind === "reset" && "Reset voice"}
                  {confirm.kind === "delete" && "Delete participant"}
                  {confirm.kind === "all" && "Delete all voice data"}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
