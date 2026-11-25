import { useCallback, useEffect, useId, useMemo, useState } from "react";
import { useBackend } from "../../backend-context";
import { BrowseButton } from "../../components/BrowseButton";
import { HelpPopover } from "../../components/HelpPopover";
import { errorMessage } from "../../hooks";
import type { DetectedModels, LlmKind, ModelStatus, ProcessingSettings } from "../../ipc/types";
import { backendDetail, validateLlmSettings } from "../../llm";
import { percent } from "../../processing";
import type { EditableTabProps } from "../Settings";

function isModelsDirty(
  saved: ProcessingSettings | null,
  draft: ProcessingSettings | null,
): boolean {
  if (!saved || !draft) return false;
  if (saved.llamaServerPath !== draft.llamaServerPath) return true;
  if (saved.modelsDir !== draft.modelsDir) return true;
  if (
    (saved.visionModelPath ?? "") !== (draft.visionModelPath ?? "") ||
    Boolean(saved.skipScreenshotAnalysis) !== Boolean(draft.skipScreenshotAnalysis)
  ) {
    return true;
  }
  const a = saved.llm;
  const b = draft.llm;
  if (!a || !b) return a !== b;
  return (
    a.defaultKind !== b.defaultKind ||
    a.managed.modelPath !== b.managed.modelPath ||
    a.managed.ctx !== b.managed.ctx ||
    a.external.baseUrl !== b.external.baseUrl ||
    a.external.model !== b.external.model ||
    a.cloud.baseUrl !== b.cloud.baseUrl ||
    a.cloud.model !== b.cloud.model
  );
}

function normalizeModelSettings(s: ProcessingSettings): ProcessingSettings {
  return {
    ...s,
    visionModelPath: s.visionModelPath ?? "",
    skipScreenshotAnalysis: Boolean(s.skipScreenshotAnalysis),
  };
}

/**
 * AI Models settings tab: auto-detection of local binaries/GGUFs, default LLM backend,
 * vision model configuration, and model download/cache status.
 */
