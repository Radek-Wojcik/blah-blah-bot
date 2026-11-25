import { useCallback, useEffect, useId, useState } from "react";
import { useBackend } from "../backend-context";
import { newCycleError } from "../cycles";
import { formatDuration } from "../format";
import { errorMessage, useRecordingLevels, useRecordingState, useSources } from "../hooks";
import { isIpcError } from "../ipc/tauri";
import type {
  ActiveSource,
  CycleListItem,
  LanguageSetting,
  ReportDefaults,
  ReportMode,
  SourceInfo,
} from "../ipc/types";
import {
  doubleRecording,
  groupSources,
  kindLabel,
  restorePersistedSelection,
  selectionStableKeys,
} from "../picker";
import {
  EMPTY_FORM,
  type GateForm,
  MODE_HELP,
  MODE_NAMES,
  REPORT_MODES,
  formFromDefaults,
  gateMessage,
  initialCycleId,
  toReportChoice,
  validateGate,
} from "../report-gate";
import { HelpPopover } from "./HelpPopover";
import { WaveformEq } from "./WaveformEq";

export interface RecorderPanelProps {
  /** Compact mode for the tray widget vs full mode for the main window Record view. */
  compact?: boolean;
  /** Optional externally controlled project id. */
  cycleId?: string;
  /** Optional callback when the user changes the selected project. */
  onCycleChange?: (cycleId: string) => void;
  className?: string;
}

/**
 * Self-contained recording control component ready to embed in both `RecordView.tsx` (Main UI)
 * and `Widget.tsx` (tray). Includes Project picker, Report mode selector, Source checklist
 * with `restorePersistedSelection` and auto-saving `saveLastSources`, live `WaveformEq`, elapsed
 * timer, Screenshot button, and Record/Stop button.
 */
