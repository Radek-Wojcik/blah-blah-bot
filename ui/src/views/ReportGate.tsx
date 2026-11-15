import { useCallback, useEffect, useState } from "react";
import { useBackend } from "../backend-context";
import { newCycleError } from "../cycles";
import { errorMessage } from "../hooks";
import { isIpcError } from "../ipc/tauri";
import type {
  CycleListItem,
  LanguageSetting,
  LastReport,
  ReportDefaults,
  ReportMode,
  StartRecordingRequest,
} from "../ipc/types";
import {
  EMPTY_FORM,
  type GateForm,
  MODE_HELP,
  MODE_NAMES,
  REPORT_MODES,
  describeChoice,
  formFromDefaults,
  gateMessage,
  initialCycleId,
  toReportChoice,
  usableLast,
  validateGate,
} from "../report-gate";
import { Modal } from "./Modal";

type Start = Pick<StartRecordingRequest, "report" | "useLastReport">;

/**
 * The report mode gate: shown every time before a recording starts. It offers the cycle, the
 * report mode and its details, preselected from the cycle's defaults and the last choice. When
 * the last choice is still usable the dialog first shows it as "Record with: CYCLE · mode" and
 * a single Start button; Change opens the whole form. Nothing starts until Start is pressed.
 */
export function ReportGate(props: {
  onCancel: () => void;
  /** Starts the recording with the chosen report settings; a rejection is shown in the dialog. */
  onStart: (req: Start) => Promise<void>;
}) {
  const { onCancel, onStart } = props;
  const backend = useBackend();
  const [cycles, setCycles] = useState<CycleListItem[] | null>(null);
  const [last, setLast] = useState<LastReport | null>(null);
  const [form, setForm] = useState<GateForm>(EMPTY_FORM);
  const [defaults, setDefaults] = useState<ReportDefaults | null>(null);
  const [changing, setChanging] = useState(false);
  const [creating, setCreating] = useState(false);
  const [newId, setNewId] = useState("");
  const [newName, setNewName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // The cycles and the last choice, once.
  useEffect(() => {
    let cancelled = false;
    void Promise.all([backend.listCycles(), backend.getSettings()])
      .then(([list, settings]) => {
        if (cancelled) return;
        setCycles(list);
        setLast(settings.lastReport ?? null);
        setForm({ ...EMPTY_FORM, cycleId: initialCycleId(settings.lastReport, list) });
      })
      .catch((e) => !cancelled && setError(errorMessage(e)));
    return () => {
      cancelled = true;
    };
  }, [backend]);

  // The defaults of the chosen cycle replace the form (the mode, template and type).
  const cycleId = form.cycleId;
  useEffect(() => {
    if (cycleId === "") return;
    let cancelled = false;
    backend
      .reportDefaults(cycleId)
      .then((d) => {
        if (cancelled) return;
        setDefaults(d);
        setForm((f) =>
          f.cycleId === d.cycleId
            ? { ...formFromDefaults(d), outputLanguage: f.outputLanguage }
            : f,
        );
      })
      .catch((e) => {
        if (cancelled) return;
        setError(errorMessage(e));
        setForm((f) => ({ ...f, cycleId: "" }));
      });
    return () => {
      cancelled = true;
    };
  }, [backend, cycleId]);

  const set = (patch: Partial<GateForm>) => setForm((f) => ({ ...f, ...patch }));

  const start = (req: Start) => {
    setBusy(true);
    setError(null);
    onStart(req).catch((e) => {
      setBusy(false);
      setError(gateMessage(isIpcError(e) ? e.code : undefined, errorMessage(e)));
    });
  };

  const createCycle = useCallback(() => {
    setError(null);
    backend
      .createCycle(newId.trim(), newName.trim())
      .then(async (details) => {
        setCycles(await backend.listCycles());
        setCreating(false);
        setNewId("");
        setNewName("");
        setChanging(true);
        setForm({ ...EMPTY_FORM, cycleId: details.cycle.id });
      })
      .catch((e) => setError(errorMessage(e)));
  }, [backend, newId, newName]);

  const usable = usableLast(last, cycles ?? []);
  const problem = validateGate(form, defaults);
  const noCycles = cycles !== null && cycles.length === 0;
  const quick = cycles !== null && usable !== null && !changing && !noCycles;
  const showCreate = creating || noCycles;
  const selected = defaults?.cycleId === form.cycleId ? defaults : null;

  return (
    <Modal title="Choose the report for this recording" onClose={onCancel} wide>
      {cycles === null && !error && <p className="modal-text">Loading the cycles…</p>}

      {quick && usable && (
        <>
          <p className="gate-summary">
            Record with: <strong>{describeChoice(usable)}</strong>
          </p>
          <p className="modal-text">
            The same cycle and report mode as the last recording. Choose Change to pick others.
          </p>
        </>
      )}

      {cycles !== null && !quick && (
        <div className="gate-form">
          {noCycles && (
            <p className="modal-text">
              There is no cycle yet. A cycle holds the participants, vocabulary and report format of
              a series of meetings. Create one to record.
            </p>
          )}
          {!noCycles && (
            <label>
              Cycle
              <select
                aria-label="Cycle"
                value={form.cycleId}
                onChange={(e) => {
                  setDefaults(null);
                  setForm({ ...EMPTY_FORM, cycleId: e.target.value });
                }}
              >
                <option value="">Choose a cycle…</option>
                {cycles.map((c) => (
                  <option key={c.id} value={c.id} disabled={!c.valid}>
                    {c.id} · {c.name}
                    {c.valid ? "" : " (needs fixing)"}
                  </option>
                ))}
              </select>
            </label>
          )}
          {!noCycles && form.cycleId !== "" && (
            <>
              <label>
                Report mode
                <select
                  aria-label="Report mode"
                  value={form.mode}
                  onChange={(e) => set({ mode: e.target.value as ReportMode | "" })}
                >
                  <option value="">Choose a report mode…</option>
                  {REPORT_MODES.map((m) => (
                    <option key={m} value={m}>
                      {MODE_NAMES[m]}
                    </option>
                  ))}
                </select>
                {form.mode !== "" && <small>{MODE_HELP[form.mode]}</small>}
              </label>
              {form.mode === "template" && (
                <label>
                  Template
                  <select
                    aria-label="Template"
                    value={form.template}
                    onChange={(e) => set({ template: e.target.value })}
                  >
                    <option value="">Choose a template…</option>
                    {(selected?.templates ?? []).map((t) => (
                      <option key={t.name} value={t.name}>
                        {t.name}
                        {t.sections.length > 0 ? ` (${t.sections.length} sections)` : ""}
                      </option>
                    ))}
                  </select>
                </label>
              )}
              {form.mode === "by-type" && (
                <label>
                  Meeting type
                  <select
                    aria-label="Meeting type"
                    value={form.meetingType}
                    onChange={(e) => set({ meetingType: e.target.value })}
                  >
                    <option value="">Choose the meeting type…</option>
                    {(selected?.meetingTypes ?? []).map((t) => (
                      <option key={t.name} value={t.name}>
                        {t.name} → {t.template}
                      </option>
                    ))}
                  </select>
                </label>
              )}
              <label>
                Report language
                <select
                  aria-label="Report language"
                  value={form.outputLanguage}
                  onChange={(e) => set({ outputLanguage: e.target.value as LanguageSetting | "" })}
                >
                  <option value="">
                    Cycle setting{selected ? ` (${selected.outputLanguage})` : ""}
                  </option>
                  <option value="auto">Auto (the language of the meeting)</option>
                  <option value="pl">Polish</option>
                  <option value="en">English</option>
                </select>
              </label>
            </>
          )}

          {!showCreate && (
            <button className="link-btn" type="button" onClick={() => setCreating(true)}>
              New cycle…
            </button>
          )}
          {showCreate && (
            <fieldset className="gate-new">
              <legend>New cycle</legend>
              <label>
                Cycle id
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
              <button
                className="btn btn-small"
                type="button"
                disabled={newCycleError(newId, newName) !== null}
                onClick={createCycle}
              >
                Create cycle
              </button>
            </fieldset>
          )}
          {problem && !noCycles && (
            <p className="gate-problem" role="status">
              {problem}
            </p>
          )}
        </div>
      )}

      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <div className="actions">
        <button className="btn btn-ghost" onClick={onCancel}>
          Cancel
        </button>
        {quick && (
          <button className="btn btn-ghost" onClick={() => setChanging(true)}>
            Change…
          </button>
        )}
        <button
          className="btn btn-record"
          autoFocus
          disabled={busy || cycles === null || (quick ? false : problem !== null)}
          onClick={() =>
            quick ? start({ useLastReport: true }) : start({ report: toReportChoice(form) })
          }
        >
          Start recording
        </button>
      </div>
    </Modal>
  );
}
