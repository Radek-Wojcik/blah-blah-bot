import { useCallback, useEffect, useRef, useState } from "react";
import { useBackend } from "../backend-context";
import { HelpPopover } from "../components/HelpPopover";
import { errorMessage } from "../hooks";
import { bytesToBlob } from "../ipc/bytes";
import type { CycleListItem, GlobalVoiceProfile } from "../ipc/types";

type Confirm =
  | { kind: "reset"; profile: GlobalVoiceProfile }
  | { kind: "delete"; profile: GlobalVoiceProfile }
  | { kind: "all" };

function useSampleClipPlayer(onError: (msg: string) => void) {
  const backend = useBackend();
  const [playingId, setPlayingId] = useState<string | null>(null);
  const activeRef = useRef<{ audio: HTMLAudioElement; url: string } | null>(null);
  const tokenRef = useRef(0);

  const stop = useCallback(() => {
    tokenRef.current += 1;
    const current = activeRef.current;
    activeRef.current = null;
    if (current) {
      current.audio.pause();
      URL.revokeObjectURL(current.url);
    }
    setPlayingId(null);
  }, []);

  const toggle = useCallback(
    async (profileId: string) => {
      const wasPlaying = playingId === profileId;
      stop();
      if (wasPlaying) return;
      const mine = tokenRef.current;
      setPlayingId(profileId);
      try {
        const bytes = await backend.voiceProfileSampleClip(profileId);
        if (tokenRef.current !== mine) return;
        const blob = bytesToBlob(bytes, "audio/wav");
        const url = URL.createObjectURL(blob);
        if (tokenRef.current !== mine) {
          URL.revokeObjectURL(url);
          return;
        }
        const audio = new Audio(url);
        activeRef.current = { audio, url };
        audio.onended = () => {
          if (tokenRef.current === mine) stop();
        };
        audio.onerror = () => {
          if (tokenRef.current === mine) stop();
        };
        await Promise.resolve(audio.play());
      } catch (e) {
        if (tokenRef.current === mine) {
          stop();
          onError(`Sample clip could not be played: ${errorMessage(e)}`);
        }
      }
    },
    [backend, onError, playingId, stop],
  );

  useEffect(() => stop, [stop]);
  return { playingId, toggle, stop };
}

/**
 * Participants and their voice profiles across projects:
 * project badges, playable sample clip, manual 15-30 s enrollment, rename, reset, and delete.
 */
