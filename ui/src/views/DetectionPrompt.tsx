import { useEffect, useState } from "react";
import { useBackend } from "../backend-context";
import { errorMessage } from "../hooks";
import type { DetectionAnswer, DetectionPrompt as Prompt } from "../ipc/types";

/**
 * "Record this call?" for a call the detector found. In the application it is the content of a
 * small always-on-top window that does not take the focus (`?view=detection`); the demo mock
 * shows it inside the recorder (`embedded`). It follows the `detection-prompt` and
 * `detection-cleared` events and asks for the prompt on offer once when it opens.
 */
export function DetectionPrompt(props: { embedded?: boolean }) {
  const backend = useBackend();
  const [prompt, setPrompt] = useState<Prompt | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let alive = true;
    void backend.detectionPrompt().then((p) => alive && p && setPrompt(p));
    const offPrompt = backend.onDetectionPrompt((p) => {
      setNote(null);
      setPrompt(p);
    });
    const offCleared = backend.onDetectionCleared((key) =>
      setPrompt((p) => (p && p.key === key ? null : p)),
    );
    return () => {
      alive = false;
      void offPrompt.then((f) => f());
      void offCleared.then((f) => f());
    };
  }, [backend]);

  if (!prompt) return null;
  const answer = (a: DetectionAnswer) => {
    setBusy(true);
    setNote(null);
    backend
      .answerDetection(prompt.key, a)
      .then((outcome) => {
        if (outcome.needsReport) setNote("Choose the cycle and report in the recorder window.");
        setPrompt(null);
      })
      .catch((e) => setNote(errorMessage(e)))
      .finally(() => setBusy(false));
  };

  const what = prompt.source === "meet" ? "A Google Meet call" : `A ${prompt.display} call`;
  return (
    <div
      className={props.embedded ? "detect detect-embedded" : "detect"}
      role="alertdialog"
      aria-label="Record this call?"
    >
      <div className="panel">
        <div className="panel-body">
          <p className="detect-title">Record this call?</p>
          <p className="detect-text">{what} seems to have started.</p>
          {note && <p role="alert">{note}</p>}
          <div className="actions">
            <button className="btn btn-record" disabled={busy} onClick={() => answer("record")}>
              Record
            </button>
            <button className="btn" disabled={busy} onClick={() => answer("not_now")}>
              Not now
            </button>
            <button className="btn" disabled={busy} onClick={() => answer("never")}>
              Never for {prompt.display}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
