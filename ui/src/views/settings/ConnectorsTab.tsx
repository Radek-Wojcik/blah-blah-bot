import { useCallback, useEffect, useId, useRef, useState } from "react";
import { useBackend } from "../../backend-context";
import { HelpPopover } from "../../components/HelpPopover";
import { errorMessage, useCycles } from "../../hooks";
import type { CycleListItem, ProcessingSettings, PublishingSecrets } from "../../ipc/types";
import { validateLlmSettings } from "../../llm";
import { tokenProblem } from "../../publish";
import type { EditableTabProps } from "../Settings";

interface CheckResult {
  ok: boolean;
  message: string;
}

/** What a secret field reports to the tab so unsaved typed text takes part in the guard. */
interface SecretDraft {
  dirty: boolean;
  save: () => Promise<boolean>;
  discard: () => void;
}

function SecretCard(props: {
  label: string;
  storedText: string;
  emptyText: string;
  isSet: boolean;
  placeholder: string;
  help: string;
  validate?: (value: string) => string | null;
  onStore: (value: string) => Promise<void>;
  onClear: () => Promise<void>;
  onTest?: () => Promise<CheckResult>;
  testLabel?: string;
  testDisabledReason?: string;
  onDraft?: (label: string, draft: SecretDraft | null) => void;
}) {
  const { label, isSet } = props;
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ text: string; kind: "status" | "error" } | null>(null);
  const helpId = useId();
  const propsRef = useRef(props);
  useEffect(() => {
    propsRef.current = props;
  });

  const storeValue = useCallback(async (): Promise<boolean> => {
    const err = propsRef.current.validate ? propsRef.current.validate(value) : null;
    if (err) {
      setMessage({ text: err, kind: "error" });
      return false;
    }
    try {
      await propsRef.current.onStore(value);
      setValue("");
      setMessage({ text: "Stored.", kind: "status" });
      return true;
    } catch (e) {
      setMessage({ text: errorMessage(e), kind: "error" });
      return false;
    }
  }, [value]);

  useEffect(() => {
    propsRef.current.onDraft?.(label, {
      dirty: value.trim() !== "",
      save: storeValue,
      discard: () => {
        setValue("");
        setMessage(null);
      },
    });
  }, [label, value, storeValue]);

  useEffect(() => () => propsRef.current.onDraft?.(label, null), [label]);

  const run = (work: () => Promise<void>) => {
    setMessage(null);
    setBusy(true);
    work()
      .catch((e) => setMessage({ text: errorMessage(e), kind: "error" }))
      .finally(() => setBusy(false));
  };

  return (
    <form
      className="form secret-form"
      aria-label={label}
      onSubmit={(e) => {
        e.preventDefault();
        setMessage(null);
        setBusy(true);
        void storeValue().finally(() => setBusy(false));
      }}
    >
      <p className="token-state">{isSet ? props.storedText : props.emptyText}</p>
      <label>
        {label}
        <input
          type="password"
          autoComplete="off"
          spellCheck={false}
          value={value}
          placeholder={isSet ? "Enter a new value to replace it" : props.placeholder}
          aria-describedby={helpId}
          onChange={(e) => {
            setValue(e.target.value);
            setMessage(null);
          }}
        />
        <small id={helpId}>{props.help}</small>
      </label>
      <p className="person-meta" data-testid="secret-hint">
        {value.trim() !== ""
          ? "Not stored yet: press Store to keep it, or it counts as an unsaved change."
          : "Saved immediately when you press Store; nothing else to save."}
      </p>
      {message && (
        <p
          className={message.kind === "error" ? "error" : "secret-result"}
          role={message.kind === "error" ? "alert" : "status"}
        >
          {message.text}
        </p>
      )}
      <div className="actions">
        {props.onTest && (
          <button
            type="button"
            className="btn btn-ghost"
            disabled={!isSet || busy || Boolean(props.testDisabledReason)}
            title={props.testDisabledReason}
            onClick={() =>
              run(async () => {
                const res = await props.onTest!();
                setMessage({ text: res.message, kind: res.ok ? "status" : "error" });
              })
            }
          >
            {props.testLabel ?? "Test access"}
          </button>
        )}
        <button
          type="button"
          className="btn btn-ghost"
          disabled={!isSet || busy}
          onClick={() =>
            run(async () => {
              await props.onClear();
              setMessage({ text: "Removed.", kind: "status" });
            })
          }
        >
          Clear
        </button>
        <button type="submit" className="btn" disabled={value.trim() === "" || busy}>
          Store
        </button>
      </div>
    </form>
  );
}

