import { useCallback, useEffect, useId, useRef, useState } from "react";
import { useBackend } from "../backend-context";
import { newCycleError } from "../cycles";
import {
  STATE_TEXT,
  STEPS,
  type StepId,
  downloadFailure,
  fileName,
  finishState,
  formatBytes,
  isProblem,
  itemLabel,
  itemOf,
  parentDir,
  pathProblem,
  stepForItem,
  stepItems,
  stepState,
} from "../first-run";
import { errorMessage, useCycles } from "../hooks";
import type { Backend } from "../ipc/backend";
import type {
  GgufFile,
  ModelStatus,
  ProcessingSettings,
  SetupItem,
  SetupStatus,
} from "../ipc/types";
import { percent } from "../processing";
import "./first-run.css";

/** The URL of the page where the terms of the gated speaker model are accepted. */
const DIARIZATION_TERMS = "huggingface.co/pyannote/speaker-diarization-community-1";

interface Progress {
  fraction: number;
  message: string;
}

/** What the steps share: the readiness report, the settings and the running jobs. */
interface Ctx {
  backend: Backend;
  status: SetupStatus | null;
  settings: ProcessingSettings | null;
  models: ModelStatus[];
  reload: () => Promise<void>;
  /** Reads the settings again, applies `patch`, saves them and reloads the report. */
  change: (patch: (s: ProcessingSettings) => ProcessingSettings) => Promise<void>;
  /** Runs an action; its error is shown at the foot of the step. */
  act: (p: Promise<unknown>) => Promise<void>;
  downloads: Downloads;
  env: EnvRun;
  go: (step: StepId) => void;
  onOpenSettings?: () => void;
}

interface Downloads {
  progress: Record<string, Progress>;
  failed: Record<string, string>;
  start: (key: string) => void;
}

interface EnvRun {
  running: boolean;
  lines: string[];
  fraction: number | null;
  error: string | null;
  start: () => void;
}

function useDownloads(backend: Backend, reload: () => Promise<void>): Downloads {
  const [progress, setProgress] = useState<Record<string, Progress>>({});
  const [failed, setFailed] = useState<Record<string, string>>({});
  useEffect(() => {
    let cancelled = false;
    const offs: Array<() => void> = [];
    const keep = (p: Promise<() => void>) =>
      void p.then((u) => {
        if (cancelled) u();
        else offs.push(u);
      });
    keep(
      backend.onModelsProgress((p) => {
        const failure = downloadFailure(p.message);
        if (failure !== null) {
          setFailed((cur) => ({ ...cur, [p.key]: failure }));
          setProgress((cur) => omit(cur, p.key));
        } else {
          setProgress((cur) => ({ ...cur, [p.key]: { fraction: p.fraction, message: p.message } }));
        }
      }),
    );
    keep(
      backend.onModelsChanged(() => {
        setProgress({});
        void reload();
      }),
    );
    return () => {
      cancelled = true;
      offs.forEach((u) => u());
    };
  }, [backend, reload]);

  const start = (key: string) => {
    setFailed((cur) => omit(cur, key));
    setProgress((cur) => ({ ...cur, [key]: { fraction: 0, message: "Starting" } }));
    backend.downloadModels([key]).catch((e) => {
      setFailed((cur) => ({ ...cur, [key]: errorMessage(e) }));
      setProgress((cur) => omit(cur, key));
    });
  };
  return { progress, failed, start };
}

function omit<T>(record: Record<string, T>, key: string): Record<string, T> {
  const next = { ...record };
  delete next[key];
  return next;
}

