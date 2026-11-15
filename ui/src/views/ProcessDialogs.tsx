import { useEffect, useState } from "react";
import { useBackend } from "../backend-context";
import { errorMessage } from "../hooks";
import type { CloudPreview, LlmKind, LlmSettings, ProcessOptions } from "../ipc/types";
import { KIND_LABELS, backendDetail, isConfigured } from "../llm";
import { Modal } from "./Modal";

/** The language model settings (the choice of a backend is made against them), read again when `refreshKey` changes. */
export function useLlmSettings(refreshKey: unknown = null): LlmSettings | null {
  const backend = useBackend();
  const [llm, setLlm] = useState<LlmSettings | null>(null);
  useEffect(() => {
    let cancelled = false;
    void backend.getProcessingSettings().then((s) => {
      if (!cancelled && s.llm) setLlm(s.llm);
    });
    return () => {
      cancelled = true;
    };
  }, [backend, refreshKey]);
  return llm;
}

const KINDS: LlmKind[] = ["managed", "external", "cloud"];

/**
 * The language model for one meeting: the local model (default), an external server or the
 * cloud. A backend that is not configured cannot be chosen, except the default one, which then
 * means "no draft" (the extraction stages are skipped).
 */
export function BackendChoice(props: {
  settings: LlmSettings;
  value: LlmKind;
  onChange: (kind: LlmKind) => void;
}) {
  const { settings, value, onChange } = props;
  const defaultKind = settings.defaultKind;
  const missingDefault = value === defaultKind && !isConfigured(settings, defaultKind);
  return (
    <fieldset className="backend-choice">
      <legend>Language model for the draft</legend>
      {KINDS.map((kind) => {
        const configured = isConfigured(settings, kind);
        return (
          <label key={kind} className="pick-row">
            <input
              type="radio"
              name="llm-backend"
              value={kind}
              checked={value === kind}
              disabled={!configured && kind !== defaultKind}
              onChange={() => onChange(kind)}
            />
            <span className="pick-label">
              {KIND_LABELS[kind]}
              {kind === defaultKind && " (default)"}
            </span>
            <span className="pick-kind">{backendDetail(settings, kind)}</span>
          </label>
        );
      })}
      {value === "cloud" && (
        <small className="backend-note">
          The cloud is used only after you confirm what is sent. There is no automatic fallback to
          it.
        </small>
      )}
      {missingDefault && (
        <small className="backend-note">
          No model is configured, so no draft is made. Set one in Settings, Language model.
        </small>
      )}
    </fieldset>
  );
}

/** The options of `processMeeting`: the default backend is the absence of a choice. */
export function processOptions(settings: LlmSettings, kind: LlmKind): ProcessOptions | undefined {
  return kind === settings.defaultKind ? undefined : { llm: { kind } };
}

/**
 * What the cloud would receive, and the button that confirms it. The confirmation is only
 * stored by that button; before the transcript exists the amount is unknown, so the dialog
 * explains that and offers to process locally first.
 */
export function CloudConfirmDialog(props: {
  meetingId: string;
  onCancel: () => void;
  /** Called after the confirmation was stored. */
  onConfirmed: () => void;
  /** Without a transcript: process with the default backend up to the transcript. */
  onProcessLocally?: () => void;
}) {
  const { meetingId, onCancel, onConfirmed, onProcessLocally } = props;
  const backend = useBackend();
  const [preview, setPreview] = useState<CloudPreview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    backend
      .cloudPreview(meetingId)
      .then((p) => !cancelled && setPreview(p))
      .catch((e) => !cancelled && setError(errorMessage(e)));
    return () => {
      cancelled = true;
    };
  }, [backend, meetingId]);

  const send = () => {
    setBusy(true);
    setError(null);
    backend
      .confirmCloud(meetingId)
      .then(onConfirmed)
      .catch((e) => {
        setError(errorMessage(e));
        setBusy(false);
      });
  };

  return (
    <Modal title="Send this meeting to the cloud?" onClose={onCancel} wide>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {preview && !preview.transcriptReady && (
        <>
          <p className="modal-text">
            The meeting has no transcript yet, so the amount of text that would be sent is not
            known. The confirmation happens after transcription, when you can see exactly what will
            be sent to {preview.endpointHost}. Nothing is sent now.
          </p>
          <div className="actions">
            <button className="btn btn-ghost" autoFocus onClick={onCancel}>
              Cancel
            </button>
            {onProcessLocally && (
              <button className="btn btn-record" onClick={onProcessLocally}>
                Process locally up to the transcript
              </button>
            )}
          </div>
        </>
      )}
      {preview && preview.transcriptReady && (
        <>
          <dl className="facts">
            <div>
              <dt>Endpoint host</dt>
              <dd>{preview.endpointHost}</dd>
            </div>
            <div>
              <dt>Model</dt>
              <dd>{preview.model}</dd>
            </div>
            <div>
              <dt>Words of text</dt>
              <dd>{preview.wordCount}</dd>
            </div>
            <div>
              <dt>Screenshot descriptions</dt>
              <dd>{preview.screenshotDescriptions}</dd>
            </div>
            <div>
              <dt>Items of earlier meetings</dt>
              <dd>{preview.previousItemCount}</dd>
            </div>
          </dl>
          <p className="modal-text">{preview.summaryText}</p>
          <p className="modal-text">
            The items of earlier meetings of the cycle (their text, assignees and due dates) are
            sent too, also those of meetings you processed only on this computer.
          </p>
          <p className="modal-text modal-strong">No audio and no images are sent.</p>
          <div className="actions">
            <button className="btn btn-ghost" autoFocus onClick={onCancel}>
              Cancel
            </button>
            <button className="btn btn-record" disabled={busy} onClick={send}>
              Send text to {preview.endpointHost}
            </button>
          </div>
        </>
      )}
      {!preview && error && (
        <div className="actions">
          <button className="btn btn-ghost" autoFocus onClick={onCancel}>
            Close
          </button>
        </div>
      )}
    </Modal>
  );
}

/**
 * Asks which language model builds the draft of a meeting that has none (its extraction was
 * skipped or the meeting was processed before cycles), then runs the extraction stages.
 */
export function BuildDraftDialog(props: {
  meetingId: string;
  onClose: () => void;
  onStarted: () => void;
}) {
  const { meetingId, onClose, onStarted } = props;
  const backend = useBackend();
  const settings = useLlmSettings();
  const [pick, setPick] = useState<LlmKind | null>(null);
  const [cloud, setCloud] = useState(false);
  const [error, setError] = useState<string | null>(null);
  if (!settings) return null;
  const kind = pick ?? settings.defaultKind;

  const run = (k: LlmKind) => {
    setError(null);
    backend
      .processMeeting(meetingId, { llm: { kind: k } })
      .then(onStarted)
      .catch((e) => setError(errorMessage(e)));
  };

  if (cloud) {
    return (
      <CloudConfirmDialog
        meetingId={meetingId}
        onCancel={() => setCloud(false)}
        onConfirmed={() => run("cloud")}
      />
    );
  }
  return (
    <Modal title="Build the draft" onClose={onClose}>
      <p className="modal-text">
        The transcript is ready. The draft report is made from it by a language model.
      </p>
      <BackendChoice settings={settings} value={kind} onChange={setPick} />
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <div className="actions">
        <button className="btn btn-ghost" onClick={onClose}>
          Cancel
        </button>
        <button
          className="btn btn-record"
          disabled={!isConfigured(settings, kind)}
          onClick={() => (kind === "cloud" ? setCloud(true) : run(kind))}
        >
          Build draft
        </button>
      </div>
    </Modal>
  );
}
