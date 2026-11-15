import { useState } from "react";
import { useBackend } from "../backend-context";
import { formatDuration } from "../format";
import { errorMessage, useRecordingState, useSources } from "../hooks";
import type { ActiveSource, SourceInfo } from "../ipc/types";

const EQ_BARS = 12;

/** The tray popup: Record and Stop, the sources being recorded, add and remove, screenshot. */
export function Widget() {
  const backend = useBackend();
  const state = useRecordingState();
  const recording = state.status === "recording";
  const sources = useSources(recording);
  const [choice, setChoice] = useState<Set<string> | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Until the user chooses, the microphone (or the first source) is preselected.
  const picked =
    choice ??
    new Set(
      sources.length === 0 ? [] : [(sources.find((s) => s.kind === "microphone") ?? sources[0]).id],
    );

  const run = (p: Promise<unknown>) => {
    setError(null);
    p.catch((e) => setError(errorMessage(e)));
  };

  const live = state.sources.filter((s) => s.status === "recording" || s.status === "starting");
  const addable = sources.filter((s) => !live.some((l) => l.id === s.id));

  return (
    <div className="widget" role="region" aria-label="Recorder">
      <div className="panel">
        <div className="panel-body">
          <span className={recording ? "chip chip-live" : "chip"}>
            <span className={recording ? "dot dot-live" : "dot"} aria-hidden="true" />
            {recording ? "Recording" : "Standby"}
          </span>
          <div className="readout">
            <span
              className={recording ? "timer glitch glitch-fast" : "timer glitch"}
              data-text={formatDuration(state.elapsedMs)}
              aria-label="Elapsed time"
            >
              {formatDuration(state.elapsedMs)}
            </span>
            <span className={recording ? "eq eq-live" : "eq"} aria-hidden="true">
              {Array.from({ length: EQ_BARS }, (_, i) => (
                <i key={i} />
              ))}
            </span>
          </div>
        </div>
      </div>

      <div className="panel quiet grow">
        <div className="panel-body">
          {recording ? (
            <ul className="source-list" aria-label="Recorded sources">
              {state.sources.map((s) => (
                <ActiveRow key={s.id} source={s} onRemove={() => run(backend.removeSource(s.id))} />
              ))}
            </ul>
          ) : (
            <fieldset className="picker">
              <legend>Sources</legend>
              {sources.map((s) => (
                <PickRow
                  key={s.id}
                  source={s}
                  checked={picked.has(s.id)}
                  onToggle={() => {
                    const next = new Set(picked);
                    if (!next.delete(s.id)) next.add(s.id);
                    setChoice(next);
                  }}
                />
              ))}
            </fieldset>
          )}
        </div>
      </div>

      {recording && addable.length > 0 && (
        <div className="add-row">
          <label htmlFor="add-source">Add source</label>
          <select
            id="add-source"
            value=""
            onChange={(e) => e.target.value && run(backend.addSource(e.target.value))}
          >
            <option value="">Choose…</option>
            {addable.map((s) => (
              <option key={s.id} value={s.id}>
                {s.label}
              </option>
            ))}
          </select>
        </div>
      )}

      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}

      <div className="actions">
        {recording ? (
          <>
            <button className="btn" onClick={() => run(backend.takeScreenshot())}>
              Screenshot
              {state.screenshotCount > 0 && <span className="count">{state.screenshotCount}</span>}
            </button>
            <button className="btn btn-stop" onClick={() => run(backend.stopRecording())}>
              Stop
            </button>
          </>
        ) : (
          <button
            className="btn btn-record"
            disabled={picked.size === 0}
            onClick={() => run(backend.startRecording({ sourceIds: [...picked] }))}
          >
            Record
          </button>
        )}
      </div>
    </div>
  );
}

function PickRow(props: { source: SourceInfo; checked: boolean; onToggle: () => void }) {
  const { source, checked, onToggle } = props;
  return (
    <label className="pick-row">
      <input type="checkbox" checked={checked} onChange={onToggle} />
      <span className="pick-label">{source.label}</span>
      <span className="pick-kind">
        {source.kind === "microphone" ? "mic" : source.active ? "playing" : "idle"}
      </span>
    </label>
  );
}

function ActiveRow(props: { source: ActiveSource; onRemove: () => void }) {
  const { source, onRemove } = props;
  const ended = source.status === "ended" || source.status === "error";
  return (
    <li className={ended ? "source-row source-ended" : "source-row"}>
      <span className="source-label">{source.label}</span>
      {ended ? (
        <span className="source-detail">{source.detail ?? "Ended"}</span>
      ) : (
        <>
          <span
            className="meter"
            role="img"
            aria-label={`Level ${Math.round(source.level * 100)} percent`}
          >
            <span className="meter-fill" style={{ width: `${Math.round(source.level * 100)}%` }} />
          </span>
          <button className="icon-btn" aria-label={`Remove ${source.label}`} onClick={onRemove}>
            ×
          </button>
        </>
      )}
    </li>
  );
}