function useEnvRun(backend: Backend, reload: () => Promise<void>): EnvRun {
  const [running, setRunning] = useState(false);
  const [lines, setLines] = useState<string[]>([]);
  const [fraction, setFraction] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    let off: (() => void) | undefined;
    void backend
      .onSetupProgress((p) => {
        if (p.key !== "workerEnv") return;
        if (p.line) setLines((cur) => [...cur.slice(-5), p.line]);
        setFraction(p.fraction);
        if (p.done) {
          setRunning(false);
          setError(p.ok ? null : (p.error ?? "Preparing the environment failed."));
          void reload();
        }
      })
      .then((u) => {
        if (cancelled) u();
        else off = u;
      });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [backend, reload]);

  const start = () => {
    setError(null);
    setLines([]);
    setFraction(null);
    setRunning(true);
    backend.prepareWorkerEnv().catch((e) => {
      setRunning(false);
      setError(errorMessage(e));
    });
  };
  return { running, lines, fraction, error, start };
}

/**
 * The first-run wizard: it walks through everything the application needs before it can process
 * a meeting. Nothing in it blocks the user: every step can be skipped, and what is not set up
 * is listed on the summary and explained again when a meeting is processed.
 *
 * Shown full-screen on the first start and again from Settings. `initialStep` and `reason` open
 * it at the step that a refused processing points to.
 */
export function FirstRun(props: {
  onClose: () => void;
  initialStep?: StepId;
  reason?: string | null;
  onOpenSettings?: () => void;
}) {
  const { onClose, initialStep, reason, onOpenSettings } = props;
  const backend = useBackend();
  const [step, setStep] = useState<StepId>(initialStep ?? "welcome");
  const [status, setStatus] = useState<SetupStatus | null>(null);
  const [settings, setSettings] = useState<ProcessingSettings | null>(null);
  const [models, setModels] = useState<ModelStatus[]>([]);
  const [error, setError] = useState<string | null>(null);
  const heading = useRef<HTMLHeadingElement>(null);

  const reload = useCallback(
    () =>
      Promise.all([
        backend.setupStatus(),
        backend.getProcessingSettings(),
        backend.modelStatus(),
      ]).then(([st, se, mo]) => {
        setStatus(st);
        setSettings(se);
        setModels(mo);
      }),
    [backend],
  );
  useEffect(() => {
    let cancelled = false;
    void Promise.all([
      backend.setupStatus(),
      backend.getProcessingSettings(),
      backend.modelStatus(),
    ]).then(([st, se, mo]) => {
      if (cancelled) return;
      setStatus(st);
      setSettings(se);
      setModels(mo);
    });
    return () => {
      cancelled = true;
    };
  }, [backend]);

  const downloads = useDownloads(backend, reload);
  const env = useEnvRun(backend, reload);

  // A new step starts at its heading, so keyboard and screen reader users notice the change.
  useEffect(() => {
    heading.current?.focus();
  }, [step]);

  const act = useCallback((p: Promise<unknown>) => {
    setError(null);
    return p.then(
      () => undefined,
      (e) => setError(errorMessage(e)),
    );
  }, []);
  const change = useCallback(
    (patch: (s: ProcessingSettings) => ProcessingSettings) =>
      backend
        .getProcessingSettings()
        .then((fresh) => backend.saveProcessingSettings(patch(fresh)))
        .then(reload),
    [backend, reload],
  );
  const go = (next: StepId) => {
    setError(null);
    setStep(next);
  };
  const finish = () => {
    void act(backend.setFirstRunDone(true).then(onClose));
  };

  const ctx: Ctx = {
    backend,
    status,
    settings,
    models,
    reload,
    change,
    act,
    downloads,
    env,
    go,
    onOpenSettings: onOpenSettings
      ? () => void backend.setFirstRunDone(true).then(onOpenSettings)
      : undefined,
  };
  const at = STEPS.findIndex((s) => s.id === step);
  const title = STEPS[at].title;
  const last = step === "cycle";
  const summary = step === "summary" ? finishState(status) : null;

  return (
    <div className="setup">
      <nav className="panel setup-nav" aria-label="Setup steps">
        <div className="panel-body">
          <h1 className="brand glitch" data-text="BlahBlahBot">
            Blah<span>Blah</span>Bot
          </h1>
          <ol className="setup-steps">
            {STEPS.map((s, n) => {
              const state = stepState(status, s.id);
              return (
                <li key={s.id}>
                  <button
                    type="button"
                    className={s.id === step ? "nav nav-on" : "nav"}
                    aria-current={s.id === step ? "step" : undefined}
                    onClick={() => go(s.id)}
                  >
                    <span className="setup-num">{n + 1}</span>
                    {s.title}
                    {state === "ok" && <span className="visually-hidden"> (done)</span>}
                    {state === "todo" && <span className="visually-hidden"> (not done)</span>}
                  </button>
                  {state !== "none" && <span className={`setup-dot setup-dot-${state}`} />}
                </li>
              );
            })}
          </ol>
        </div>
      </nav>
      <section className="setup-body" aria-labelledby="setup-title">
        <div className="panel quiet">
          <div className="panel-body setup-panel">
            <p className="setup-kicker">
              First-run setup · step {at + 1} of {STEPS.length}
            </p>
            <h2 id="setup-title" ref={heading} tabIndex={-1} className="setup-title">
              {title}
            </h2>
            {reason && (
              <p className="notice setup-reason" role="alert">
                Processing could not start: {reason}
              </p>
            )}
            {step === "welcome" && <WelcomeStep />}
            {step === "env" && <EnvStep ctx={ctx} />}
            {step === "models" && <ModelsStep ctx={ctx} />}
            {step === "llama" && <LlamaStep ctx={ctx} />}
            {step === "llm" && <LlmStep ctx={ctx} />}
            {step === "vision" && <VisionStep ctx={ctx} />}
            {step === "summary" && <SummaryStep ctx={ctx} />}
            {step === "cycle" && <CycleStep ctx={ctx} onFinish={finish} />}
            {error && (
              <p className="error" role="alert">
                {error}
              </p>
            )}
            {summary?.note && <p className="notice">{summary.note}</p>}
            <div className="actions setup-actions">
              <button type="button" className="btn btn-ghost btn-small" onClick={finish}>
                Leave setup
              </button>
              <button
                type="button"
                className="btn"
                disabled={at === 0}
                onClick={() => go(STEPS[at - 1].id)}
              >
                Back
              </button>
              {!last && (
                <button
                  type="button"
                  className="btn btn-record"
                  onClick={() => go(STEPS[at + 1].id)}
                >
                  {summary ? summary.label : "Next"}
                </button>
              )}
            </div>
          </div>
        </div>
      </section>
    </div>
  );
}