export function VoiceProfiles() {
  const backend = useBackend();
  const [profiles, setProfiles] = useState<GlobalVoiceProfile[] | null>(null);
  const [cycles, setCycles] = useState<CycleListItem[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<{ id: string; name: string } | null>(null);
  const [confirm, setConfirm] = useState<Confirm | null>(null);

  const [enrollName, setEnrollName] = useState("");
  const [enrollCycleId, setEnrollCycleId] = useState("");
  const [enrollPath, setEnrollPath] = useState("");
  const [enrolling, setEnrolling] = useState(false);
  const [enrollStatus, setEnrollStatus] = useState<string | null>(null);

  const player = useSampleClipPlayer(setError);

  const load = useCallback(() => {
    void Promise.all([backend.listAllVoiceProfiles(), backend.listCycles()])
      .then(([allProfiles, allCycles]) => {
        setProfiles(allProfiles);
        setCycles(allCycles);
      })
      .catch((e) => setError(errorMessage(e)));
  }, [backend]);

  useEffect(() => {
    load();
    let off: (() => void) | undefined;
    let cancelled = false;
    void backend.onCyclesChanged(load).then((u) => {
      if (cancelled) u();
      else off = u;
    });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [backend, load]);

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

  const runConfirm = () => {
    if (confirm === null) return;
    const c = confirm;
    setConfirm(null);
    player.stop();
    if (c.kind === "reset") void act(backend.resetProfile(c.profile.id));
    else if (c.kind === "delete") void act(backend.deleteParticipant(c.profile.id));
    else void act(backend.deleteVoiceData());
  };

  const handleEnroll = async (e: React.FormEvent) => {
    e.preventDefault();
    if (enrollName.trim() === "" || enrollPath.trim() === "") return;
    setError(null);
    setEnrollStatus(null);
    setEnrolling(true);
    try {
      const created = await backend.enrollVoiceProfile(
        enrollName.trim(),
        enrollCycleId === "" ? null : enrollCycleId,
        enrollPath.trim(),
      );
      setProfiles((cur) => {
        const base = (cur ?? []).filter((item) => item.id !== created.id);
        return [...base, created].sort((a, b) => a.name.localeCompare(b.name));
      });
      setEnrollName("");
      setEnrollPath("");
      setEnrollStatus(`Enrolled voice profile for ${created.name}.`);
      load();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setEnrolling(false);
    }
  };

  return (
    <div className="settings-pane">
      <div className="panel quiet">
        <div className="panel-body">
          <div className="field-label-row" style={{ marginBottom: 8 }}>
            <h3 className="sub-head" style={{ margin: 0 }}>
              Add voice profile manually
            </h3>
            <HelpPopover
              title="Manual Voice Enrollment"
              recommended="15–30 seconds of clean solo speech (.wav, .mp3, or .mp4)"
            >
              <p>
                A clean 15–30 s solo speech clip lets the app recognize this speaker automatically
                across future meetings and projects.
              </p>
              <p>
                Choose <strong>All projects (Global)</strong> or link the profile to a specific
                project.
              </p>
            </HelpPopover>
          </div>

          <form
            className="form"
            aria-label="Add voice profile manually"
            onSubmit={(e) => void handleEnroll(e)}
          >
            <div className="settings-grid-2col">
              <label>
                Person name
                <input
                  aria-label="Person name"
                  value={enrollName}
                  maxLength={80}
                  placeholder="Alex Kowalski"
                  onChange={(e) => setEnrollName(e.target.value)}
                />
              </label>
              <label>
                Project
                <select
                  aria-label="Project"
                  value={enrollCycleId}
                  onChange={(e) => setEnrollCycleId(e.target.value)}
                >
                  <option value="">All projects (Global)</option>
                  {cycles.map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.name} ({c.id})
                    </option>
                  ))}
                </select>
              </label>
            </div>
            <label>
              15–30 s audio/video sample path
              <input
                aria-label="15–30 s audio/video sample path"
                value={enrollPath}
                placeholder="C:\Audio\alex-sample-20s.wav"
                onChange={(e) => setEnrollPath(e.target.value)}
              />
              <small>Provide a 15–30 second recording where only this person speaks clearly.</small>
            </label>
            {enrollStatus && (
              <p className="secret-result" role="status">
                {enrollStatus}
              </p>
            )}
            <div className="actions">
              <button
                type="submit"
                className="btn"
                disabled={enrolling || enrollName.trim() === "" || enrollPath.trim() === ""}
              >
                {enrolling ? "Enrolling…" : "Enroll voice profile"}
              </button>
            </div>
          </form>
        </div>
      </div>

      <div className="panel quiet">
        <div className="panel-body">
          <h3 className="sub-head" style={{ marginTop: 0 }}>
            Voice profiles
          </h3>
          <p className="notice">
            A profile is a numeric fingerprint of a voice, learned from confirmed meeting speakers
            or manual samples. It stays on this computer and recognizes the person across meetings.
          </p>
          {error && (
            <p className="error" role="alert">
              {error}
            </p>
          )}
          {profiles && profiles.length === 0 && (
            <p className="empty">
              No voice profiles yet. Enroll a 15–30 s sample above or confirm voices in a processed
              meeting.
            </p>
          )}
          <ul className="people" aria-label="Voice profiles">
            {(profiles ?? []).map((p) => {
              const isPlaying = player.playingId === p.id;
              return (
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
                        <span className="person-name">
                          {p.name}{" "}
                          {p.projects.length > 0 ? (
                            p.projects.map((proj) => (
                              <span
                                key={proj}
                                className="badge badge-recovered"
                                style={{ marginLeft: 4 }}
                              >
                                {proj}
                              </span>
                            ))
                          ) : (
                            <span className="badge" style={{ marginLeft: 4 }}>
                              Global
                            </span>
                          )}
                        </span>
                        <span className="person-meta">
                          {p.sampleCount === 0
                            ? "No voice learned yet"
                            : `${p.sampleCount} ${p.sampleCount === 1 ? "sample" : "samples"}`}
                        </span>
                      </div>
                      <div className="person-actions">
                        {p.hasSampleClip && (
                          <button
                            type="button"
                            className={isPlaying ? "btn btn-small btn-stop" : "btn btn-small"}
                            aria-label={
                              isPlaying ? `Stop sample for ${p.name}` : `Play sample for ${p.name}`
                            }
                            onClick={() => void player.toggle(p.id)}
                          >
                            {isPlaying ? "Stop" : "Play sample"}
                          </button>
                        )}
                        <button
                          type="button"
                          className="btn btn-small btn-ghost"
                          aria-label={`Rename ${p.name}`}
                          onClick={() => setRenaming({ id: p.id, name: p.name })}
                        >
                          Rename
                        </button>
                        <button
                          type="button"
                          className="btn btn-small btn-stop"
                          aria-label={`Reset the voice of ${p.name}`}
                          disabled={p.sampleCount === 0}
                          onClick={() => setConfirm({ kind: "reset", profile: p })}
                        >
                          Reset voice
                        </button>
                        <button
                          type="button"
                          className="btn btn-small btn-record"
                          aria-label={`Delete ${p.name}`}
                          onClick={() => setConfirm({ kind: "delete", profile: p })}
                        >
                          Delete participant
                        </button>
                      </div>
                    </>
                  )}
                </li>
              );
            })}
          </ul>
          <div className="actions">
            <button
              type="button"
              className="btn btn-record"
              onClick={() => setConfirm({ kind: "all" })}
            >
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
                  `The learned voice of ${confirm.profile.name} is forgotten, so ${confirm.profile.name} is no longer recognized automatically. The name is kept.`}
                {confirm.kind === "delete" &&
                  `${confirm.profile.name} and their voice profile are deleted. Voices in meetings that were named after them become unnamed. This cannot be undone.`}
                {confirm.kind === "all" &&
                  "This removes all voice profiles, all stored voice embeddings of your meetings and all sample clips. Participant names are kept, but nobody is recognized automatically until you name voices again. This cannot be undone."}
              </p>
              <div className="actions">
                <button
                  type="button"
                  className="btn btn-ghost"
                  autoFocus
                  onClick={() => setConfirm(null)}
                >
                  Cancel
                </button>
                <button type="button" className="btn btn-record" onClick={runConfirm}>
                  {confirm.kind === "reset" && "Reset voice"}
                  {confirm.kind === "delete" && "Delete participant"}
                  {confirm.kind === "all" && "Delete all voice data"}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