export function RecorderPanel(props: RecorderPanelProps = {}) {
  const { compact = false, cycleId: controlledCycleId, onCycleChange, className } = props;
  const backend = useBackend();
  const state = useRecordingState();
  const recording = state.status === "recording";
  const levels = useRecordingLevels(recording);
  const sources = useSources(recording);

  const [lastKeys, setLastKeys] = useState<string[] | null>(null);
  const [choice, setChoice] = useState<Set<string> | null>(null);
  const [cycles, setCycles] = useState<CycleListItem[] | null>(null);
  const [form, setForm] = useState<GateForm>(EMPTY_FORM);
  const [defaults, setDefaults] = useState<ReportDefaults | null>(null);
  const [creatingCycle, setCreatingCycle] = useState(false);
  const [newId, setNewId] = useState("");
  const [newName, setNewName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const cycleSelectId = useId();
  const modeSelectId = useId();

  const loadCyclesAndSettings = useCallback(() => {
    void Promise.all([backend.listCycles(), backend.getSettings()])
      .then(([list, settings]) => {
        setCycles(list);
        setLastKeys(settings.lastSourceKeys ?? []);
        setForm((prev) => {
          const targetCycle =
            controlledCycleId ??
            (prev.cycleId && list.some((c) => c.id === prev.cycleId && c.valid)
              ? prev.cycleId
              : initialCycleId(settings.lastReport, list));
          return {
            ...prev,
            cycleId: targetCycle,
          };
        });
      })
      .catch((e) => setError(errorMessage(e)));
  }, [backend, controlledCycleId]);

  useEffect(() => {
    loadCyclesAndSettings();
    let off: (() => void) | undefined;
    let cancelled = false;
    void backend.onCyclesChanged(loadCyclesAndSettings).then((u) => {
      if (cancelled) u();
      else off = u;
    });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [backend, loadCyclesAndSettings]);

  const activeCycleId = controlledCycleId ?? form.cycleId;
  const effectiveForm: GateForm =
    activeCycleId === form.cycleId ? form : { ...form, cycleId: activeCycleId };

  // Load report defaults whenever the selected project changes.
  useEffect(() => {
    if (!activeCycleId) return;
    let cancelled = false;
    backend
      .reportDefaults(activeCycleId)
      .then((d) => {
        if (cancelled) return;
        setDefaults(d);
        setForm((prev) =>
          (controlledCycleId ?? prev.cycleId) === d.cycleId
            ? { ...formFromDefaults(d), outputLanguage: prev.outputLanguage }
            : prev,
        );
      })
      .catch((e) => {
        if (cancelled) return;
        setError(errorMessage(e));
      });
    return () => {
      cancelled = true;
    };
  }, [backend, activeCycleId, controlledCycleId]);

  const picked = choice ?? restorePersistedSelection(sources, lastKeys);

  const run = (p: Promise<unknown>) => {
    setError(null);
    p.catch((e) => setError(gateMessage(isIpcError(e) ? e.code : undefined, errorMessage(e))));
  };

  const toggleSource = (sourceId: string) => {
    const next = new Set(picked);
    if (!next.delete(sourceId)) next.add(sourceId);
    setChoice(next);
    const stableKeys = selectionStableKeys(sources, next);
    setLastKeys(stableKeys);
    void backend.saveLastSources(stableKeys).catch(() => {});
  };

  const handleCreateCycle = () => {
    setError(null);
    backend
      .createCycle(newId.trim(), newName.trim())
      .then(async (details) => {
        const list = await backend.listCycles();
        setCycles(list);
        setCreatingCycle(false);
        setNewId("");
        setNewName("");
        setForm({ ...EMPTY_FORM, cycleId: details.cycle.id });
        onCycleChange?.(details.cycle.id);
      })
      .catch((e) => setError(errorMessage(e)));
  };

  const handleStartRecording = () => {
    setError(null);
    const problem = validateGate(effectiveForm, defaults);
    if (problem !== null) {
      setError(problem);
      return;
    }
    run(
      backend.startRecording({
        sourceIds: [...picked],
        report: toReportChoice(effectiveForm),
      }),
    );
  };

  const handleStopRecording = () => {
    run(backend.stopRecording());
  };

  const live = state.sources.filter((s) => s.status === "recording" || s.status === "starting");
  const addable = sources.filter((s) => !live.some((l) => l.id === s.id));
  const doubled = doubleRecording(sources, recording ? live.map((l) => l.id) : picked);
  const noCycles = cycles !== null && cycles.length === 0;
  const selectedDefaults = defaults?.cycleId === activeCycleId ? defaults : null;
  const gateProblem = validateGate(effectiveForm, defaults);

  return (
    <div
      className={[
        "recorder-panel",
        compact ? "widget recorder-panel-compact" : "recorder-panel-full",
        className ?? "",
      ]
        .filter(Boolean)
        .join(" ")}
      role="region"
      aria-label="Recorder"
    >
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
            <WaveformEq levels={levels} active={recording} />
          </div>
        </div>
      </div>

      {!recording && (
        <div className="panel quiet">
          <div className="panel-body">
            <div className="recorder-config-grid">
              {noCycles ? (
                <p className="modal-text">
                  No project exists yet. Create a project below to configure participants and report
                  templates.
                </p>
              ) : (
                <div className="inline-field">
                  <div className="field-label-row">
                    <label htmlFor={cycleSelectId}>Project</label>
                    <HelpPopover
                      title="Project"
                      text="Each project stores its participants, glossary, report templates, and publishing targets."
                      recommended="Pick the project that matches this meeting series."
                    />
                  </div>
                  <select
                    id={cycleSelectId}
                    aria-label="Project"
                    value={activeCycleId}
                    onChange={(e) => {
                      const nextId = e.target.value;
                      setDefaults(null);
                      setForm({ ...EMPTY_FORM, cycleId: nextId });
                      onCycleChange?.(nextId);
                    }}
                  >
                    <option value="">Choose a project…</option>
                    {(cycles ?? []).map((c) => (
                      <option key={c.id} value={c.id} disabled={!c.valid}>
                        {c.id} · {c.name}
                        {c.valid ? "" : " (needs fixing)"}
                      </option>
                    ))}
                  </select>
                </div>
              )}

              {!noCycles && activeCycleId !== "" && (
                <>
                  <div className="inline-field">
                    <div className="field-label-row">
                      <label htmlFor={modeSelectId}>Report mode</label>
                      <HelpPopover
                        title="Report mode"
                        text="Controls how extracted tasks, decisions, and notes are organized into sections."
                        recommended="Auto or Fixed for general meetings; Template or By type for recurring formats."
                      />
                    </div>
                    <select
                      id={modeSelectId}
                      aria-label="Report mode"
                      value={form.mode}
                      onChange={(e) =>
                        setForm((prev) => ({
                          ...prev,
                          mode: e.target.value as ReportMode | "",
                        }))
                      }
                    >
                      <option value="">Choose a report mode…</option>
                      {REPORT_MODES.map((m) => (
                        <option key={m} value={m}>
                          {MODE_NAMES[m]}
                        </option>
                      ))}
                    </select>
                    {form.mode !== "" && <small>{MODE_HELP[form.mode]}</small>}
                  </div>

                  {form.mode === "template" && (
                    <label className="inline-field">
                      Template
                      <select
                        aria-label="Template"
                        value={form.template}
                        onChange={(e) =>
                          setForm((prev) => ({
                            ...prev,
                            template: e.target.value,
                          }))
                        }
                      >
                        <option value="">Choose a template…</option>
                        {(selectedDefaults?.templates ?? []).map((t) => (
                          <option key={t.name} value={t.name}>
                            {t.name}
                            {t.sections.length > 0 ? ` (${t.sections.length} sections)` : ""}
                          </option>
                        ))}
                      </select>
                    </label>
                  )}

                  {form.mode === "by-type" && (
                    <label className="inline-field">
                      Meeting type
                      <select
                        aria-label="Meeting type"
                        value={form.meetingType}
                        onChange={(e) =>
                          setForm((prev) => ({
                            ...prev,
                            meetingType: e.target.value,
                          }))
                        }
                      >
                        <option value="">Choose the meeting type…</option>
                        {(selectedDefaults?.meetingTypes ?? []).map((t) => (
                          <option key={t.name} value={t.name}>
                            {t.name} → {t.template}
                          </option>
                        ))}
                      </select>
                    </label>
                  )}

                  <label className="inline-field">
                    Report language
                    <select
                      aria-label="Report language"
                      value={form.outputLanguage}
                      onChange={(e) =>
                        setForm((prev) => ({
                          ...prev,
                          outputLanguage: e.target.value as LanguageSetting | "",
                        }))
                      }
                    >
                      <option value="">
                        Project setting
                        {selectedDefaults ? ` (${selectedDefaults.outputLanguage})` : ""}
                      </option>
                      <option value="auto">Auto (the language of the meeting)</option>
                      <option value="pl">Polish</option>
                      <option value="en">English</option>
                    </select>
                  </label>
                </>
              )}
            </div>

            {!creatingCycle && !noCycles && (
              <div className="actions" style={{ marginTop: 8, justifyContent: "space-between" }}>
                <button className="link-btn" type="button" onClick={() => setCreatingCycle(true)}>
                  New project…
                </button>
                {activeCycleId !== "" && (
                  <button
                    className="link-btn"
                    type="button"
                    title={`Open briefing of ${activeCycleId}`}
                    onClick={() =>
                      backend.openBriefing(activeCycleId).catch((e) => setError(errorMessage(e)))
                    }
                  >
                    Open briefing
                  </button>
                )}
              </div>
            )}

            {(creatingCycle || noCycles) && (
              <fieldset className="gate-new" style={{ marginTop: 8 }}>
                <legend>New project</legend>
                <label>
                  Project ID
                  <input
                    value={newId}
                    placeholder="NOMAD"
                    onChange={(e) => setNewId(e.target.value)}
                  />
                </label>
                <label>
                  Name
                  <input
                    value={newName}
                    placeholder="Nomad rover"
                    onChange={(e) => setNewName(e.target.value)}
                  />
                </label>
                {newId !== "" && newCycleError(newId, newName) && (
                  <small>{newCycleError(newId, newName)}</small>
                )}
                <div className="actions">
                  {!noCycles && (
                    <button
                      className="btn btn-small btn-ghost"
                      type="button"
                      onClick={() => setCreatingCycle(false)}
                    >
                      Cancel
                    </button>
                  )}
                  <button
                    className="btn btn-small"
                    type="button"
                    disabled={newCycleError(newId, newName) !== null}
                    onClick={handleCreateCycle}
                  >
                    Create project
                  </button>
                </div>
              </fieldset>
            )}
          </div>
        </div>
      )}

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
                        onToggle={() => toggleSource(row.source.id)}
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
          <label htmlFor="recorder-add-source">Add source</label>
          <select
            id="recorder-add-source"
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

      <div className="actions">
        {recording ? (
          <>
            <button type="button" className="btn" onClick={() => run(backend.takeScreenshot())}>
              Screenshot
              {state.screenshotCount > 0 && <span className="count">{state.screenshotCount}</span>}
            </button>
            <button type="button" className="btn btn-stop" onClick={handleStopRecording}>
              Stop
            </button>
          </>
        ) : (
          <button
            type="button"
            className="btn btn-record"
            disabled={picked.size === 0 || gateProblem !== null}
            onClick={handleStartRecording}
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
          <button
            type="button"
            className="icon-btn"
            aria-label={`Remove ${source.label}`}
            onClick={onRemove}
          >
            ×
          </button>
        </>
      )}
    </li>
  );
}