/** What an item says about itself: its message, and the fix when something is missing. */
function ItemNote({ item }: { item: SetupItem | undefined }) {
  if (!item) return null;
  const problem = isProblem(item);
  return (
    <p className={problem ? "setup-note setup-note-problem" : "setup-note"} role="status">
      <span className={`badge ${badgeClass(item.state)}`}>{STATE_TEXT[item.state]}</span>{" "}
      {item.message}
      {problem && item.fixHint ? ` ${item.fixHint}` : ""}
    </p>
  );
}

function badgeClass(state: SetupItem["state"]): string {
  if (state === "ok") return "badge-job-done";
  if (state === "skipped") return "badge-recovered";
  if (state === "working") return "badge-job-running";
  return "badge-job-failed";
}

function ProgressBar(props: { label: string; fraction: number | null }) {
  const { label, fraction } = props;
  return (
    <div
      className="progress"
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={fraction === null ? undefined : percent(fraction)}
    >
      <span
        className={fraction === null ? "progress-fill progress-busy" : "progress-fill"}
        style={{ width: fraction === null ? "40%" : `${percent(fraction)}%` }}
      />
    </div>
  );
}

function WelcomeStep() {
  return (
    <>
      <p>
        BlahBlahBot turns recordings of your meetings into transcripts and draft reports. Everything
        runs on this computer: the audio, the transcript and the report do not leave it unless you
        choose a cloud language model yourself.
      </p>
      <p>
        This setup prepares what processing needs: a processing environment, the speech models, a
        language model for reports and, if you want it, screenshot analysis. You can skip any part
        and come back later from Settings.
      </p>
      <p className="notice">
        Disk space: the environment and the models are large. Plan for tens of gigabytes of free
        space, and more for the language model file you choose. Processing uses the GPU for a while.
      </p>
    </>
  );
}

