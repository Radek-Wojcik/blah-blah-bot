import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useBackend } from "../backend-context";
import { formatDuration } from "../format";
import { errorMessage } from "../hooks";
import type { ParticipantView, VoiceView, VoicesData } from "../ipc/types";
import {
  type VoiceGroup,
  groupVoices,
  hasUnconfirmedAuto,
  namingProgress,
  participantName,
  voiceState,
} from "../speakers";

const NEW = "__new";
const SKIP = "__skip";
const EMPTY_SET: ReadonlySet<string> = new Set();
const EMPTY_MAP: ReadonlyMap<string, string | null> = new Map();

/**
 * Plays one sample clip at a time. Starting another clip, leaving the screen or the end of the
 * clip stops the current one. `playing` is `voiceId:idx` of the clip that is loading or playing.
 */
function useClipPlayer(meetingId: string, onError: (message: string) => void) {
  const backend = useBackend();
  const [playing, setPlaying] = useState<string | null>(null);
  const current = useRef<{ audio: HTMLAudioElement; url: string } | null>(null);
  // Every start and stop bumps the token, so a slow download of an older clip is dropped.
  const token = useRef(0);

  const stop = useCallback(() => {
    token.current += 1;
    const c = current.current;
    current.current = null;
    if (c) {
      c.audio.pause();
      if (c.url.startsWith("blob:")) URL.revokeObjectURL(c.url);
    }
    setPlaying(null);
  }, []);

  const toggle = useCallback(
    async (voiceId: string, idx: number) => {
      const key = `${voiceId}:${idx}`;
      const same = playing === key;
      stop();
      if (same) return;
      const mine = token.current;
      setPlaying(key);
      let reported = false;
      const reportError = (msg: string) => {
        if (reported) return;
        reported = true;
        onError(msg);
      };
      try {
        const url = await backend.voiceClip(meetingId, voiceId, idx);
        if (token.current !== mine) {
          if (url.startsWith("blob:")) URL.revokeObjectURL(url);
          return;
        }
        const audio = new Audio(url);
        current.current = { audio, url };
        audio.onended = () => {
          if (token.current === mine) stop();
        };
        audio.onerror = () => {
          if (token.current === mine) {
            stop();
            reportError("The clip could not be played.");
          }
        };
        await audio.play();
      } catch (e) {
        if (token.current === mine) {
          stop();
          reportError(`The clip could not be played: ${errorMessage(e)}`);
        }
      }
    },
    [backend, meetingId, onError, playing, stop],
  );

  useEffect(() => stop, [stop]);
  return { playing, toggle, stop };
}

