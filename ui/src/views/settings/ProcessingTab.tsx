import { useCallback, useEffect, useMemo, useState } from "react";
import { useBackend } from "../../backend-context";
import { AudioRetentionHelp } from "../../components/AudioRetentionHelp";
import { BrowseButton } from "../../components/BrowseButton";
import { HelpPopover } from "../../components/HelpPopover";
import { errorMessage } from "../../hooks";
import type { ProcessingSettings } from "../../ipc/types";
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
  return validateVoiceSettings(s);
}

/**
 * Whether the settings keep the audio after processing. A `keepDays` value saved by an older
 * version counts as keeping it, which is what the backend does with it.
 */
function keepsAudio(s: ProcessingSettings): boolean {
  return s.audioRetention.mode !== "delete";
}

function isProcessingDirty(
  saved: ProcessingSettings | null,
  draft: ProcessingSettings | null,
): boolean {
  if (!saved || !draft) return false;
  return (
    saved.processAfterRecording !== draft.processAfterRecording ||
    keepsAudio(saved) !== keepsAudio(draft) ||
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

export interface ProcessingTabProps extends EditableTabProps {
  onRunSetup?: () => void;
}

/**
 * Processing & hardware policy tab: GPU guard thresholds, audio retention,
 * voice matching thresholds, and Python worker environment paths.
 */
export function ProcessingTab({ onDirtyChange, registerHandle, onRunSetup }: ProcessingTabProps) {
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
        audioRetention: { mode: keepsAudio(draft) ? "keep" : "delete" },
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
                  Audio after processing
                  <AudioRetentionHelp />
                </span>
              </legend>
              <label className="check">
                <input
                  type="checkbox"
                  aria-label="Keep audio after processing"
                  checked={keepsAudio(draft)}
                  onChange={(e) =>
                    set({ audioRetention: { mode: e.target.checked ? "keep" : "delete" } })
                  }
                />
                Keep audio after processing
              </label>
              <small>
                {keepsAudio(draft)
                  ? "The audio stays on this computer until you delete the meeting. A 10 hour meeting takes several GB."
                  : "The audio is deleted once the meeting is processed and its report is ready. The transcript, the report, key screenshots and voice sample clips are kept. A project can override this in its settings."}
              </small>
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
                  <span className="path-row">
                    <input
                      aria-label="Worker folder"
                      value={draft.workerDir}
                      onChange={(e) => set({ workerDir: e.target.value })}
                    />
                    <BrowseButton
                      target={{ folder: true }}
                      label="Browse for the worker folder"
                      title="Choose the worker folder"
                      onPicked={(path) => set({ workerDir: path })}
                    />
                  </span>
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
              {onRunSetup && (
                <button type="button" className="btn btn-ghost" onClick={onRunSetup}>
                  Run setup again
                </button>
              )}
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