function EnvStep({ ctx }: { ctx: Ctx }) {
  const { status, env } = ctx;
  const item = itemOf(status, "workerEnv");
  const busy = env.running || item?.state === "working";
  return (
    <>
      <p>
        Transcription runs in a separate Python environment. Preparing it downloads the needed
        packages once, which can take several minutes.
      </p>
      <ItemNote item={item} />
      {busy && (
        <div className="setup-run">
          <ProgressBar label="Preparing the environment" fraction={env.fraction} />
          <ul className="setup-log" aria-label="Progress" aria-live="polite">
            {env.lines.map((l, n) => (
              <li key={n}>{l}</li>
            ))}
          </ul>
        </div>
      )}
      {env.error && (
        <p className="error" role="alert">
          {env.error} Check the network connection and try again.
        </p>
      )}
      <div className="actions">
        <button
          type="button"
          className="btn"
          disabled={busy || item?.state === "ok"}
          onClick={env.start}
        >
          {env.error ? "Retry" : "Prepare environment"}
        </button>
      </div>
    </>
  );
}

function ModelRow(props: { ctx: Ctx; modelKey: string; blockedBy?: string }) {
  const { ctx, modelKey, blockedBy } = props;
  const model = ctx.models.find((m) => m.key === modelKey);
  const progress = ctx.downloads.progress[modelKey];
  const failure = ctx.downloads.failed[modelKey];
  const anyBusy = Object.keys(ctx.downloads.progress).length > 0;
  const hintId = useId();
  if (!model) return null;
  return (
    <li className="model">
      <div className="model-main">
        <span className="model-name">{model.label}</span>
        <span className="model-meta">{model.sizeMb !== null && `${model.sizeMb} MB`}</span>
        {blockedBy && !model.cached && (
          <small id={hintId} className="model-note">
            {blockedBy}
          </small>
        )}
        {failure && (
          <small className="error" role="alert">
            Download failed: {failure}
          </small>
        )}
      </div>
      {model.cached ? (
        <span className="badge badge-recovered">Downloaded</span>
      ) : progress ? (
        <div className="model-progress">
          <ProgressBar label={`Download of ${model.label}`} fraction={progress.fraction} />
          <small>{progress.message}</small>
        </div>
      ) : (
        <button
          type="button"
          className="btn btn-small"
          aria-label={`${failure ? "Retry download of" : "Download"} ${model.label}`}
          aria-describedby={blockedBy && !model.cached ? hintId : undefined}
          disabled={anyBusy || !!blockedBy}
          onClick={() => ctx.downloads.start(modelKey)}
        >
          {failure ? "Retry" : "Download"}
        </button>
      )}
    </li>
  );
}

function ModelsStep({ ctx }: { ctx: Ctx }) {
  const { backend, status, settings, act, reload } = ctx;
  const [token, setToken] = useState("");
  const tokenSet = settings?.hfTokenSet ?? false;
  const store = () => {
    const value = token;
    setToken("");
    void act(backend.setHfToken(value).then(reload));
  };
  return (
    <>
      <p>Speech recognition turns the audio into text. The speaker model tells the voices apart.</p>
      <ul className="models" aria-label="Speech models">
        <ModelRow ctx={ctx} modelKey="whisper-turbo" />
        <ModelRow
          ctx={ctx}
          modelKey="diarization"
          blockedBy={tokenSet ? undefined : "Store a Hugging Face token below first."}
        />
      </ul>
      {stepItems(status, "models")
        .filter(isProblem)
        .map((i) => (
          <ItemNote key={i.key} item={i} />
        ))}
      <form
        className="form setup-token"
        onSubmit={(e) => {
          e.preventDefault();
          if (token.trim() !== "") store();
        }}
      >
        <h3 className="sub-head">Hugging Face token</h3>
        <p className="setup-help">
          The speaker model is gated. Create a free access token on huggingface.co (Settings, Access
          Tokens), and accept the terms of the model at {DIARIZATION_TERMS}. The token is kept in
          the system credential store and is never shown again.
        </p>
        <p className="token-state">{tokenSet ? "A token is stored" : "No token stored"}</p>
        <label>
          Hugging Face token
          <input
            type="password"
            autoComplete="off"
            spellCheck={false}
            value={token}
            placeholder={tokenSet ? "Enter a new token to replace it" : "hf_…"}
            onChange={(e) => setToken(e.target.value)}
          />
        </label>
        <div className="actions">
          {tokenSet && (
            <button
              type="button"
              className="btn btn-ghost btn-small"
              onClick={() => void act(backend.clearHfToken().then(reload))}
            >
              Clear token
            </button>
          )}
          <button type="submit" className="btn btn-small" disabled={token.trim() === ""}>
            Store token
          </button>
        </div>
      </form>
    </>
  );
}

