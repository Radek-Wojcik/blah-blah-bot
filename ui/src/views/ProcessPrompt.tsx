import { useEffect, useRef, useState } from "react";
import { useBackend } from "../backend-context";
import { errorMessage, useRecordingState } from "../hooks";
import type { LlmKind } from "../ipc/types";
import { Modal } from "./Modal";
import {
  BackendChoice,
  CloudConfirmDialog,
  processOptions,
  useLlmSettings,
} from "./ProcessDialogs";

/**
 * The "Process now or later?" dialog shown after a recording stops, only while the setting
 * "after a recording stops" is "ask".
 *
 * Placement: the stop can come from the tray widget, a hotkey or (in the future) the main
 * window, and either window may be the only one that is open, so both windows render this
 * component and each watches the recording state itself. When both are open, the first
 * answer "now" closes the other dialog as soon as the job shows up in the queue; an answer
 * "later" in one window leaves the dialog of the other window until it is answered too.
 * Starting the same job twice is harmless because the backend creates a job only once.
 *
 * The prompt also asks which language model makes the draft (the local model unless the user
 * picks another). Choosing the cloud opens a confirmation of what would be sent; there is no
 * automatic fallback to it.
 */
export function ProcessPrompt() {
  const backend = useBackend();
  const recording = useRecordingState();
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pick, setPick] = useState<LlmKind | null>(null);
  const [cloud, setCloud] = useState(false);
  const llm = useLlmSettings(pending);
  const lastRecorded = useRef<string | null>(null);

  // A transition from recording to idle is the stop of that meeting.
  useEffect(() => {
    if (recording.status === "recording") {
      lastRecorded.current = recording.meetingId;
      return;
    }
    const id = lastRecorded.current;
    lastRecorded.current = null;
    if (id === null) return;
    let cancelled = false;
    void backend.getProcessingSettings().then((s) => {
      if (!cancelled && s.processAfterRecording === "ask") {
        setError(null);
        setPick(null);
        setCloud(false);
        setPending(id);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [backend, recording.status, recording.meetingId]);

  // Another window answered "now": the meeting has a job, nothing left to ask.
  useEffect(() => {
    if (pending === null) return;
    let off: (() => void) | undefined;
    let cancelled = false;
    void backend
      .onQueueChanged((q) => {
        if (q.jobs.some((j) => j.meetingId === pending)) setPending(null);
      })
      .then((u) => {
        if (cancelled) u();
        else off = u;
      });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [backend, pending]);

  if (pending === null) return null;

  const run = (options?: Parameters<typeof backend.processMeeting>[1]) => {
    setError(null);
    backend
      .processMeeting(pending, options)
      .then(() => {
        setCloud(false);
        setPending(null);
      })
      .catch((e) => {
        setCloud(false);
        setError(errorMessage(e));
      });
  };
  const kind = pick ?? llm?.defaultKind ?? "managed";
  const now = () => {
    if (kind === "cloud") setCloud(true);
    else run(llm ? processOptions(llm, kind) : undefined);
  };

  if (cloud) {
    // A just-recorded meeting has no transcript, so the dialog explains that the confirmation
    // comes later and offers to process locally up to the transcript.
    return (
      <CloudConfirmDialog
        meetingId={pending}
        onCancel={() => setCloud(false)}
        onConfirmed={() => run({ llm: { kind: "cloud" } })}
        onProcessLocally={() => run()}
      />
    );
  }

  return (
    <Modal title="Process now or later?" onClose={() => setPending(null)}>
      <p className="modal-text">
        The recording is saved. Processing creates the transcript and can use the GPU for a while.
      </p>
      {llm && <BackendChoice settings={llm} value={kind} onChange={setPick} />}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <div className="actions">
        <button className="btn btn-ghost" onClick={() => setPending(null)}>
          Later
        </button>
        <button className="btn btn-record" autoFocus onClick={now}>
          Process now
        </button>
      </div>
    </Modal>
  );
}
