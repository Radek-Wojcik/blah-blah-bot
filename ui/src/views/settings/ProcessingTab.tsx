import { useCallback, useEffect, useMemo, useState } from "react";
import { useBackend } from "../../backend-context";
import { HelpPopover } from "../../components/HelpPopover";
import { errorMessage } from "../../hooks";
import type { ProcessingSettings, RetentionMode } from "../../ipc/types";
import { validateVoiceSettings } from "../../speakers";
import type { EditableTabProps } from "../Settings";

function validateProcessingPolicy(s: ProcessingSettings): string | null {
  if (
    !Number.isInteger(s.gpuMaxUtilPercent) ||
    s.gpuMaxUtilPercent < 1 ||
    s.gpuMaxUtilPercent > 100
  ) {
    return "Max GPU utilization must be a whole number between 1 and 100%.";
  }
  if (!Number.isInteger(s.gpuIdleSeconds) || s.gpuIdleSeconds < 0) {
    return "GPU idle wait time must be 0 seconds or more.";
  }
  if (
    s.audioRetention.mode === "keepDays" &&
    (!Number.isInteger(s.audioRetention.days) || s.audioRetention.days < 1)
  ) {
    return "Keep audio days must be a whole number, at least 1.";
  }
  return validateVoiceSettings(s);
}

function isProcessingDirty(
  saved: ProcessingSettings | null,
  draft: ProcessingSettings | null,
): boolean {
  if (!saved || !draft) return false;
  return (
    saved.processAfterRecording !== draft.processAfterRecording ||
    saved.audioRetention.mode !== draft.audioRetention.mode ||
    saved.audioRetention.days !== draft.audioRetention.days ||
    saved.gpuIdleSeconds !== draft.gpuIdleSeconds ||
    saved.gpuMaxUtilPercent !== draft.gpuMaxUtilPercent ||
    saved.voiceAccept !== draft.voiceAccept ||
    saved.voiceSuggest !== draft.voiceSuggest ||
    saved.voiceMargin !== draft.voiceMargin ||
    saved.clipMaxDays !== draft.clipMaxDays ||
    saved.uvPath !== draft.uvPath ||
    saved.workerDir !== draft.workerDir
  );
}

/**
 * Processing & hardware policy tab: GPU guard thresholds, audio retention,
 * voice matching thresholds, and Python worker environment paths.
 */