function LlamaStep({ ctx }: { ctx: Ctx }) {
  const { backend, status, settings, change, act } = ctx;
  const item = itemOf(status, "llamaServer");
  const [found, setFound] = useState<string[] | null>(null);
  const candidates = found ?? status?.llamaCandidates ?? [];
  const [path, setPath] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const current = settings?.llamaServerPath ?? "";

  const use = (p: string) => {
    setProblem(null);
    void act(
      backend.checkSetupPath("llamaServer", p).then((check) => {
        if (!check.ok) {
          setProblem(pathProblem(check));
          return;
        }
        return change((s) => ({ ...s, llamaServerPath: p.trim() }));
      }),
    );
  };

  return (
    <>
      <p>
        Reports and screenshot descriptions are made by a language model that runs through
        llama-server, a small program that comes with llama.cpp.
      </p>
      <ItemNote item={item} />
      {current !== "" && (
        <p className="setup-help">
          In use: <code>{current}</code>
        </p>
      )}
      <fieldset className="backend-choice">
        <legend>llama-server found on this computer</legend>
        {candidates.length === 0 ? (
          <p className="empty">
            None found. Install llama.cpp, then press Detect again, or enter the full path of
            llama-server.exe below.
          </p>
        ) : (
          candidates.map((c) => (
            <label key={c} className="pick-row">
              <input
                type="radio"
                name="llama-candidate"
                checked={path === c}
                onChange={() => {
                  setPath(c);
                  setProblem(null);
                }}
              />
              <span className="pick-label">{c}</span>
            </label>
          ))
        )}
      </fieldset>
      <div className="actions setup-inline">
        <button
          type="button"
          className="btn btn-small"
          onClick={() => void act(backend.detectLlamaServer().then(setFound))}
        >
          Detect again
        </button>
      </div>
      <form
        className="form"
        onSubmit={(e) => {
          e.preventDefault();
          use(path);
        }}
      >
        <label>
          Path of llama-server.exe
          <input
            value={path}
            spellCheck={false}
            aria-invalid={problem !== null}
            onChange={(e) => {
              setPath(e.target.value);
              setProblem(null);
            }}
          />
        </label>
        {problem && (
          <p className="error" role="alert">
            {problem}
          </p>
        )}
        <div className="actions">
          <button type="submit" className="btn btn-small" disabled={path.trim() === ""}>
            Use this llama-server
          </button>
        </div>
      </form>
      <p className="setup-help">
        llama-server is not needed when you skip both reports and screenshot analysis.
      </p>
    </>
  );
}

