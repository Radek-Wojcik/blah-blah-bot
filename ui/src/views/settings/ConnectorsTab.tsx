import { useCallback, useEffect, useId, useState } from "react";
import { useBackend } from "../../backend-context";
import { HelpPopover } from "../../components/HelpPopover";
import { errorMessage, useCycles } from "../../hooks";
import type { CycleListItem, ProcessingSettings, PublishingSecrets } from "../../ipc/types";
import { validateLlmSettings } from "../../llm";
import { tokenProblem } from "../../publish";

interface CheckResult {
  ok: boolean;
  message: string;
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
}) {
  const { label, isSet } = props;
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ text: string; kind: "status" | "error" } | null>(null);
  const helpId = useId();

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
        const err = props.validate ? props.validate(value) : null;
        if (err) {
          setMessage({ text: err, kind: "error" });
          return;
        }
        run(async () => {
          await props.onStore(value);
          setValue("");
          setMessage({ text: "Stored.", kind: "status" });
        });
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
export function ConnectorsTab() {
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

  const saveEndpoints = async (kind: "external" | "cloud") => {
    if (!processing?.llm) return;
    setEndpointMsg(null);
    const nextLlm = {
      ...processing.llm,
      external:
        kind === "external"
          ? { baseUrl: extUrl.trim(), model: extModel.trim() }
          : processing.llm.external,
      cloud:
        kind === "cloud"
          ? { baseUrl: cloudUrl.trim(), model: cloudModel.trim() }
          : processing.llm.cloud,
    };
    const problem = validateLlmSettings(nextLlm);
    if (problem) {
      setEndpointMsg({ text: problem, kind: "error" });
      return;
    }
    try {
      await backend.saveProcessingSettings({ ...processing, llm: nextLlm });
      setEndpointMsg({ text: "Endpoint settings saved.", kind: "status" });
      load();
    } catch (e) {
      setEndpointMsg({ text: errorMessage(e), kind: "error" });
    }
  };

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
              <div className="llm-group">
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
                      onClick={() => void saveEndpoints("external")}
                    >
                      Save external host
                    </button>
                  </div>
                </div>
                <SecretCard
                  label="External LLM API Key"
                  storedText="API key stored"
                  emptyText="No API key stored"
                  isSet={processing.llm.externalKeySet}
                  placeholder="Optional bearer key"
                  help="Stored in Windows Credential Manager for the external OpenAI-compatible host."
                  onStore={(k) => backend.setExternalKey(k).then(load)}
                  onClear={() => backend.clearExternalKey().then(load)}
                />
              </div>

              <div className="llm-group">
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
                      onClick={() => void saveEndpoints("cloud")}
                    >
                      Save cloud host
                    </button>
                  </div>
                </div>
                <SecretCard
                  label="Cloud LLM API Key"
                  storedText="API key stored"
                  emptyText="No API key stored"
                  isSet={processing.llm.cloudKeySet}
                  placeholder="API key"
                  help="Stored in Windows Credential Manager for per-meeting cloud extraction."
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