export function ProcessingTab({ onDirtyChange, registerHandle }: EditableTabProps) {
  const backend = useBackend();
  const [saved, setSaved] = useState<ProcessingSettings | null>(null);
  const [draft, setDraft] = useState<ProcessingSettings | null>(null);
  const [message, setMessage] = useState<{ text: string; kind: "status" | "error" } | null>(null);

  useEffect(() => {
    let cancelled = false;
    void backend
      .getProcessingSettings()
      .then((s) => {
        if (cancelled) return;
        setSaved(structuredClone(s));
        setDraft(structuredClone(s));
      })
      .catch((err) => {
        if (!cancelled) setMessage({ text: errorMessage(err), kind: "error" });
      });
    return () => {
      cancelled = true;
    };
  }, [backend]);

  const dirty = useMemo(() => isProcessingDirty(saved, draft), [saved, draft]);

  useEffect(() => {
    onDirtyChange(dirty);
  }, [dirty, onDirtyChange]);

  useEffect(() => () => onDirtyChange(false), [onDirtyChange]);

  const save = useCallback(async (): Promise<boolean> => {
    if (!draft) return false;
    const problem = validateProcessingPolicy(draft);
    if (problem) {
      setMessage({ text: problem, kind: "error" });
      return false;
    }
    setMessage(null);
    try {
      const latest = await backend.getProcessingSettings();
      const merged: ProcessingSettings = {
        ...latest,
        processAfterRecording: draft.processAfterRecording,
        audioRetention: draft.audioRetention,
        gpuIdleSeconds: draft.gpuIdleSeconds,
        gpuMaxUtilPercent: draft.gpuMaxUtilPercent,
        voiceAccept: draft.voiceAccept,
        voiceSuggest: draft.voiceSuggest,
        voiceMargin: draft.voiceMargin,
        clipMaxDays: draft.clipMaxDays,
        uvPath: draft.uvPath,
        workerDir: draft.workerDir,
      };
      await backend.saveProcessingSettings(merged);
      setSaved(structuredClone(merged));
      setDraft(structuredClone(merged));
      setMessage({ text: "Processing policy saved.", kind: "status" });
      return true;
    } catch (err) {
      setMessage({ text: errorMessage(err), kind: "error" });
      return false;
    }
  }, [backend, draft]);

  const discard = useCallback(() => {
    if (saved) setDraft(structuredClone(saved));
    setMessage(null);
  }, [saved]);

  useEffect(() => {
    registerHandle({ save, discard });
    return () => registerHandle(null);
  }, [registerHandle, save, discard]);

  if (!draft) return null;
  const set = (patch: Partial<ProcessingSettings>) => setDraft({ ...draft, ...patch });
  const num = (v: string) => (v === "" ? 0 : Number(v));
  const problem = validateProcessingPolicy(draft);

  return (
    <div className="settings-pane">
      <div className="panel quiet">
        <div className="panel-body">
          <h3 className="sub-head" style={{ marginTop: 0 }}>
            Processing &amp; Hardware Policy
          </h3>
          <form
            className="form"
            aria-label="Processing policy"
            onSubmit={(e) => {
              e.preventDefault();
              void save();
            }}
          >
            <label>
              After a recording stops
              <select
                value={draft.processAfterRecording}
                onChange={(e) =>
                  set({
                    processAfterRecording: e.target
                      .value as ProcessingSettings["processAfterRecording"],
                  })
                }
              >
                <option value="ask">Ask me</option>
                <option value="now">Process immediately</option>
                <option value="later">Do nothing, I will process manually</option>
              </select>
            </label>

            <fieldset className="llm-group">
              <legend>
                <span className="field-label-row">
                  GPU guard thresholds
                  <HelpPopover
                    title="GPU Guard (Gaming & Heavy Apps)"
                    recommended="Max GPU utilization: 25–40%, Wait for idle: 10–30 s"
                  >
                    <p>
                      Pauses background transcription and AI extraction whenever another app (like a
                      game or video editor) is using the GPU so your frame rate never drops.
                    </p>
                    <p>
                      If you game while recording, keep utilization around <strong>25–40%</strong>{" "}
                      and idle wait at <strong>10–30 s</strong>. Built-in VRAM safety budgets (3 GB
                      transcription, 3.5 GB diarization, 9.5 GB LLM/vision) also prevent
                      out-of-memory crashes automatically.
                    </p>
                  </HelpPopover>
                </span>
              </legend>
              <div className="settings-grid-2col">
                <label>
                  Max GPU utilization (%)
                  <input
                    type="number"
                    min={1}
                    max={100}
                    value={draft.gpuMaxUtilPercent}
                    onChange={(e) => set({ gpuMaxUtilPercent: num(e.target.value) })}
                  />
                </label>
                <label>
                  Wait for idle (seconds)
                  <input
                    type="number"
                    min={0}
                    value={draft.gpuIdleSeconds}
                    onChange={(e) => set({ gpuIdleSeconds: num(e.target.value) })}
                  />
                </label>
              </div>
              <small>
                Built-in VRAM guard reserves ~3,000 MB (ASR), ~3,500 MB (diarization), and ~9,500 MB
                (LLM/vision) free memory before starting a stage. You can override this temporarily
                per job in the Queue.
              </small>
            </fieldset>

            <fieldset className="llm-group">
              <legend>
                <span className="field-label-row">
                  Audio retention policy
                  <HelpPopover
                    title="Audio Retention"
                    recommended="Keep for 14–30 days if disk space is limited"
                  >
                    <p>
                      Controls what happens to raw meeting audio files after processing finishes.
                      Keeping audio allows you to re-run speaker recognition or re-transcribe later;
                      deleting frees disk space immediately.
                    </p>
                  </HelpPopover>
                </span>
              </legend>
              <label>
                Audio files after processing
                <select
                  value={draft.audioRetention.mode}
                  onChange={(e) =>
                    set({
                      audioRetention: {
                        ...draft.audioRetention,
                        mode: e.target.value as RetentionMode,
                      },
                    })
                  }
                >
                  <option value="keep">Keep</option>
                  <option value="delete">Delete after processing</option>
                  <option value="keepDays">Keep for N days</option>
                </select>
                {draft.audioRetention.mode !== "keep" && (
                  <small>
                    Once audio is deleted, corrections can only be made by editing the transcript.
                  </small>
                )}
              </label>
              {draft.audioRetention.mode === "keepDays" && (
                <label>
                  Keep audio for (days)
                  <input
                    type="number"
                    min={1}
                    value={draft.audioRetention.days}
                    onChange={(e) =>
                      set({
                        audioRetention: { ...draft.audioRetention, days: num(e.target.value) },
                      })
                    }
                  />
                </label>
              )}
            </fieldset>

            <fieldset className="llm-group">
              <legend>
                <span className="field-label-row">
                  Voice recognition thresholds
                  <HelpPopover
                    title="Voice Recognition Thresholds"
                    recommended="Accept: 0.75, Suggest: 0.60, Margin: 0.10"
                  >
                    <p>
                      Scores range from 0 to 1. Voices scoring at or above the automatic threshold
                      are pre-named for your confirmation; weaker matches are offered as
                      suggestions.
                    </p>
                  </HelpPopover>
                </span>
              </legend>
              <div className="settings-grid-2col">
                <label>
                  Name automatically from score
                  <input
                    type="number"
                    min={0}
                    max={1}
                    step={0.01}
                    value={draft.voiceAccept}
                    onChange={(e) => set({ voiceAccept: num(e.target.value) })}
                  />
                </label>
                <label>
                  Suggest a name from score
                  <input
                    type="number"
                    min={0}
                    max={1}
                    step={0.01}
                    value={draft.voiceSuggest}
                    onChange={(e) => set({ voiceSuggest: num(e.target.value) })}
                  />
                </label>
                <label>
                  Margin over second best match
                  <input
                    type="number"
                    min={0}
                    step={0.01}
                    value={draft.voiceMargin}
                    onChange={(e) => set({ voiceMargin: num(e.target.value) })}
                  />
                </label>
                <label>
                  Keep unconfirmed sample clips (days)
                  <input
                    type="number"
                    min={1}
                    step={1}
                    value={draft.clipMaxDays}
                    onChange={(e) => set({ clipMaxDays: num(e.target.value) })}
                  />
                </label>
              </div>
            </fieldset>

            <fieldset className="llm-group">
              <legend>
                <span className="field-label-row">
                  Worker Python environment
                  <HelpPopover title="Worker Environment">
                    <p>
                      The background transcription and diarization worker uses <code>uv</code> to
                      manage an isolated Python environment inside the Worker folder. It bootstraps
                      automatically when a job runs.
                    </p>
                  </HelpPopover>
                </span>
              </legend>
              <div className="settings-grid-2col">
                <label>
                  uv executable
                  <input value={draft.uvPath} onChange={(e) => set({ uvPath: e.target.value })} />
                </label>
                <label>
                  Worker folder
                  <input
                    value={draft.workerDir}
                    onChange={(e) => set({ workerDir: e.target.value })}
                  />
                </label>
              </div>
            </fieldset>

            {problem && (
              <p className="error" role="alert">
                {problem}
              </p>
            )}
            {message && (
              <p
                className={message.kind === "error" ? "error" : "secret-result"}
                role={message.kind === "error" ? "alert" : "status"}
              >
                {message.text}
              </p>
            )}
            <div className="actions">
              <button
                type="submit"
                className="btn btn-record"
                disabled={problem !== null || !dirty}
              >
                Save processing policy
              </button>
            </div>
          </form>
        </div>
      </div>
    </div>
  );
}