function LlmStep({ ctx }: { ctx: Ctx }) {
  const { backend, status, settings, change, act, onOpenSettings } = ctx;
  const item = itemOf(status, "llmModel");
  const [files, setFiles] = useState<GgufFile[] | null>(null);
  const [dir, setDir] = useState("");
  const [path, setPath] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const current = settings?.llm?.managed.modelPath ?? "";
  const skipped = settings?.skipReports ?? false;
  const models = (files ?? []).filter((f) => f.kind === "llm");

  const scan = () =>
    void act(
      backend.findGgufModels(dir.trim() === "" ? undefined : dir.trim()).then((list) => {
        setFiles(list);
      }),
    );
  const use = () => {
    setProblem(null);
    void act(
      backend.checkSetupPath("llmModel", path).then((check) => {
        if (!check.ok) {
          setProblem(pathProblem(check));
          return;
        }
        return change((s) => ({
          ...s,
          skipReports: false,
          llm: s.llm && {
            ...s.llm,
            defaultKind: "managed",
            managed: { ...s.llm.managed, modelPath: path.trim() },
          },
        }));
      }),
    );
  };

  return (
    <>
      <p>
        The language model writes the report from the transcript. Choose a model file in the GGUF
        format on this computer. It is not downloaded for you.
      </p>
      <ItemNote item={item} />
      {current !== "" && (
        <p className="setup-help">
          In use: <code>{current}</code>
        </p>
      )}
      <form
        className="form"
        onSubmit={(e) => {
          e.preventDefault();
          scan();
        }}
      >
        <label>
          Folder to scan (optional)
          <input value={dir} spellCheck={false} onChange={(e) => setDir(e.target.value)} />
          <small>Leave empty to search the usual model folders.</small>
        </label>
        <div className="actions">
          <button type="submit" className="btn btn-small">
            Find model files
          </button>
        </div>
      </form>
      {files !== null && (
        <fieldset className="backend-choice">
          <legend>Model files found</legend>
          {models.length === 0 ? (
            <p className="empty">No .gguf model files found. Enter the path of one below.</p>
          ) : (
            models.map((f) => (
              <label key={f.path} className="pick-row">
                <input
                  type="radio"
                  name="llm-file"
                  checked={path === f.path}
                  onChange={() => {
                    setPath(f.path);
                    setProblem(null);
                    if (dir.trim() === "") setDir(parentDir(f.path));
                  }}
                />
                <span className="pick-label" title={f.path}>
                  {fileName(f.path)}
                </span>
                <span className="pick-kind">{formatBytes(f.size)}</span>
              </label>
            ))
          )}
        </fieldset>
      )}
      <form
        className="form"
        onSubmit={(e) => {
          e.preventDefault();
          use();
        }}
      >
        <label>
          Path of the model file (.gguf)
          <input
            value={path}
            spellCheck={false}
            aria-invalid={problem !== null}
            onChange={(e) => {
              setPath(e.target.value);
              setProblem(null);
            }}
          />
        </label>
        {problem && (
          <p className="error" role="alert">
            {problem}
          </p>
        )}
        <div className="actions">
          <button type="submit" className="btn btn-small" disabled={path.trim() === ""}>
            Use this model
          </button>
        </div>
      </form>
      <h3 className="sub-head">Other options</h3>
      {skipped ? (
        <p className="notice" role="status">
          Reports are turned off. Meetings are transcribed, but no report is written. You can turn
          reports on again at any time.
        </p>
      ) : null}
      <div className="actions setup-inline">
        {onOpenSettings && (
          <button type="button" className="btn btn-ghost btn-small" onClick={ctx.onOpenSettings}>
            Use an external server instead
          </button>
        )}
        {skipped ? (
          <button
            type="button"
            className="btn btn-small"
            onClick={() => void act(change((s) => ({ ...s, skipReports: false })))}
          >
            Turn reports on
          </button>
        ) : (
          <button
            type="button"
            className="btn btn-ghost btn-small"
            onClick={() => void act(change((s) => ({ ...s, skipReports: true })))}
          >
            Skip reports for now
          </button>
        )}
      </div>
    </>
  );
}

