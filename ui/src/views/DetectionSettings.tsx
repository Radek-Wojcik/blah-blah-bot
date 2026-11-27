import { useEffect, useState } from "react";
import { useBackend } from "../backend-context";
import {
  SENSITIVITIES,
  appLabel,
  appProblem,
  detectionSummary,
  meetWatched,
  withApp,
  withMeet,
} from "../detection";
import { errorMessage } from "../hooks";
import type { DetectionSettings as Settings } from "../ipc/types";

/**
 * Settings > Detection: switch the prompt on or off, choose the sensitivity, the watched
 * applications and the ones that were switched off. Every change is saved at once.
 */
export function DetectionSettingsView() {
  const backend = useBackend();
  const [settings, setSettings] = useState<Settings | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [draft, setDraft] = useState("");

  useEffect(() => {
    let alive = true;
    void backend.getDetectionSettings().then((s) => alive && setSettings(s));
    return () => {
      alive = false;
    };
  }, [backend]);

  if (!settings) return null;
  const save = (next: Settings) => {
    setMessage(null);
    backend
      .saveDetectionSettings(next)
      .then(setSettings)
      .catch((e) => setMessage(errorMessage(e)));
  };
  const problem = draft.trim() === "" ? null : appProblem(draft, settings.knownApps);
  const activeSensitivity = SENSITIVITIES.find((s) => s.value === settings.sensitivity);

  return (
    <section aria-labelledby="detection-heading">
      <div className="panel quiet">
        <div className="panel-body">
          <div className="settings-card-head">
            <h3 id="detection-heading" className="sub-head">
              Detection
            </h3>
            <span className="person-meta" data-testid="detection-state">
              {detectionSummary(settings)}
            </span>
          </div>

          <div className="form">
            <div className="detect-head-row">
              <label className="check">
                <input
                  type="checkbox"
                  checked={settings.enabled}
                  onChange={(e) => save({ ...settings, enabled: e.target.checked })}
                />
                Ask whether to record when a call starts
              </label>
              <label className="check">
                <input
                  type="checkbox"
                  checked={meetWatched(settings)}
                  disabled={!settings.enabled}
                  onChange={(e) => save(withMeet(settings, e.target.checked))}
                />
                Google Meet calls reported by the browser extension
              </label>
            </div>

            <div className="detect-preset-row">
              <span className="field-label">Sensitivity</span>
              <div className="segmented" role="group" aria-label="Detection sensitivity">
                {SENSITIVITIES.map((s) => {
                  const active = settings.sensitivity === s.value;
                  return (
                    <button
                      key={s.value}
                      type="button"
                      className={active ? "btn btn-small btn-stop" : "btn btn-small btn-ghost"}
                      aria-pressed={active}
                      disabled={!settings.enabled}
                      title={s.text}
                      onClick={() => save({ ...settings, sensitivity: s.value })}
                    >
                      {s.label}
                    </button>
                  );
                })}
              </div>
              {activeSensitivity && (
                <small className="detect-preset-hint">{activeSensitivity.text}</small>
              )}
            </div>

            <div>
              <h4 className="field-head" style={{ marginTop: 4 }}>
                Applications that count as calls
              </h4>
              <p className="person-meta" style={{ margin: "0 0 8px" }}>
                A program on this list that uses the microphone is taken as a call. Browsers are
                never detected this way; Google Meet comes from the extension.
              </p>
              <ul className="detect-apps" aria-label="Watched applications">
                {settings.knownApps.map((a) => {
                  const off = settings.optedOut.includes(a);
                  return (
                    <li key={a} className="detect-app-row">
                      <span className="detect-app-name">{appLabel(a)}</span>
                      <span className="detect-app-state person-meta">
                        {a}.exe · {off ? "switched off" : "watching"}
                      </span>
                      <span className="detect-app-actions">
                        {off ? (
                          <button
                            type="button"
                            className="btn btn-small"
                            aria-label={`Ask again for ${appLabel(a)}`}
                            onClick={() =>
                              save({
                                ...settings,
                                optedOut: settings.optedOut.filter((x) => x !== a),
                              })
                            }
                          >
                            Ask again
                          </button>
                        ) : (
                          <button
                            type="button"
                            className="btn btn-small btn-ghost"
                            aria-label={`Turn off ${appLabel(a)}`}
                            onClick={() =>
                              save({ ...settings, optedOut: [...settings.optedOut, a] })
                            }
                          >
                            Turn off
                          </button>
                        )}
                        <button
                          type="button"
                          className="icon-btn"
                          aria-label={`Remove ${appLabel(a)}`}
                          title={`Remove ${appLabel(a)}`}
                          onClick={() =>
                            save({
                              ...settings,
                              knownApps: settings.knownApps.filter((x) => x !== a),
                            })
                          }
                        >
                          ×
                        </button>
                      </span>
                    </li>
                  );
                })}
              </ul>
              <form
                className="detect-add-row"
                onSubmit={(e) => {
                  e.preventDefault();
                  if (draft.trim() === "" || problem) return;
                  save({ ...settings, knownApps: withApp(settings.knownApps, draft) });
                  setDraft("");
                }}
              >
                <label className="detect-add-field">
                  <span>Add a program</span>
                  <input
                    value={draft}
                    placeholder="for example webex"
                    onChange={(e) => setDraft(e.target.value)}
                  />
                </label>
                <button
                  className="btn btn-small"
                  type="submit"
                  disabled={draft.trim() === "" || problem !== null}
                >
                  Add
                </button>
              </form>
              {problem && (
                <p className="error" role="alert" style={{ marginTop: 6 }}>
                  {problem}
                </p>
              )}
              {message && (
                <p className="error" role="alert" style={{ marginTop: 6 }}>
                  {message}
                </p>
              )}
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}
