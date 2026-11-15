import { useCallback, useEffect, useState } from "react";
import { useBackend } from "../backend-context";
import { errorMessage } from "../hooks";
import type {
  LlmKind,
  LlmSettings,
  ModelStatus,
  ProcessingSettings as Settings,
  RetentionMode,
} from "../ipc/types";
import { KIND_LABELS, backendDetail, validateLlmSettings } from "../llm";
import { percent } from "../processing";
import { validateVoiceSettings } from "../speakers";
import { VoiceProfiles } from "./VoiceProfiles";

/** Settings of the processing pipeline: after-recording choice, retention, GPU guard, paths, token, models. */
export function ProcessingSettingsView() {
  const backend = useBackend();
  const [settings, setSettings] = useState<Settings | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [token, setToken] = useState("");
  const [tokenMessage, setTokenMessage] = useState<string | null>(null);

  const load = useCallback(() => {
    void backend.getProcessingSettings().then(setSettings);
  }, [backend]);
  useEffect(load, [load]);

  if (!settings) return null;
  const set = (patch: Partial<Settings>) => setSettings({ ...settings, ...patch });
  const num = (v: string) => (v === "" ? 0 : Number(v));
  const voiceError = validateVoiceSettings(settings);

  return (
    <section className="processing" aria-labelledby="processing-heading">
      <div className="page-head">
        <h2 id="processing-heading">
          <span className="glitch" data-text="Processing">
            Processing
          </span>
        </h2>
      </div>
      <div className="panel quiet" style={{ maxWidth: 620 }}>
        <div className="panel-body">
          <form
            className="form"
            onSubmit={(e) => {
              e.preventDefault();
              setMessage(null);
              backend
                .saveProcessingSettings(settings)
                .then(() => setMessage("Processing settings saved"))
                .catch((err) => setMessage(errorMessage(err)));
            }}
          >
            <label>
              After a recording stops
              <select
                value={settings.processAfterRecording}
                onChange={(e) =>
                  set({
                    processAfterRecording: e.target.value as Settings["processAfterRecording"],
                  })
                }
              >
                <option value="ask">Ask me</option>
                <option value="now">Process immediately</option>
                <option value="later">Do nothing, I will process manually</option>
              </select>
            </label>
            <label>
              Audio files after processing
              <select
                value={settings.audioRetention.mode}
                onChange={(e) =>
                  set({
                    audioRetention: {
                      ...settings.audioRetention,
                      mode: e.target.value as RetentionMode,
                    },
                  })
                }
              >
                <option value="keep">Keep</option>
                <option value="delete">Delete after processing</option>
                <option value="keepDays">Keep for a number of days</option>
              </select>
              {settings.audioRetention.mode !== "keep" && (
                <small>
                  Once the audio is deleted, corrections are only possible by editing the
                  transcript.
                </small>
              )}
            </label>
            {settings.audioRetention.mode === "keepDays" && (
              <label>
                Keep audio for (days)
                <input
                  type="number"
                  min={1}
                  value={settings.audioRetention.days}
                  onChange={(e) =>
                    set({
                      audioRetention: { ...settings.audioRetention, days: num(e.target.value) },
                    })
                  }
                />
              </label>
            )}
            <label>
              GPU guard: idle time before processing starts (seconds)
              <input
                type="number"
                min={0}
                value={settings.gpuIdleSeconds}
                onChange={(e) => set({ gpuIdleSeconds: num(e.target.value) })}
              />
            </label>
            <label>
              GPU guard: maximum utilization (%)
              <input
                type="number"
                min={0}
                max={100}
                value={settings.gpuMaxUtilPercent}
                onChange={(e) => set({ gpuMaxUtilPercent: num(e.target.value) })}
              />
              <small>Processing waits while another program keeps the GPU busier than this.</small>
            </label>
            <label>
              Voice recognition: name automatically from score
              <input
                type="number"
                min={0}
                max={1}
                step={0.01}
                value={settings.voiceAccept}
                onChange={(e) => set({ voiceAccept: num(e.target.value) })}
              />
              <small>
                A voice that matches a profile at least this well is named automatically. You still
                confirm it.
              </small>
            </label>
            <label>
              Voice recognition: suggest a name from score
              <input
                type="number"
                min={0}
                max={1}
                step={0.01}
                value={settings.voiceSuggest}
                onChange={(e) => set({ voiceSuggest: num(e.target.value) })}
              />
              <small>
                A weaker match is only offered as a suggestion. Cannot be higher than the automatic
                score.
              </small>
            </label>
            <label>
              Voice recognition: margin over the second best match
              <input
                type="number"
                min={0}
                step={0.01}
                value={settings.voiceMargin}
                onChange={(e) => set({ voiceMargin: num(e.target.value) })}
              />
              <small>
                When two people match almost equally, the voice is not named automatically.
              </small>
            </label>
            <label>
              Keep sample clips for (days)
              <input
                type="number"
                min={1}
                step={1}
                value={settings.clipMaxDays}
                onChange={(e) => set({ clipMaxDays: num(e.target.value) })}
              />
              <small>Clips of voices you have not named are deleted after this time.</small>
            </label>
            <label>
              uv executable
              <input value={settings.uvPath} onChange={(e) => set({ uvPath: e.target.value })} />
            </label>
            <label>
              Worker folder
              <input
                value={settings.workerDir}
                onChange={(e) => set({ workerDir: e.target.value })}
              />
            </label>
            <label>
              llama-server executable
              <input
                value={settings.llamaServerPath}
                onChange={(e) => set({ llamaServerPath: e.target.value })}
              />
            </label>
            <label>
              Models folder
              <input
                value={settings.modelsDir}
                onChange={(e) => set({ modelsDir: e.target.value })}
              />
            </label>
            {voiceError && (
              <p className="error" role="alert">
                {voiceError}
              </p>
            )}
            <div className="actions">
              {message && <span role="status">{message}</span>}
              <button className="btn btn-record" type="submit" disabled={voiceError !== null}>
                Save processing settings
              </button>
            </div>
          </form>
        </div>
      </div>

      {settings.llm && (
        <LanguageModelSettings
          llm={settings.llm}
          onChange={(llm) => set({ llm })}
          onSave={() => {
            setMessage(null);
            return backend
              .saveProcessingSettings(settings)
              .then(() => {
                load();
                return "Language model settings saved";
              })
              .catch((err) => errorMessage(err));
          }}
          onKeysChanged={() =>
            // Only the "saved" flags are taken over: unsaved edits of the form stay.
            void backend.getProcessingSettings().then((fresh) =>
              setSettings((cur) =>
                cur?.llm && fresh.llm
                  ? {
                      ...cur,
                      llm: {
                        ...cur.llm,
                        externalKeySet: fresh.llm.externalKeySet,
                        cloudKeySet: fresh.llm.cloudKeySet,
                      },
                    }
                  : cur,
              ),
            )
          }
        />
      )}

      <h3 className="sub-head">Hugging Face token</h3>
      <div className="panel quiet" style={{ maxWidth: 620 }}>
        <div className="panel-body">
          <form
            className="form"
            onSubmit={(e) => {
              e.preventDefault();
              setTokenMessage(null);
              backend
                .setHfToken(token)
                .then(() => {
                  setToken("");
                  load();
                })
                .catch((err) => setTokenMessage(errorMessage(err)));
            }}
          >
            <p className="token-state">
              {settings.hfTokenSet ? "Token stored" : "No token stored"}
            </p>
            <label>
              Token
              <input
                type="password"
                autoComplete="off"
                value={token}
                placeholder={settings.hfTokenSet ? "Enter a new token to replace it" : "hf_…"}
                onChange={(e) => setToken(e.target.value)}
              />
              <small>
                Needed for the gated speaker model. It is kept in the system credential store and
                never shown again.
              </small>
            </label>
            {tokenMessage && (
              <p className="error" role="alert">
                {tokenMessage}
              </p>
            )}
            <div className="actions">
              <button
                className="btn btn-ghost"
                type="button"
                disabled={!settings.hfTokenSet}
                onClick={() => {
                  setTokenMessage(null);
                  backend
                    .clearHfToken()
                    .then(load)
                    .catch((err) => setTokenMessage(errorMessage(err)));
                }}
              >
                Clear token
              </button>
              <button className="btn" type="submit" disabled={token.trim() === ""}>
                Store token
              </button>
            </div>
          </form>
        </div>
      </div>

      <VoiceProfiles />

      <ModelList />
    </section>
  );
}