function VisionStep({ ctx }: { ctx: Ctx }) {
  const { status, settings, change, act } = ctx;
  const item = itemOf(status, "vision");
  const on = settings?.describeScreenshots ?? true;
  return (
    <>
      <p>
        Screenshots taken during a meeting can be described by a vision model, so the report can
        refer to what was on the screen. It is an optional download.
      </p>
      <ItemNote item={item} />
      {on ? (
        <ul className="models" aria-label="Screenshot model">
          <ModelRow ctx={ctx} modelKey="vision" />
        </ul>
      ) : (
        <p className="notice" role="status">
          Screenshot analysis is off. Screenshots are still saved with the meeting, but they are not
          described.
        </p>
      )}
      <div className="actions setup-inline">
        {on ? (
          <button
            type="button"
            className="btn btn-ghost btn-small"
            onClick={() => void act(change((s) => ({ ...s, describeScreenshots: false })))}
          >
            Skip screenshot analysis
          </button>
        ) : (
          <button
            type="button"
            className="btn btn-small"
            onClick={() => void act(change((s) => ({ ...s, describeScreenshots: true })))}
          >
            Turn screenshot analysis on
          </button>
        )}
      </div>
    </>
  );
}

function SummaryStep({ ctx }: { ctx: Ctx }) {
  const { status, go } = ctx;
  return (
    <>
      <p>This is what is ready and what is not.</p>
      <ul className="setup-summary" aria-label="Readiness">
        {(status?.items ?? []).map((i) => {
          const target = stepForItem(i.key);
          return (
            <li key={i.key} className="setup-item">
              <div className="setup-item-main">
                <span className="setup-item-name">{itemLabel(i.key)}</span>
                <span className="setup-item-text">
                  {i.message}
                  {isProblem(i) && i.fixHint ? ` ${i.fixHint}` : ""}
                </span>
              </div>
              <span className={`badge ${badgeClass(i.state)}`}>{STATE_TEXT[i.state]}</span>
              {isProblem(i) && target && (
                <button
                  type="button"
                  className="btn btn-small"
                  aria-label={`Fix ${itemLabel(i.key)}`}
                  onClick={() => go(target)}
                >
                  Fix
                </button>
              )}
            </li>
          );
        })}
      </ul>
      {status?.ready && (
        <p className="setup-note" role="status">
          Everything needed is ready.
        </p>
      )}
    </>
  );
}

function CycleStep({ ctx, onFinish }: { ctx: Ctx; onFinish: () => void }) {
  const { backend, act } = ctx;
  const { cycles, reload } = useCycles();
  const [id, setId] = useState("");
  const [name, setName] = useState("");
  const [created, setCreated] = useState<string | null>(null);
  const problem = newCycleError(id, name);
  const existing = cycles?.length ?? 0;
  return (
    <>
      <p>
        A cycle is a series of meetings that share participants and a report format. Every recording
        belongs to one. You can create the first one now, or later on the Cycles page.
      </p>
      {existing > 0 && created === null && (
        <p className="setup-note" role="status">
          {existing === 1 ? "There is already 1 cycle." : `There are already ${existing} cycles.`}{" "}
          You do not need to create another one.
        </p>
      )}
      {created !== null ? (
        <p className="setup-note" role="status">
          Cycle {created} was created.
        </p>
      ) : (
        <form
          className="form"
          onSubmit={(e) => {
            e.preventDefault();
            if (problem) return;
            void act(
              backend.createCycle(id.trim(), name.trim()).then((d) => {
                setCreated(d.cycle.id);
                reload();
              }),
            );
          }}
        >
          <label>
            Cycle id
            <input value={id} spellCheck={false} onChange={(e) => setId(e.target.value)} />
            <small>Letters, digits, - and _, for example WEEKLY.</small>
          </label>
          <label>
            Cycle name
            <input value={name} onChange={(e) => setName(e.target.value)} />
          </label>
          <div className="actions">
            <button type="submit" className="btn btn-small" disabled={problem !== null}>
              Create first cycle
            </button>
          </div>
        </form>
      )}
      <div className="actions">
        <button type="button" className="btn btn-record" onClick={onFinish}>
          {created !== null || existing > 0 ? "Finish" : "Skip and finish"}
        </button>
      </div>
    </>
  );
}
