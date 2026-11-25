import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useBackend } from "../backend-context";
import { formatDuration } from "../format";
import { errorMessage } from "../hooks";
import { isIpcError } from "../ipc/tauri";
import type { ScreenshotDescription, TranscriptData, TranscriptSegment } from "../ipc/types";
import { buildTimeline, segmentAt, speakerColors } from "../processing";

/** Transcript of one meeting: speakers, segments with screenshots in between, and the editor. */
export function TranscriptView(props: {
  meetingId: string;
  onBack?: () => void;
  onOpenSpeakers: () => void;
  /** A time in the meeting: the segment there is scrolled into view and highlighted. */
  highlightMs?: number | null;
}) {
  const { meetingId, onOpenSpeakers, highlightMs } = props;
  const backend = useBackend();
  const [data, setData] = useState<TranscriptData | null>(null);
  const [title, setTitle] = useState<string>("");
  const [notReady, setNotReady] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hideExcluded, setHideExcluded] = useState(false);

  const load = useCallback(() => {
    backend
      .getTranscript(meetingId)
      .then((t) => {
        setNotReady(false);
        setData(t);
      })
      .catch((e) => {
        if (isIpcError(e) && e.code === "not_ready") setNotReady(true);
        else setError(errorMessage(e));
      });
  }, [backend, meetingId]);

  useEffect(() => {
    load();
    void backend
      .getMeeting(meetingId)
      .then((m) => setTitle(m.summary.title))
      .catch(() => undefined);
  }, [backend, meetingId, load]);

  const colors = useMemo(() => speakerColors(data?.speakers ?? []), [data]);
  const labels = useMemo(
    () => Object.fromEntries((data?.speakers ?? []).map((s) => [s.id, s.label])),
    [data],
  );
  const timeline = useMemo(
    () => (data ? buildTimeline(data.segments, data.screenshots) : []),
    [data],
  );

  /** Persist an edit, then reload so the view shows what the backend stored. */
  const save = async (segment: TranscriptSegment, text: string | null, excluded: boolean) => {
    setError(null);
    try {
      await backend.editSegment(meetingId, segment.id, { text, excluded });
      load();
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  // The segment to show (a jump from the draft), scrolled to once the transcript is there.
  const target =
    data && highlightMs !== undefined && highlightMs !== null
      ? (segmentAt(data.segments, highlightMs)?.id ?? null)
      : null;
  const targetRef = useRef<HTMLLIElement | null>(null);
  const scrolledTo = useRef<number | null>(null);
  useEffect(() => {
    // Once per jump: reloading the transcript after an edit must not scroll again.
    if (target === null || scrolledTo.current === target) return;
    scrolledTo.current = target;
    targetRef.current?.scrollIntoView({ block: "center" });
  }, [target, data]);

  const excludedCount = data?.segments.filter((s) => s.excluded).length ?? 0;

  return (
    <>
      <div className="page-head">
        <h2>
          <span className="glitch" data-text="Transcript">
            Transcript
          </span>
        </h2>
        <span className="head-actions">
          <button className="btn btn-small" onClick={onOpenSpeakers}>
            Name speakers
          </button>
        </span>
      </div>
      {title && <p className="card-when">{title}</p>}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {notReady && <p className="empty">This meeting has no transcript yet. Process it first.</p>}
      {data && (
        <>
          <div className="transcript-head">
            <span className="chip">Language: {data.language}</span>
            <ul className="legend" aria-label="Speakers">
              {data.speakers.map((s) => (
                <li key={s.id}>
                  <span
                    className="swatch"
                    style={{ background: colors[s.id] }}
                    aria-hidden="true"
                  />
                  {s.label}
                </li>
              ))}
            </ul>
            <label className="check">
              <input
                type="checkbox"
                checked={hideExcluded}
                onChange={(e) => setHideExcluded(e.target.checked)}
              />
              Hide excluded{excludedCount > 0 ? ` (${excludedCount})` : ""}
            </label>
          </div>
          <ol className="segments" aria-label="Transcript">
            {timeline.map((item) => {
              if (item.kind === "screenshot") {
                return (
                  <li key={`shot-${item.screenshot.file}`} className="shot-row">
                    <ShotRow meetingId={meetingId} shot={item.screenshot} />
                  </li>
                );
              }
              const seg = item.segment;
              if (hideExcluded && seg.excluded) return null;
              return (
                <li
                  key={`seg-${seg.id}`}
                  ref={seg.id === target ? targetRef : undefined}
                  aria-current={seg.id === target ? "true" : undefined}
                  className={
                    (seg.excluded ? "seg seg-excluded" : "seg") +
                    (seg.id === target ? " seg-highlight" : "")
                  }
                >
                  <SegmentRow
                    segment={seg}
                    speaker={labels[seg.speaker] ?? seg.speaker}
                    color={colors[seg.speaker] ?? "var(--muted)"}
                    onSave={(text) => save(seg, text, seg.excluded)}
                    // `text: null` would restore the recognized text, so a corrected text is sent along.
                    onToggle={() =>
                      save(seg, seg.originalText !== null ? seg.text : null, !seg.excluded)
                    }
                    onRestore={() => save(seg, null, seg.excluded)}
                    onError={setError}
                  />
                </li>
              );
            })}
          </ol>
        </>
      )}
    </>
  );
}

function SegmentRow(props: {
  segment: TranscriptSegment;
  speaker: string;
  color: string;
  onSave: (text: string) => Promise<void>;
  onToggle: () => Promise<void>;
  onRestore: () => Promise<void>;
  onError: (message: string | null) => void;
}) {
  const { segment, speaker, color, onSave, onToggle, onRestore, onError } = props;
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(segment.text);
  // Set when Escape or a save already ended the edit, so the blur that follows does nothing.
  const finished = useRef(false);
  const time = formatDuration(segment.startMs);

  const begin = () => {
    finished.current = false;
    setDraft(segment.text);
    setEditing(true);
  };
  const cancel = () => {
    finished.current = true;
    setEditing(false);
  };
  const commit = () => {
    if (finished.current) return;
    finished.current = true;
    const text = draft.trim();
    setEditing(false);
    if (text === segment.text) return;
    if (text === "") {
      onError("The text of a segment cannot be empty. Exclude the segment instead.");
      return;
    }
    void onSave(text);
  };

  return (
    <>
      <span className="seg-time">{time}</span>
      <span className="seg-speaker" style={{ color }}>
        {speaker}
      </span>
      <div className="seg-body">
        {editing ? (
          <textarea
            className="seg-edit"
            aria-label={`Correct the text at ${time}`}
            value={draft}
            rows={2}
            autoFocus
            onChange={(e) => setDraft(e.target.value)}
            onBlur={commit}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                commit();
              } else if (e.key === "Escape") {
                e.preventDefault();
                cancel();
              }
            }}
          />
        ) : (
          <button className="seg-text" title="Click to correct the text" onClick={begin}>
            {segment.text}
          </button>
        )}
        <span className="seg-tags">
          {segment.originalText !== null && (
            <>
              <span className="badge badge-edited" title={`Recognized as: ${segment.originalText}`}>
                edited
              </span>
              <button
                className="link-btn"
                aria-label={`Restore the original text at ${time}`}
                onClick={() => void onRestore()}
              >
                Restore original
              </button>
            </>
          )}
          {segment.excluded && <span className="badge badge-excluded">excluded from reports</span>}
        </span>
      </div>
      <button
        className="btn btn-small btn-ghost seg-toggle"
        aria-label={`${segment.excluded ? "Include" : "Exclude"} the segment at ${time}`}
        onClick={() => void onToggle()}
      >
        {segment.excluded ? "Include" : "Exclude"}
      </button>
    </>
  );
}

/** A screenshot at its time position with its (read-only) description. */
function ShotRow(props: { meetingId: string; shot: ScreenshotDescription }) {
  const { meetingId, shot } = props;
  const backend = useBackend();
  const [src, setSrc] = useState<string | null>(null);
  const time = formatDuration(shot.offsetMs);

  useEffect(() => {
    let cancelled = false;
    let url: string | null = null;
    void backend
      .meetingImage(meetingId, shot.file)
      .then((u) => {
        if (cancelled) {
          if (u.startsWith("blob:")) URL.revokeObjectURL(u);
          return;
        }
        url = u;
        setSrc(u);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
      if (url?.startsWith("blob:")) URL.revokeObjectURL(url);
    };
  }, [backend, meetingId, shot.file]);

  return (
    <>
      <span className="seg-time">{time}</span>
      <span className="seg-speaker shot-label">Screenshot</span>
      <figure className="shot-fig">
        {src ? (
          <img className="shot-thumb" src={src} alt={`Screenshot taken at ${time}`} />
        ) : (
          <span className="shot-thumb shot-missing" aria-hidden="true" />
        )}
        <figcaption className="shot-desc">{shot.description}</figcaption>
      </figure>
    </>
  );
}