function ModelList() {
  const backend = useBackend();
  const [models, setModels] = useState<ModelStatus[]>([]);
  const [progress, setProgress] = useState<Record<string, { fraction: number; message: string }>>(
    {},
  );
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    void backend.modelStatus().then(setModels);
  }, [backend]);

  useEffect(() => {
    load();
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
        load();
        setProgress({});
      }),
    );
    return () => {
      cancelled = true;
      offs.forEach((u) => u());
    };
  }, [backend, load]);

  const download = (key: string) => {
    setError(null);
    setProgress((cur) => ({ ...cur, [key]: { fraction: 0, message: "Starting" } }));
    backend.downloadModels([key]).catch((e) => {
      setError(errorMessage(e));
      setProgress((cur) => {
        const next = { ...cur };
        delete next[key];
        return next;
      });
    });
  };

  return (
    <>
      <h3 className="sub-head">Models</h3>
      {error && (
        <p className="error" role="alert">
          {error}
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
                    Needs a Hugging Face token and acceptance of the model terms on Hugging Face.
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
                    <span className="progress-fill" style={{ width: `${percent(p.fraction)}%` }} />
                  </div>
                  <small>{p.message}</small>
                </div>
              ) : (
                <button
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
    </>
  );
}

type KeyKind = "external" | "cloud";

/**
 * The language model that makes the draft: the managed local model, an external OpenAI-compatible
 * server and the cloud endpoint (chosen per meeting, never as the default). Keys are write-only:
 * only whether one is saved is shown.
 */
function LanguageModelSettings(props: {
  llm: LlmSettings;
  onChange: (llm: LlmSettings) => void;
  onSave: () => Promise<string>;
  onKeysChanged: () => void;
}) {
  const { llm, onChange, onKeysChanged } = props;
  const [message, setMessage] = useState<string | null>(null);
  const problem = validateLlmSettings(llm);
  const num = (v: string) => (v === "" ? 0 : Number(v));
  const kinds: Exclude<LlmKind, "cloud">[] = ["managed", "external"];
  return (
    <section aria-labelledby="llm-heading">
      <h3 id="llm-heading" className="sub-head">
        Language model
      </h3>
      <div className="panel quiet" style={{ maxWidth: 620 }}>
        <div className="panel-body">
          <form
            className="form"
            onSubmit={(e) => {
              e.preventDefault();
              setMessage(null);
              void props.onSave().then(setMessage);
            }}
          >
            <p className="notice">
              The language model turns the transcript into the draft report. Only the transcript
              text and screenshot descriptions are ever sent anywhere, and the cloud only after you
              confirm it for a meeting. There is no automatic fallback to it.
            </p>
            <label>
              Default model
              <select
                aria-label="Default language model"
                value={llm.defaultKind}
                onChange={(e) =>
                  onChange({ ...llm, defaultKind: e.target.value as Exclude<LlmKind, "cloud"> })
                }
              >
                {kinds.map((k) => (
                  <option key={k} value={k}>
                    {KIND_LABELS[k]} · {backendDetail(llm, k)}
                  </option>
                ))}
              </select>
              <small>Used when a meeting is processed without another choice.</small>
            </label>
            <fieldset className="llm-group">
              <legend>Local model (managed)</legend>
              <label>
                Model file (GGUF)
                <input
                  value={llm.managed.modelPath}
                  onChange={(e) =>
                    onChange({ ...llm, managed: { ...llm.managed, modelPath: e.target.value } })
                  }
                />
              </label>
              <label>
                Context size (2048 or more)
                <input
                  type="number"
                  min={2048}
                  step={1024}
                  value={llm.managed.ctx}
                  onChange={(e) =>
                    onChange({ ...llm, managed: { ...llm.managed, ctx: num(e.target.value) } })
                  }
                />
              </label>
            </fieldset>
            <fieldset className="llm-group">
              <legend>External server</legend>
              <label>
                Base address
                <input
                  value={llm.external.baseUrl}
                  placeholder="http://127.0.0.1:11434/v1"
                  onChange={(e) =>
                    onChange({ ...llm, external: { ...llm.external, baseUrl: e.target.value } })
                  }
                />
              </label>
              <label>
                Model name
                <input
                  value={llm.external.model}
                  onChange={(e) =>
                    onChange({ ...llm, external: { ...llm.external, model: e.target.value } })
                  }
                />
              </label>
            </fieldset>
            <fieldset className="llm-group">
              <legend>Cloud (chosen per meeting)</legend>
              <label>
                Base address
                <input
                  value={llm.cloud.baseUrl}
                  placeholder="https://api.example.com/v1"
                  onChange={(e) =>
                    onChange({ ...llm, cloud: { ...llm.cloud, baseUrl: e.target.value } })
                  }
                />
              </label>
              <label>
                Model name
                <input
                  value={llm.cloud.model}
                  onChange={(e) =>
                    onChange({ ...llm, cloud: { ...llm.cloud, model: e.target.value } })
                  }
                />
              </label>
            </fieldset>
            {problem && (
              <p className="error" role="alert">
                {problem}
              </p>
            )}
            <div className="actions">
              {message && <span role="status">{message}</span>}
              <button className="btn btn-record" type="submit" disabled={problem !== null}>
                Save language model settings
              </button>
            </div>
          </form>
        </div>
      </div>
      <ApiKey kind="external" saved={llm.externalKeySet} onChanged={onKeysChanged} />
      <ApiKey kind="cloud" saved={llm.cloudKeySet} onChanged={onKeysChanged} />
    </section>
  );
}

/** One write-only API key: a field to store a new one, a button to clear it, and a "saved" flag. */
function ApiKey(props: { kind: KeyKind; saved: boolean; onChanged: () => void }) {
  const { kind, saved, onChanged } = props;
  const backend = useBackend();
  const [key, setKey] = useState("");
  const [error, setError] = useState<string | null>(null);
  const label = kind === "cloud" ? "Cloud API key" : "External server key";
  const run = (p: Promise<void>) => {
    setError(null);
    p.then(() => {
      setKey("");
      onChanged();
    }).catch((e) => setError(errorMessage(e)));
  };
  return (
    <div className="panel quiet key-panel" style={{ maxWidth: 620 }}>
      <div className="panel-body">
        <form
          className="form"
          aria-label={label}
          onSubmit={(e) => {
            e.preventDefault();
            run(kind === "cloud" ? backend.setCloudKey(key) : backend.setExternalKey(key));
          }}
        >
          <p className="token-state">
            {label}: {saved ? "saved" : "not saved"}
          </p>
          <label>
            {label}
            <input
              type="password"
              autoComplete="off"
              value={key}
              placeholder={saved ? "Enter a new key to replace it" : "Key"}
              onChange={(e) => setKey(e.target.value)}
            />
            <small>It is kept in the system credential store and never shown again.</small>
          </label>
          {error && (
            <p className="error" role="alert">
              {error}
            </p>
          )}
          <div className="actions">
            <button
              className="btn btn-ghost"
              type="button"
              disabled={!saved}
              onClick={() =>
                run(kind === "cloud" ? backend.clearCloudKey() : backend.clearExternalKey())
              }
            >
              Clear {kind} key
            </button>
            <button className="btn" type="submit" disabled={key.trim() === ""}>
              Save {kind} key
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