/**
 * Connectors settings tab: external account tokens and API keys stored in Windows Credential
 * Manager (GitHub Token, Hugging Face Token, External / Cloud LLM API keys).
 * Discord webhooks and Notion targets are intentionally per-project inside Project Settings.
 */
export function ConnectorsTab({ onDirtyChange, registerHandle }: Partial<EditableTabProps> = {}) {
  const backend = useBackend();
  const { cycles, repos } = useCycles();
  const [secrets, setSecrets] = useState<PublishingSecrets | null>(null);
  const [processing, setProcessing] = useState<ProcessingSettings | null>(null);
  const [extUrl, setExtUrl] = useState("");
  const [extModel, setExtModel] = useState("");
  const [cloudUrl, setCloudUrl] = useState("");
  const [cloudModel, setCloudModel] = useState("");
  const [endpointMsg, setEndpointMsg] = useState<{ text: string; kind: "status" | "error" } | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);
  const draftsRef = useRef(new Map<string, SecretDraft>());
  const [dirtySecrets, setDirtySecrets] = useState<ReadonlySet<string>>(new Set());

  const load = useCallback(() => {
    void Promise.all([backend.publishingSecrets(), backend.getProcessingSettings()])
      .then(([sec, proc]) => {
        setSecrets(sec);
        setProcessing(proc);
        if (proc.llm) {
          setExtUrl(proc.llm.external.baseUrl);
          setExtModel(proc.llm.external.model);
          setCloudUrl(proc.llm.cloud.baseUrl);
          setCloudModel(proc.llm.cloud.model);
        }
      })
      .catch((e) => setError(errorMessage(e)));
  }, [backend]);

  useEffect(() => {
    load();
    let off: (() => void) | undefined;
    let cancelled = false;
    void backend.onCyclesChanged(load).then((u) => {
      if (cancelled) u();
      else off = u;
    });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [backend, load]);

  const cycleForHost = (api: string): CycleListItem | undefined => {
    const validCycles = (cycles ?? []).filter((c) => c.valid);
    for (const c of validCycles) {
      const repo = repos.find((r) => r.id === c.repoId);
      const repoApi = repo?.githubApi || "https://api.github.com";
      if (repoApi === api) return c;
    }
    return validCycles[0];
  };

  const externalDirty =
    processing?.llm != null &&
    (extUrl.trim() !== processing.llm.external.baseUrl ||
      extModel.trim() !== processing.llm.external.model);
  const cloudDirty =
    processing?.llm != null &&
    (cloudUrl.trim() !== processing.llm.cloud.baseUrl ||
      cloudModel.trim() !== processing.llm.cloud.model);
  const dirty = dirtySecrets.size > 0 || externalDirty || cloudDirty;

  const onDraft = useCallback((label: string, draft: SecretDraft | null) => {
    if (draft) draftsRef.current.set(label, draft);
    else draftsRef.current.delete(label);
    setDirtySecrets((prev) => {
      const isDirty = draft?.dirty ?? false;
      if (prev.has(label) === isDirty) return prev;
      const next = new Set(prev);
      if (isDirty) next.add(label);
      else next.delete(label);
      return next;
    });
  }, []);

  const saveEndpoints = useCallback(
    async (kinds: ("external" | "cloud")[]): Promise<boolean> => {
      if (!processing?.llm) return true;
      setEndpointMsg(null);
      const nextLlm = {
        ...processing.llm,
        external: kinds.includes("external")
          ? { baseUrl: extUrl.trim(), model: extModel.trim() }
          : processing.llm.external,
        cloud: kinds.includes("cloud")
          ? { baseUrl: cloudUrl.trim(), model: cloudModel.trim() }
          : processing.llm.cloud,
      };
      const problem = validateLlmSettings(nextLlm);
      if (problem) {
        setEndpointMsg({ text: problem, kind: "error" });
        return false;
      }
      try {
        await backend.saveProcessingSettings({ ...processing, llm: nextLlm });
        setEndpointMsg({ text: "Endpoint settings saved.", kind: "status" });
        load();
        return true;
      } catch (e) {
        setEndpointMsg({ text: errorMessage(e), kind: "error" });
        return false;
      }
    },
    [backend, cloudModel, cloudUrl, extModel, extUrl, load, processing],
  );

  const save = useCallback(async (): Promise<boolean> => {
    let ok = true;
    const kinds: ("external" | "cloud")[] = [];
    if (externalDirty) kinds.push("external");
    if (cloudDirty) kinds.push("cloud");
    if (kinds.length > 0 && !(await saveEndpoints(kinds))) ok = false;
    for (const draft of Array.from(draftsRef.current.values())) {
      if (draft.dirty && !(await draft.save())) ok = false;
    }
    return ok;
  }, [cloudDirty, externalDirty, saveEndpoints]);

  const discard = useCallback(() => {
    for (const draft of Array.from(draftsRef.current.values())) draft.discard();
    if (processing?.llm) {
      setExtUrl(processing.llm.external.baseUrl);
      setExtModel(processing.llm.external.model);
      setCloudUrl(processing.llm.cloud.baseUrl);
      setCloudModel(processing.llm.cloud.model);
    }
    setEndpointMsg(null);
  }, [processing]);

  useEffect(() => {
    onDirtyChange?.(dirty);
  }, [dirty, onDirtyChange]);

  useEffect(() => () => onDirtyChange?.(false), [onDirtyChange]);

  useEffect(() => {
    registerHandle?.({ save, discard });
    return () => registerHandle?.(null);
  }, [registerHandle, save, discard]);

  return (
    <div className="settings-pane">
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}

      <div className="panel quiet">
        <div className="panel-body">
          <div className="field-label-row" style={{ marginBottom: 8 }}>
            <h3 className="sub-head" style={{ margin: 0 }}>
              GitHub Token
            </h3>
            <HelpPopover
              title="GitHub Token & Windows Credential Manager"
              recommended="Fine-grained personal access token with Contents and Issues read/write"
            >
              <p>
                Tokens are written directly to the <strong>Windows Credential Manager</strong> and
                never stored in project files, reports, or logs.
              </p>
              <p>
                Required permissions: <strong>Contents (Read &amp; Write)</strong> to push meeting
                reports and <strong>Issues (Read &amp; Write)</strong> to create and update tasks.
              </p>
            </HelpPopover>
          </div>
          {secrets ? (
            <div className="secret-list">
              {secrets.github.map((h) => {
                const testCycle = cycleForHost(h.api);
                return (
                  <SecretCard
                    key={h.host}
                    label={`GitHub Token (${h.host})`}
                    storedText="Token stored in Windows Credential Manager"
                    emptyText="No token stored"
                    isSet={h.tokenSet}
                    placeholder="github_pat_…"
                    help="Used to push reports and manage issues for linked repositories."
                    validate={tokenProblem}
                    onDraft={onDraft}
                    onStore={(tok) =>
                      backend
                        .setGithubToken(tok, h.api === "https://api.github.com" ? undefined : h.api)
                        .then(load)
                    }
                    onClear={() =>
                      backend
                        .clearGithubToken(h.api === "https://api.github.com" ? undefined : h.api)
                        .then(load)
                    }
                    onTest={testCycle ? () => backend.testGithubAccess(testCycle.id) : undefined}
                    testLabel="Test access"
                    testDisabledReason={
                      testCycle
                        ? undefined
                        : "Create or link a valid project on the Projects page to test repository access."
                    }
                  />
                );
              })}
            </div>
          ) : (
            <p className="person-meta">Loading GitHub hosts…</p>
          )}
        </div>
      </div>

      <div className="panel quiet">
        <div className="panel-body">
          <div className="field-label-row" style={{ marginBottom: 8 }}>
            <h3 className="sub-head" style={{ margin: 0 }}>
              Hugging Face Token
            </h3>
            <HelpPopover
              title="Hugging Face Token"
              recommended="Read-only access token after accepting gated model terms"
            >
              <p>
                Stored safely in <strong>Windows Credential Manager</strong>. Only needed to
                download gated speaker diarization models after accepting their terms on Hugging
                Face.
              </p>
            </HelpPopover>
          </div>
          {processing && (
            <SecretCard
              label="Hugging Face Token"
              storedText="Token stored in Windows Credential Manager"
              emptyText="No token stored"
              isSet={processing.hfTokenSet}
              placeholder="hf_…"
              help="Used only during model downloads for gated diarization weights."
              validate={tokenProblem}
              onDraft={onDraft}
              onStore={(tok) => backend.setHfToken(tok).then(load)}
              onClear={() => backend.clearHfToken().then(load)}
            />
          )}
        </div>
      </div>

      <div className="panel quiet">
        <div className="panel-body">
          <div className="field-label-row" style={{ marginBottom: 8 }}>
            <h3 className="sub-head" style={{ margin: 0 }}>
              External / Cloud LLM API Keys
            </h3>
            <HelpPopover
              title="External & Cloud LLM API Keys"
              recommended="Use local endpoints whenever possible; Cloud requires confirmation per meeting"
            >
              <p>
                API keys are stored per host in <strong>Windows Credential Manager</strong> and are
                never read back into the UI.
              </p>
              <p>
                Cloud endpoints only receive transcript text and screenshot descriptions after you
                explicitly confirm a meeting.
              </p>
            </HelpPopover>
          </div>
          {endpointMsg && (
            <p
              className={endpointMsg.kind === "error" ? "error" : "secret-result"}
              role={endpointMsg.kind === "error" ? "alert" : "status"}
            >
              {endpointMsg.text}
            </p>
          )}
          {processing?.llm && (
            <div className="settings-grid-2col">
              <div className="settings-subcard">
                <p className="token-state" style={{ marginTop: 0 }}>
                  External LLM Server
                </p>
                <div className="form" style={{ marginBottom: 12 }}>
                  <label>
                    Host URL
                    <input
                      value={extUrl}
                      placeholder="http://127.0.0.1:11434/v1"
                      onChange={(e) => setExtUrl(e.target.value)}
                    />
                  </label>
                  <label>
                    Model
                    <input
                      value={extModel}
                      placeholder="gemma4:12b"
                      onChange={(e) => setExtModel(e.target.value)}
                    />
                  </label>
                  <div className="actions">
                    <button
                      type="button"
                      className="btn btn-small"
                      onClick={() => void saveEndpoints(["external"])}
                    >
                      Save external host
                    </button>
                    <span className="person-meta" data-testid="external-endpoint-state">
                      {externalDirty
                        ? "Unsaved changes: press the button to save, or Settings will ask."
                        : "Saved."}
                    </span>
                  </div>
                </div>
                <SecretCard
                  label="External LLM API Key"
                  storedText="API key stored"
                  emptyText="No API key stored"
                  isSet={processing.llm.externalKeySet}
                  placeholder="Optional bearer key"
                  help="Stored in Windows Credential Manager for the external OpenAI-compatible host."
                  onDraft={onDraft}
                  onStore={(k) => backend.setExternalKey(k).then(load)}
                  onClear={() => backend.clearExternalKey().then(load)}
                />
              </div>

              <div className="settings-subcard">
                <p className="token-state" style={{ marginTop: 0 }}>
                  Cloud LLM API (per-meeting only)
                </p>
                <div className="form" style={{ marginBottom: 12 }}>
                  <label>
                    Host URL
                    <input
                      value={cloudUrl}
                      placeholder="https://api.example.com/v1"
                      onChange={(e) => setCloudUrl(e.target.value)}
                    />
                  </label>
                  <label>
                    Model
                    <input
                      value={cloudModel}
                      placeholder="large-model-1"
                      onChange={(e) => setCloudModel(e.target.value)}
                    />
                  </label>
                  <div className="actions">
                    <button
                      type="button"
                      className="btn btn-small"
                      onClick={() => void saveEndpoints(["cloud"])}
                    >
                      Save cloud host
                    </button>
                    <span className="person-meta" data-testid="cloud-endpoint-state">
                      {cloudDirty
                        ? "Unsaved changes: press the button to save, or Settings will ask."
                        : "Saved."}
                    </span>
                  </div>
                </div>
                <SecretCard
                  label="Cloud LLM API Key"
                  storedText="API key stored"
                  emptyText="No API key stored"
                  isSet={processing.llm.cloudKeySet}
                  placeholder="API key"
                  help="Stored in Windows Credential Manager for per-meeting cloud extraction."
                  onDraft={onDraft}
                  onStore={(k) => backend.setCloudKey(k).then(load)}
                  onClear={() => backend.clearCloudKey().then(load)}
                />
              </div>
            </div>
          )}
        </div>
      </div>

      <div className="panel quiet">
        <div className="panel-body">
          <h3 className="sub-head" style={{ marginTop: 0 }}>
            Discord &amp; Notion targets
          </h3>
          <p className="notice" style={{ margin: 0 }}>
            There is no global Discord webhook here. Discord webhooks and Notion database targets
            are configured per project inside <strong>Project Settings</strong> on the Projects
            page.
          </p>
        </div>
      </div>
    </div>
  );
}
