import { useEffect, useState } from "react";
import { useBackend } from "../backend-context";
import { SENSITIVITIES, appLabel, appProblem, detectionSummary, withApp } from "../detection";
import { errorMessage } from "../hooks";
import type { DetectionSettings as Settings } from "../ipc/types";

/**
 * Settings > Detection: switch the prompt on or off, choose the sensitivity, the watched
 * applications and the ones that were switched off with "Never". Every change is saved at once.
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

  return (
    <section aria-labelledby="detection-heading">
      <h3 id="detection-heading" className="sub-head">
        Detection
      </h3>
      <div className="panel quiet" style={{ maxWidth: 620 }}>
        <div className="panel-body">
          <div className="form">
            <label className="check">
              <input
                type="checkbox"
                checked={settings.enabled}
                onChange={(e) => save({ ...settings, enabled: e.target.checked })}
              />
              Ask whether to record when a call starts
            </label>
            <p data-testid="detection-state">{detectionSummary(settings)}</p>

            <fieldset disabled={!settings.enabled}>
              <legend>Sensitivity</legend>
              {SENSITIVITIES.map((s) => (
                <label key={s.value} className="check">
                  <input
                    type="radio"
                    name="sensitivity"
                    checked={settings.sensitivity === s.value}
                    onChange={() => save({ ...settings, sensitivity: s.value })}
                  />
                  <span>
                    {s.label}
                    <small> {s.text}</small>
                  </span>
                </label>
              ))}
            </fieldset>

            <label className="check">
              <input
                type="checkbox"
                checked={settings.meet}
                disabled={!settings.enabled}
                onChange={(e) => save({ ...settings, meet: e.target.checked })}
              />
              Google Meet calls reported by the browser extension
            </label>
          </div>
        </div>
      </div>

      <h4 className="sub-head tight">Applications that count as calls</h4>
      <p className="person-meta">
        A program on this list that uses the microphone is taken as a call. Browsers are never
        detected this way; Google Meet comes from the extension.
      </p>
      <ul className="people" aria-label="Watched applications">
        {settings.knownApps.map((a) => {
          const off = settings.optedOut.includes(a);
          return (
            <li key={a} className="person">
              <span className="person-main">
                <span>{appLabel(a)}</span>
                <span className="person-meta">
                  {a}.exe{off ? " · switched off with Never" : ""}
                </span>
              </span>
              {off ? (
                <button
                  className="btn"
                  aria-label={`Ask again for ${appLabel(a)}`}
                  onClick={() =>
                    save({ ...settings, optedOut: settings.optedOut.filter((x) => x !== a) })
                  }
                >
                  Ask again
                </button>
              ) : (
                <button
                  className="btn"
                  aria-label={`Never ask for ${appLabel(a)}`}
                  onClick={() => save({ ...settings, optedOut: [...settings.optedOut, a] })}
                >
                  Never ask
                </button>
              )}
              <button
                className="btn"
                aria-label={`Remove ${appLabel(a)}`}
                onClick={() =>
                  save({ ...settings, knownApps: settings.knownApps.filter((x) => x !== a) })
                }
              >
                Remove
              </button>
            </li>
          );
        })}
      </ul>
      <form
        className="actions"
        onSubmit={(e) => {
          e.preventDefault();
          if (draft.trim() === "" || problem) return;
          save({ ...settings, knownApps: withApp(settings.knownApps, draft) });
          setDraft("");
        }}
      >
        <label>
          Add a program
          <input
            value={draft}
            placeholder="for example webex"
            onChange={(e) => setDraft(e.target.value)}
          />
        </label>
        <button className="btn" type="submit" disabled={draft.trim() === "" || problem !== null}>
          Add
        </button>
      </form>
      {problem && <p role="alert">{problem}</p>}
      {message && <p role="alert">{message}</p>}
    </section>
  );
}
