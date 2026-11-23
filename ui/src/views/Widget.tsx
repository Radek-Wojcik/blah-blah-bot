import { useState } from "react";
import { useBackend } from "../backend-context";
import { formatDuration } from "../format";
import { errorMessage, useRecordingState, useSources } from "../hooks";
import type { ActiveSource, SourceInfo } from "../ipc/types";
import { defaultSelection, doubleRecording, groupSources, kindLabel } from "../picker";
import { DetectionPrompt } from "./DetectionPrompt";
import { ProcessPrompt } from "./ProcessPrompt";
import { ReportGate } from "./ReportGate";

const EQ_BARS = 12;

/** The tray popup: Record and Stop, the sources being recorded, add and remove, screenshot. */
export function Widget() {
  const backend = useBackend();
  const state = useRecordingState();
  const recording = state.status === "recording";
  const sources = useSources(recording);
  const [choice, setChoice] = useState<Set<string> | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Every recording starts through the report mode gate; Record only opens it.
  const [gate, setGate] = useState(false);

  // Until the user chooses, the microphone (or the first source) and the captured tabs are ticked.
  const picked = choice ?? defaultSelection(sources);

  const run = (p: Promise<unknown>) => {
    setError(null);
    p.catch((e) => setError(errorMessage(e)));
  };

  const live = state.sources.filter((s) => s.status === "recording" || s.status === "starting");
  const addable = sources.filter((s) => !live.some((l) => l.id === s.id));
  // A tab and the whole browser together record the sound of the tab twice.
  const doubled = doubleRecording(sources, recording ? live.map((l) => l.id) : picked);

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
              {groupSources(sources).map((section) => (
                <div
                  key={section.id}
                  className="pick-section"
                  role="group"
                  aria-label={section.title}
                >
                  <p className="pick-heading" aria-hidden="true">
                    {section.title}
                  </p>
                  {section.rows.map((row) =>
                    row.type === "heading" ? (
                      <p key={`h-${row.text}`} className="pick-subheading">
                        {row.text}
                      </p>
                    ) : (
                      <PickRow
                        key={row.source.id}
                        source={row.source}
                        checked={picked.has(row.source.id)}
                        onToggle={() => {
                          const next = new Set(picked);
                          if (!next.delete(row.source.id)) next.add(row.source.id);
                          setChoice(next);
                        }}
                      />
                    ),
                  )}
                </div>
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

      {doubled && (
        <p className="pick-warning" role="status">
          {doubled}
        </p>
      )}

      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}

      <ProcessPrompt />
      {/* The demo mock raises its prompt here; the application shows it in its own window. */}
      {new URLSearchParams(window.location.search).has("detectms") && <DetectionPrompt embedded />}
      {gate && (
        <ReportGate
          onCancel={() => setGate(false)}
          onStart={(extra) =>
            backend.startRecording({ sourceIds: [...picked], ...extra }).then(() => setGate(false))
          }
        />
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
            onClick={() => {
              setError(null);
              setGate(true);
            }}
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
  const hintId = source.hint ? `hint-${source.id}` : undefined;
  return (
    <div className="pick-item">
      <label className="pick-row">
        <input type="checkbox" checked={checked} onChange={onToggle} aria-describedby={hintId} />
        <span className="pick-label">{source.label}</span>
        <span className="pick-kind">{kindLabel(source)}</span>
      </label>
      {source.hint && (
        <p id={hintId} className="pick-hint">
          {source.hint}
        </p>
      )}
    </div>
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