export function AiModelsTab({ onDirtyChange, registerHandle }: EditableTabProps) {
  const backend = useBackend();
  const [saved, setSaved] = useState<ProcessingSettings | null>(null);
  const [draft, setDraft] = useState<ProcessingSettings | null>(null);
  const [detected, setDetected] = useState<DetectedModels | null>(null);
  const [detecting, setDetecting] = useState(false);
  const [detectMessage, setDetectMessage] = useState<string | null>(null);
  const [message, setMessage] = useState<{ text: string; kind: "status" | "error" } | null>(null);

  const [models, setModels] = useState<ModelStatus[]>([]);
  const [progress, setProgress] = useState<Record<string, { fraction: number; message: string }>>(
    {},
  );
  const [downloadError, setDownloadError] = useState<string | null>(null);

  const defaultLlmId = useId();
  const ctxSizeId = useId();
  const visionPathId = useId();

  const loadSettings = useCallback(() => {
    void backend
      .getProcessingSettings()
      .then((raw) => {
        const s = normalizeModelSettings(raw);
        setSaved(structuredClone(s));
        setDraft(structuredClone(s));
      })
      .catch((err) => setMessage({ text: errorMessage(err), kind: "error" }));
  }, [backend]);

  const loadModelStatus = useCallback(() => {
    void backend.modelStatus().then(setModels);
  }, [backend]);

  useEffect(() => {
    loadSettings();
    loadModelStatus();
    let cancelled = false;
    const offs: Array<() => void> = [];
    const keep = (p: Promise<() => void>) =>
      void p.then((u) => {
        if (cancelled) u();
        else offs.push(u);
      });
    keep(
      backend.onModelsProgress((p) =>
        setProgress((cur) => ({ ...cur, [p.key]: { fraction: p.fraction, message: p.message } })),
      ),
    );
    keep(
      backend.onModelsChanged(() => {
        loadModelStatus();
        setProgress({});
      }),
    );
    return () => {
      cancelled = true;
      offs.forEach((u) => u());
    };
  }, [backend, loadSettings, loadModelStatus]);

  const dirty = useMemo(() => isModelsDirty(saved, draft), [saved, draft]);

  useEffect(() => {
    onDirtyChange(dirty);
  }, [dirty, onDirtyChange]);

  useEffect(() => () => onDirtyChange(false), [onDirtyChange]);

  const llmProblem = draft?.llm ? validateLlmSettings(draft.llm) : null;

  const save = useCallback(async (): Promise<boolean> => {
    if (!draft) return false;
    if (draft.llm) {
      const problem = validateLlmSettings(draft.llm);
      if (problem) {
        setMessage({ text: problem, kind: "error" });
        return false;
      }
    }
    setMessage(null);
    try {
      const latest = await backend.getProcessingSettings();
      const merged: ProcessingSettings = normalizeModelSettings({
        ...latest,
        llamaServerPath: draft.llamaServerPath,
        modelsDir: draft.modelsDir,
        visionModelPath: draft.visionModelPath ?? "",
        skipScreenshotAnalysis: Boolean(draft.skipScreenshotAnalysis),
        describeScreenshots: !draft.skipScreenshotAnalysis,
        llm: draft.llm
          ? {
              ...draft.llm,
              externalKeySet: latest.llm?.externalKeySet ?? draft.llm.externalKeySet,
              cloudKeySet: latest.llm?.cloudKeySet ?? draft.llm.cloudKeySet,
            }
          : latest.llm,
      });
      await backend.saveProcessingSettings(merged);
      setSaved(structuredClone(merged));
      setDraft(structuredClone(merged));
      setMessage({ text: "AI model settings saved.", kind: "status" });
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

  const handleAutoDetect = async () => {
    setDetecting(true);
    setDetectMessage(null);
    setMessage(null);
    try {
      const found = await backend.autoDetectModels();
      setDetected(found);
      setDraft((cur) => {
        if (!cur) return cur;
        const nextLlm = cur.llm
          ? {
              ...cur.llm,
              managed: {
                ...cur.llm.managed,
                modelPath: found.llmModels[0] ?? cur.llm.managed.modelPath,
              },
            }
          : cur.llm;
        return {
          ...cur,
          llamaServerPath: found.llamaServerPath ?? cur.llamaServerPath,
          visionModelPath: found.visionModels[0] ?? cur.visionModelPath ?? "",
          llm: nextLlm,
        };
      });
      const total = found.llmModels.length + found.visionModels.length;
      if (total > 0 || found.llamaServerPath) {
        setDetectMessage(
          `Found ${found.llmModels.length} language ${found.llmModels.length === 1 ? "model" : "models"} and ${found.visionModels.length} vision ${found.visionModels.length === 1 ? "model" : "models"} on disk.`,
        );
      } else {
        setDetectMessage("No local models or llama-server binary found in common folders.");
      }
    } catch (err) {
      setDetectMessage(errorMessage(err));
    } finally {
      setDetecting(false);
    }
  };

  const download = (key: string) => {
    setDownloadError(null);
    setProgress((cur) => ({ ...cur, [key]: { fraction: 0, message: "Starting" } }));
    backend.downloadModels([key]).catch((e) => {
      setDownloadError(errorMessage(e));
      setProgress((cur) => {
        const next = { ...cur };
        delete next[key];
        return next;
      });
    });
  };

  if (!draft) return null;
  const llm = draft.llm;
  const setLlm = (patch: Partial<NonNullable<ProcessingSettings["llm"]>>) =>
    llm && setDraft({ ...draft, llm: { ...llm, ...patch } });
  const num = (v: string) => (v === "" ? 0 : Number(v));

  return (
    <div className="settings-pane">
      <div className="panel quiet">
        <div className="panel-body">
          <div className="actions" style={{ justifyContent: "space-between", marginBottom: 12 }}>
            <div>
              <h3 className="sub-head" style={{ margin: 0 }}>
                AI Models
              </h3>
              <p className="person-meta" style={{ margin: "4px 0 0" }}>
                Configure which language and vision models turn transcripts and screenshots into
                notes.
              </p>
            </div>
            <button
              type="button"
              className="btn"
              disabled={detecting}
              onClick={() => void handleAutoDetect()}
            >
              {detecting ? "Scanning disk…" : "Auto-detect models on disk"}
            </button>
          </div>
          {detectMessage && (
            <p className="secret-result" role="status">
              {detectMessage}
            </p>
          )}

          <form
            className="form"
            aria-label="AI Models"
            onSubmit={(e) => {
              e.preventDefault();
              void save();
            }}
          >
            {llm && (
              <>
                <div className="field-block">
                  <div className="field-label-row">
                    <label htmlFor={defaultLlmId}>Default LLM backend</label>
                    <HelpPopover
                      title="Managed vs External vs Cloud"
                      recommended="Managed local server for full privacy and zero per-meeting cost"
                    >
                      <p>
                        <strong>Managed local server:</strong> Runs llama-server directly on this
                        computer using a local GGUF file. Nothing leaves your machine.
                      </p>
                      <p>
                        <strong>External local server:</strong> Connects to an OpenAI-compatible
                        server you already run (such as a local LAN server).
                      </p>
                      <p>
                        <strong>Cloud (per-meeting only):</strong> Never used automatically as a
                        default. You choose and confirm it per meeting when needed.
                      </p>
                    </HelpPopover>
                  </div>
                  <select
                    id={defaultLlmId}
                    aria-label="Default LLM backend"
                    value={llm.defaultKind}
                    onChange={(e) =>
                      setLlm({ defaultKind: e.target.value as Exclude<LlmKind, "cloud"> })
                    }
                  >
                    <option value="managed">
                      Managed local server · {backendDetail(llm, "managed")}
                    </option>
                    <option value="external">
                      External local server · {backendDetail(llm, "external")}
                    </option>
                    <option value="cloud" disabled>
                      Cloud per-meeting only (choose per meeting)
                    </option>
                  </select>
                </div>

                <fieldset className="llm-group">
                  <legend>Managed local server</legend>
                  <label>
                    llama-server executable
                    <input
                      value={draft.llamaServerPath}
                      placeholder="C:\Tools\llama\llama-server.exe"
                      onChange={(e) => setDraft({ ...draft, llamaServerPath: e.target.value })}
                    />
                  </label>
                  <label>
                    Default language model (GGUF)
                    <input
                      value={llm.managed.modelPath}
                      placeholder="D:\Models\gemma-4-12b-q4_k_m.gguf"
                      onChange={(e) =>
                        setLlm({ managed: { ...llm.managed, modelPath: e.target.value } })
                      }
                    />
                    {detected && detected.llmModels.length > 0 && (
                      <span className="person-actions" style={{ marginTop: 6 }}>
                        {detected.llmModels.map((path) => (
                          <button
                            key={path}
                            type="button"
                            className="btn btn-small btn-ghost"
                            onClick={() => setLlm({ managed: { ...llm.managed, modelPath: path } })}
                          >
                            Use {path.split(/[\\/]/).pop()}
                          </button>
                        ))}
                      </span>
                    )}
                  </label>
                  <div className="field-block">
                    <div className="field-label-row">
                      <label htmlFor={ctxSizeId}>Context size (tokens)</label>
                      <HelpPopover
                        title="Context size"
                        recommended="16384 tokens for typical 1-hour meetings"
                      >
                        <p>
                          Controls how much transcript text fits into one prompt window (minimum
                          2048). Larger values handle longer meetings in fewer passes but use more
                          VRAM.
                        </p>
                      </HelpPopover>
                    </div>
                    <input
                      id={ctxSizeId}
                      type="number"
                      min={2048}
                      step={1024}
                      value={llm.managed.ctx}
                      onChange={(e) =>
                        setLlm({ managed: { ...llm.managed, ctx: num(e.target.value) } })
                      }
                    />
                  </div>
                  <label>
                    Models folder
                    <span className="path-row">
                      <input
                        aria-label="Models folder"
                        value={draft.modelsDir}
                        placeholder="Documents\BlahBlahBot\models"
                        onChange={(e) => setDraft({ ...draft, modelsDir: e.target.value })}
                      />
                      <BrowseButton
                        target={{ folder: true }}
                        label="Browse for the models folder"
                        title="Choose the models folder"
                        onPicked={(path) => setDraft({ ...draft, modelsDir: path })}
                      />
                    </span>
                  </label>
                </fieldset>

                <fieldset className="llm-group">
                  <legend>External local server (OpenAI-compatible endpoint)</legend>
                  <label>
                    Endpoint URL
                    <input
                      value={llm.external.baseUrl}
                      placeholder="http://127.0.0.1:11434/v1"
                      onChange={(e) =>
                        setLlm({ external: { ...llm.external, baseUrl: e.target.value } })
                      }
                    />
                  </label>
                  <label>
                    Model name
                    <input
                      value={llm.external.model}
                      onChange={(e) =>
                        setLlm({ external: { ...llm.external, model: e.target.value } })
                      }
                    />
                  </label>
                  {llm.externalHostIsRemote && (
                    <p className="error" role="note">
                      This server is not on this machine or private network. Manage its API key in
                      the Connectors tab.
                    </p>
                  )}
                </fieldset>

                <fieldset className="llm-group">
                  <legend>Cloud endpoint (per-meeting only)</legend>
                  <p className="person-meta" style={{ margin: 0 }}>
                    Cloud endpoints are never selected automatically. Store your External / Cloud
                    LLM API keys in Connectors, and confirm per meeting before sending transcripts.
                  </p>
                  <label>
                    Cloud endpoint URL
                    <input
                      value={llm.cloud.baseUrl}
                      placeholder="https://api.example.com/v1"
                      onChange={(e) => setLlm({ cloud: { ...llm.cloud, baseUrl: e.target.value } })}
                    />
                  </label>
                  <label>
                    Cloud model name
                    <input
                      value={llm.cloud.model}
                      onChange={(e) => setLlm({ cloud: { ...llm.cloud, model: e.target.value } })}
                    />
                  </label>
                </fieldset>
              </>
            )}

            <fieldset className="llm-group">
              <legend>Vision & screenshots</legend>
              <div className="field-block">
                <div className="field-label-row">
                  <label htmlFor={visionPathId}>Vision model path (GGUF)</label>
                  <HelpPopover
                    title="Vision GGUF"
                    recommended="Use a 4B–8B quantized vision GGUF inside your Models folder"
                  >
                    <p>
                      Used to describe captured screenshots and slides so visual context is included
                      in your meeting notes. Click Auto-detect models on disk to locate local vision
                      GGUF files automatically.
                    </p>
                  </HelpPopover>
                </div>
                <input
                  id={visionPathId}
                  value={draft.visionModelPath ?? ""}
                  placeholder="D:\Models\Qwen3-VL-4B-Instruct-Q4_K_M.gguf"
                  disabled={Boolean(draft.skipScreenshotAnalysis)}
                  onChange={(e) => setDraft({ ...draft, visionModelPath: e.target.value })}
                />
                {detected && detected.visionModels.length > 0 && (
                  <span className="person-actions" style={{ marginTop: 6 }}>
                    {detected.visionModels.map((path) => (
                      <button
                        key={path}
                        type="button"
                        className="btn btn-small btn-ghost"
                        onClick={() =>
                          setDraft({
                            ...draft,
                            visionModelPath: path,
                            skipScreenshotAnalysis: false,
                          })
                        }
                      >
                        Use {path.split(/[\\/]/).pop()}
                      </button>
                    ))}
                  </span>
                )}
              </div>
              <label className="check">
                <input
                  type="checkbox"
                  checked={Boolean(draft.skipScreenshotAnalysis)}
                  onChange={(e) => setDraft({ ...draft, skipScreenshotAnalysis: e.target.checked })}
                />
                Skip screenshot analysis
              </label>
              <small>
                When enabled, captured screenshots are kept on disk without loading a vision model.
              </small>
            </fieldset>

            {llmProblem && (
              <p className="error" role="alert">
                {llmProblem}
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
                disabled={llmProblem !== null || !dirty}
              >
                Save AI model settings
              </button>
            </div>
          </form>
        </div>
      </div>

      <div className="panel quiet">
        <div className="panel-body">
          <div className="field-label-row" style={{ marginBottom: 8 }}>
            <h3 className="sub-head" style={{ margin: 0 }}>
              Model downloads & cache
            </h3>
            <HelpPopover
              title="Hugging Face gated models"
              recommended="Accept model terms on Hugging Face and store a Read token in Connectors"
            >
              <p>
                Speech recognition and vision models download openly. Gated speaker diarization
                weights require accepting the license on Hugging Face and saving a Hugging Face
                Token under Connectors.
              </p>
            </HelpPopover>
          </div>
          {downloadError && (
            <p className="error" role="alert">
              {downloadError}
            </p>
          )}
          <ul className="models" aria-label="Models">
            {models.map((m) => {
              const p = progress[m.key];
              return (
                <li key={m.key} className="model">
                  <div className="model-main">
                    <span className="model-name">{m.label}</span>
                    <span className="model-meta">
                      {m.sizeMb !== null && `${m.sizeMb} MB`}
                      {m.gated && " · gated"}
                    </span>
                    {m.gated && !m.cached && (
                      <small className="model-note">
                        Requires a Hugging Face Token in Connectors after accepting the model terms.
                      </small>
                    )}
                  </div>
                  {m.cached ? (
                    <span className="badge badge-recovered">Cached</span>
                  ) : p ? (
                    <div className="model-progress">
                      <div
                        className="progress"
                        role="progressbar"
                        aria-label={`Download of ${m.label}`}
                        aria-valuemin={0}
                        aria-valuemax={100}
                        aria-valuenow={percent(p.fraction)}
                      >
                        <span
                          className="progress-fill"
                          style={{ width: `${percent(p.fraction)}%` }}
                        />
                      </div>
                      <small>{p.message}</small>
                    </div>
                  ) : (
                    <button
                      type="button"
                      className="btn btn-small"
                      aria-label={`Download ${m.label}`}
                      onClick={() => download(m.key)}
                    >
                      Download
                    </button>
                  )}
                </li>
              );
            })}
          </ul>
        </div>
      </div>
    </div>
  );
}