/** The labeling screen: name the voices detected in a meeting, with sample clips to listen to. */
export function SpeakersView(props: { meetingId: string; onOpenTranscript: () => void }) {
  const { meetingId, onOpenTranscript } = props;
  const backend = useBackend();
  const [data, setData] = useState<VoicesData | null>(null);
  const [meetingMeta, setMeetingMeta] = useState<{
    forMeeting: string;
    title: string;
    cycleId: string | null;
  } | null>(null);
  const [cycleLabel, setCycleLabel] = useState<{ forCycle: string; text: string } | null>(null);
  const [errorState, setErrorState] = useState<{ forMeeting: string; message: string } | null>(
    null,
  );
  // Suggestions the user turned down on this screen; scoped per meetingId.
  const [rejectedState, setRejectedState] = useState<{
    forMeeting: string;
    ids: ReadonlySet<string>;
  }>(() => ({ forMeeting: meetingId, ids: new Set() }));
  // Suggestions accepted on this screen, tracking whether to unlink from `cycleId` on Undo.
  const [acceptedUndoState, setAcceptedUndoState] = useState<{
    forMeeting: string;
    map: ReadonlyMap<string, string | null>;
  }>(() => ({ forMeeting: meetingId, map: new Map() }));

  const activeData = data?.meetingId === meetingId ? data : null;
  const title = meetingMeta?.forMeeting === meetingId ? meetingMeta.title : "";
  const meetingCycleId = meetingMeta?.forMeeting === meetingId ? meetingMeta.cycleId : null;
  const error = errorState?.forMeeting === meetingId ? errorState.message : null;
  const rejected = rejectedState.forMeeting === meetingId ? rejectedState.ids : EMPTY_SET;
  const acceptedUndo =
    acceptedUndoState.forMeeting === meetingId ? acceptedUndoState.map : EMPTY_MAP;

  const setError = useCallback(
    (message: string | null) => {
      setErrorState(message ? { forMeeting: meetingId, message } : null);
    },
    [meetingId],
  );
  const player = useClipPlayer(meetingId, setError);
  const stopPlayer = player.stop;

  useEffect(() => {
    let cancelled = false;
    backend
      .getMeetingVoices(meetingId)
      .then((d) => {
        if (cancelled) return;
        setData(d);
        setErrorState(null);
      })
      .catch(
        (e) => !cancelled && setErrorState({ forMeeting: meetingId, message: errorMessage(e) }),
      );
    backend
      .getMeeting(meetingId)
      .then((m) => {
        if (cancelled) return;
        setMeetingMeta({
          forMeeting: meetingId,
          title: m.summary.title,
          cycleId: m.summary.cycleId ?? null,
        });
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [backend, meetingId]);

  // The meeting's cycle id (even for a brand-new project with no participants yet).
  const cycleId =
    activeData?.cycleId ??
    activeData?.participants.find((p) => p.cycleId)?.cycleId ??
    meetingCycleId ??
    null;
  const cycleName = cycleId && cycleLabel?.forCycle === cycleId ? cycleLabel.text : null;
  useEffect(() => {
    if (!cycleId) return;
    let cancelled = false;
    backend
      .getCycle(cycleId)
      .then(
        (d) =>
          !cancelled && setCycleLabel({ forCycle: cycleId, text: `${d.cycle.name} (${cycleId})` }),
      )
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [backend, cycleId]);

  /** Run a change and show what the backend stored. */
  const act = useCallback(
    async (change: Promise<VoicesData>): Promise<boolean> => {
      setError(null);
      try {
        const d = await change;
        setData(d);
        // The backend drops the clips of a finished meeting; a playing clip must not outlive them.
        if (d.allConfirmed) stopPlayer();
        return true;
      } catch (e) {
        setError(errorMessage(e));
        return false;
      }
    },
    [setError, stopPlayer],
  );

  const groups = useMemo(() => groupVoices(activeData?.voices ?? []), [activeData]);
  const progress = namingProgress(activeData?.voices ?? []);

  return (
    <>
      <div className="page-head">
        <h2>
          <span className="glitch" data-text="Speakers">
            Speakers
          </span>
        </h2>
        <span className="head-actions">
          <button className="btn btn-small" onClick={onOpenTranscript}>
            Transcript
          </button>
        </span>
      </div>
      {title && (
        <p className="card-when">
          {title}
          {cycleId && cycleName && ` · Project: ${cycleName}`}
        </p>
      )}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <p className="notice">
        Names of recognized voices are applied to the transcript automatically, but the voice
        profiles only learn from a match after you confirm it. Voices, profiles and sample clips
        never leave this computer.
      </p>
      {activeData && activeData.voices.length === 0 && (
        <p className="empty">
          No voices were found for this meeting. Process the meeting first, or its voice data was
          deleted.
        </p>
      )}
      {activeData && activeData.voices.length > 0 && (
        <>
          <div className="voice-bar">
            <span className="chip" role="status">
              {progress.text}
            </span>
            {hasUnconfirmedAuto(activeData.voices) && (
              <button
                className="btn btn-small btn-record"
                onClick={() => void act(backend.confirmMatches(meetingId))}
              >
                Confirm all automatic matches
              </button>
            )}
            {activeData.allConfirmed && (
              <span className="voice-done">
                All voices are decided. The sample clips were deleted.
              </span>
            )}
          </div>
          <ul className="voices" aria-label="Detected voices">
            {groups.map((g) => (
              <li key={g.voice.voiceId}>
                <VoiceCard
                  group={g}
                  participants={activeData.participants}
                  candidates={groups.map((x) => x.voice).filter((v) => v !== g.voice)}
                  rejected={rejected.has(g.voice.voiceId)}
                  acceptedUndo={acceptedUndo.has(g.voice.voiceId)}
                  playing={player.playing}
                  onPlay={(idx) => void player.toggle(g.voice.voiceId, idx)}
                  onOpenTranscript={onOpenTranscript}
                  onReject={() =>
                    setRejectedState((prev) => {
                      const base = prev.forMeeting === meetingId ? prev.ids : EMPTY_SET;
                      return { forMeeting: meetingId, ids: new Set(base).add(g.voice.voiceId) };
                    })
                  }
                  onAcceptSuggestion={(participantId) => {
                    const isExternal =
                      !!cycleId && !activeData.participants.some((p) => p.id === participantId);
                    void act(backend.assignVoice(meetingId, g.voice.voiceId, participantId)).then(
                      (ok) => {
                        if (ok) {
                          setAcceptedUndoState((prev) => {
                            const base = prev.forMeeting === meetingId ? prev.map : EMPTY_MAP;
                            return {
                              forMeeting: meetingId,
                              map: new Map(base).set(g.voice.voiceId, isExternal ? cycleId : null),
                            };
                          });
                        }
                      },
                    );
                  }}
                  onUndoAccept={() => {
                    const unlink = acceptedUndo.get(g.voice.voiceId) ?? null;
                    void act(backend.undoVoiceDecision(meetingId, g.voice.voiceId, unlink)).then(
                      (ok) => {
                        if (ok) {
                          setAcceptedUndoState((prev) => {
                            if (prev.forMeeting !== meetingId) return prev;
                            const next = new Map(prev.map);
                            next.delete(g.voice.voiceId);
                            return { forMeeting: meetingId, map: next };
                          });
                        }
                      },
                    );
                  }}
                  onAssign={(participantId) => {
                    setAcceptedUndoState((prev) => {
                      if (prev.forMeeting !== meetingId || !prev.map.has(g.voice.voiceId)) {
                        return prev;
                      }
                      const next = new Map(prev.map);
                      next.delete(g.voice.voiceId);
                      return { forMeeting: meetingId, map: next };
                    });
                    return act(backend.assignVoice(meetingId, g.voice.voiceId, participantId));
                  }}
                  onAssignNew={(name) =>
                    act(backend.assignVoiceNew(meetingId, g.voice.voiceId, name))
                  }
                  onMerge={(voiceId, into) => act(backend.mergeVoices(meetingId, voiceId, into))}
                />
              </li>
            ))}
          </ul>
        </>
      )}
    </>
  );
}

function VoiceCard(props: {
  group: VoiceGroup;
  participants: ParticipantView[];
  /** The other top-level voices this one can be declared the same person as. */
  candidates: VoiceView[];
  rejected: boolean;
  acceptedUndo: boolean;
  playing: string | null;
  onPlay: (idx: number) => void;
  onOpenTranscript: () => void;
  onReject: () => void;
  onAcceptSuggestion: (participantId: string) => void;
  onUndoAccept: () => void;
  onAssign: (participantId: string | null) => Promise<boolean>;
  onAssignNew: (name: string) => Promise<boolean>;
  onMerge: (voiceId: string, into: string | null) => Promise<boolean>;
}) {
  const { group, participants, candidates, rejected, acceptedUndo, playing } = props;
  const { voice, merged } = group;
  const [creating, setCreating] = useState(false);
  const [newName, setNewName] = useState("");
  const state = voiceState(voice, participants, rejected);
  const named =
    participantName(participants, voice.assignedParticipantId) ??
    voice.assignedParticipantName ??
    null;
  const known = participants.some((p) => p.id === voice.assignedParticipantId);
  const assigned = voice.assignedParticipantId ?? "";
  let selected = "";
  if (creating) selected = NEW;
  else if (voice.confirmed) {
    selected = voice.assignedParticipantId ? (known || named ? assigned : "") : SKIP;
  } else if (voice.decision === "auto" && (known || named)) {
    selected = assigned;
  }

  const pick = (value: string) => {
    if (value === NEW) {
      setCreating(true);
    } else {
      setCreating(false);
      void props.onAssign(value === SKIP ? null : value);
    }
  };

  const nameOf = (v: VoiceView) => {
    const n = participantName(participants, v.assignedParticipantId) ?? v.assignedParticipantName;
    return n ? `${v.label} (${n})` : v.label;
  };

  return (
    <article className={`panel voice voice-${state.kind}`} aria-label={voice.label}>
      <div className="panel-body">
        <h3 className="card-title">
          {voice.label}
          {named && <span className="voice-name">{named}</span>}
          <span className={`badge badge-voice-${state.kind}`}>{state.label}</span>
        </h3>
        <p className="card-when">Speaking time {formatDuration(voice.speechMs)}</p>

        {state.kind === "suggested" && (
          <div className="voice-suggest">
            <button
              className="btn btn-small"
              aria-label={`Accept the suggestion for ${voice.label}`}
              onClick={() => state.participantId && props.onAcceptSuggestion(state.participantId)}
            >
              Accept
            </button>
            <button
              className="btn btn-small btn-ghost"
              aria-label={`Reject the suggestion for ${voice.label}`}
              onClick={props.onReject}
            >
              Reject
            </button>
          </div>
        )}
        {acceptedUndo && voice.confirmed && (
          <div className="voice-suggest">
            <span className="person-meta">
              Accepted ·{" "}
              <button
                type="button"
                className="btn btn-small btn-ghost"
                aria-label={`Undo accepted suggestion for ${voice.label}`}
                onClick={props.onUndoAccept}
              >
                Undo
              </button>
            </span>
          </div>
        )}
        {state.kind === "auto" && (
          <div className="voice-suggest">
            <button
              className="btn btn-small"
              aria-label={`Confirm the match for ${voice.label}`}
              onClick={() => void props.onAssign(state.participantId)}
            >
              Confirm
            </button>
          </div>
        )}

        {voice.clips.length > 0 ? (
          <ul className="clips" aria-label={`Sample clips of ${voice.label}`}>
            {voice.clips.map((c) => {
              const on = playing === `${voice.voiceId}:${c.idx}`;
              return (
                <li key={c.idx} className={on ? "clip clip-on" : "clip"}>
                  <button
                    className="btn btn-small btn-stop clip-play"
                    aria-label={`${on ? "Stop" : "Play"} clip ${c.idx + 1} of ${voice.label}`}
                    onClick={() => props.onPlay(c.idx)}
                  >
                    {on ? "Stop" : "Play"}
                  </button>
                  <span className="clip-time">{formatDuration(c.endMs - c.startMs)}</span>
                  <span className="clip-text">{c.text}</span>
                </li>
              );
            })}
          </ul>
        ) : (
          <p className="clip-missing">
            No sample clips are available for this voice; use the transcript to recognize it.{" "}
            <button className="link-btn" onClick={props.onOpenTranscript}>
              Open the transcript
            </button>
          </p>
        )}

        <div className="voice-tools">
          <select
            className="voice-select"
            aria-label={`Name for ${voice.label}`}
            value={selected}
            onChange={(e) => pick(e.target.value)}
          >
            <option value="" disabled>
              Choose a name…
            </option>
            {!known && voice.assignedParticipantId && named && (
              <option value={voice.assignedParticipantId}>{named}</option>
            )}
            {participants.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
            <option value={NEW}>New participant…</option>
            <option value={SKIP}>Not a person / skip</option>
          </select>
          <select
            className="voice-select"
            aria-label={`Same person as, for ${voice.label}`}
            value=""
            disabled={candidates.length === 0}
            onChange={(e) => void props.onMerge(voice.voiceId, e.target.value)}
          >
            <option value="" disabled>
              Same person as…
            </option>
            {candidates.map((v) => (
              <option key={v.voiceId} value={v.voiceId}>
                {nameOf(v)}
              </option>
            ))}
          </select>
        </div>
        {creating && (
          <form
            className="voice-new"
            onSubmit={(e) => {
              e.preventDefault();
              void props.onAssignNew(newName).then((ok) => {
                if (ok) {
                  setCreating(false);
                  setNewName("");
                }
              });
            }}
          >
            <input
              className="voice-input"
              aria-label={`Name of the new participant for ${voice.label}`}
              placeholder="Name of the new participant"
              maxLength={80}
              autoFocus
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
            />
            <button className="btn btn-small" type="submit" disabled={newName.trim() === ""}>
              Create and name
            </button>
            <button
              className="btn btn-small btn-ghost"
              type="button"
              onClick={() => {
                setCreating(false);
                setNewName("");
              }}
            >
              Cancel
            </button>
          </form>
        )}

        {merged.length > 0 && (
          <ul className="merged" aria-label={`Voices merged into ${voice.label}`}>
            {merged.map((m) => (
              <li key={m.voiceId}>
                <span className="merged-label">{m.label}</span>
                <span className="merged-note">
                  same person · speaking time {formatDuration(m.speechMs)}
                </span>
                <button
                  className="btn btn-small btn-ghost"
                  aria-label={`Undo the merge of ${m.label}`}
                  onClick={() => void props.onMerge(m.voiceId, null)}
                >
                  Undo
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </article>
  );
}
