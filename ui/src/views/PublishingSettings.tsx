import { useCallback, useEffect, useId, useState } from "react";
import { useBackend } from "../backend-context";
import { errorMessage, useCycles } from "../hooks";
import type { CycleListItem, CycleRepoView, PublishingSecrets } from "../ipc/types";
import { parseWebhook, tokenProblem } from "../publish";

/** What a check of a stored secret answered. */
interface Check {
  ok: boolean;
  message: string;
}

/**
 * One secret: a masked field the value is typed into, with Store, Clear and an optional test.
 * The value lives only in this field. It goes to the backend when stored, the field is emptied
 * at once, and nothing the screen shows or logs repeats it: it is never read back.
 */
function SecretForm(props: {
  /** `GitHub token for api.github.com`: names the field and the buttons. */
  label: string;
  /** What is shown while a secret is stored, e.g. `Token stored`. */
  storedText: string;
  emptyText: string;
  set: boolean;
  placeholder: string;
  help: string;
  /** A problem with the typed value, found before it is sent. */
  problem: (value: string) => string | null;
  onStore: (value: string) => Promise<void>;
  onClear: () => Promise<void>;
  onTest?: () => Promise<Check>;
  testLabel?: string;
}) {
  const { label, set } = props;
  const [value, setValue] = useState("");
  const [message, setMessage] = useState<{ text: string; kind: "error" | "status" } | null>(null);
  const [busy, setBusy] = useState(false);
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
        const problem = props.problem(value);
        if (problem) {
          setMessage({ text: problem, kind: "error" });
          return;
        }
        run(async () => {
          await props.onStore(value);
          // The value is not kept: once it is stored the field is empty again.
          setValue("");
          setMessage({ text: "Stored.", kind: "status" });
        });
      }}
    >
      <p className="token-state">{set ? props.storedText : props.emptyText}</p>
      <label>
        {label}
        <input
          type="password"
          autoComplete="off"
          spellCheck={false}
          value={value}
          placeholder={set ? "Enter a new value to replace it" : props.placeholder}
          aria-describedby={helpId}
          onChange={(e) => setValue(e.target.value)}
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
            className="btn btn-ghost"
            type="button"
            disabled={!set || busy}
            onClick={() =>
              run(async () => {
                const r = await props.onTest!();
                setMessage({ text: r.message, kind: r.ok ? "status" : "error" });
              })
            }
          >
            {props.testLabel ?? "Test"}
          </button>
        )}
        <button
          className="btn btn-ghost"
          type="button"
          disabled={!set || busy}
          onClick={() =>
            run(async () => {
              await props.onClear();
              setMessage({ text: "Removed.", kind: "status" });
            })
          }
        >
          Clear
        </button>
        <button className="btn" type="submit" disabled={value.trim() === "" || busy}>
          Store
        </button>
      </div>
    </form>
  );
}

/** The GitHub repository a registered cycle repository publishes to, and a check of the token. */
function RepoLink(props: {
  repo: CycleRepoView;
  cycle: CycleListItem | undefined;
  onChanged: () => void;
}) {
  const { repo, cycle } = props;
  const backend = useBackend();
  const [github, setGithub] = useState(repo.githubRepo ?? "");
  const [api, setApi] = useState(repo.githubApi ?? "");
  const [message, setMessage] = useState<{ text: string; kind: "error" | "status" } | null>(null);
  const dirty = github.trim() !== (repo.githubRepo ?? "") || api.trim() !== (repo.githubApi ?? "");

  const guard = (work: () => Promise<void>) => {
    setMessage(null);
    work().catch((e) => setMessage({ text: errorMessage(e), kind: "error" }));
  };

  return (
    <form
      className="form"
      aria-label={`GitHub repository of ${repo.id}`}
      onSubmit={(e) => {
        e.preventDefault();
        guard(async () => {
          const saved = await backend.setGithubRepo(repo.id, github.trim(), api.trim());
          setGithub(saved.githubRepo ?? "");
          setApi(saved.githubApi ?? "");
          setMessage({ text: "Saved.", kind: "status" });
          props.onChanged();
        });
      }}
    >
      <p className="token-state">{repo.id}</p>
      <label>
        GitHub repository of {repo.id}
        <input
          value={github}
          placeholder="owner/name"
          autoComplete="off"
          onChange={(e) => setGithub(e.target.value)}
        />
        <small>
          Leave empty when the address of the repository is on github.com: it is taken from there.
        </small>
      </label>
      <label>
        API address of {repo.id}
        <input
          value={api}
          placeholder="https://api.github.com"
          autoComplete="off"
          onChange={(e) => setApi(e.target.value)}
        />
        <small>Only for a GitHub Enterprise server. Empty means github.com.</small>
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
        <button
          className="btn btn-ghost"
          type="button"
          disabled={!cycle || dirty}
          title={cycle ? undefined : "This repository has no usable cycle to test with."}
          onClick={() =>
            guard(async () => {
              const r = await backend.testGithubAccess(cycle!.id);
              setMessage({ text: r.message, kind: r.ok ? "status" : "error" });
            })
          }
        >
          Test access
        </button>
        <button className="btn" type="submit" disabled={!dirty}>
          Save link
        </button>
      </div>
    </form>
  );
}

/**
 * Where the tokens and webhooks for publishing are typed: a GitHub token per host, the GitHub
 * repository of each cycle repository, and a webhook per Discord target. Values are masked,
 * stored in the system credential store and never shown again.
 */
export function PublishingSettingsView() {
  const backend = useBackend();
  const { cycles, repos } = useCycles();
  const [secrets, setSecrets] = useState<PublishingSecrets | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    backend
      .publishingSecrets()
      .then(setSecrets)
      .catch((e) => setError(errorMessage(e)));
  }, [backend]);
  useEffect(load, [load]);
  useEffect(() => {
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

  if (!secrets) {
    return error ? (
      <p className="error" role="alert">
        {error}
      </p>
    ) : null;
  }
  return (
    <section className="publishing-settings" aria-labelledby="publishing-heading">
      <div className="page-head">
        <h2 id="publishing-heading">
          <span className="glitch" data-text="Publishing">
            Publishing
          </span>
        </h2>
      </div>
      <p className="notice">
        Tokens and webhooks are typed here and nowhere else. They go straight to the Windows
        Credential Manager, are never shown again, and are never written to a cycle repository, a
        report or a log.
      </p>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}

      <h3 className="sub-head">GitHub token</h3>
      <div className="secret-list">
        {secrets.github.map((h) => (
          <div key={h.host} className="panel quiet" style={{ maxWidth: 620 }}>
            <div className="panel-body">
              <SecretForm
                label={`GitHub token for ${h.host}`}
                storedText="Token stored"
                emptyText="No token stored"
                set={h.tokenSet}
                placeholder="github_pat_…"
                help="Needs access to the repository (contents and issues). Used to create issues and to push the report."
                problem={tokenProblem}
                onStore={(token) =>
                  backend
                    .setGithubToken(token, h.api === "https://api.github.com" ? undefined : h.api)
                    .then(load)
                }
                onClear={() =>
                  backend
                    .clearGithubToken(h.api === "https://api.github.com" ? undefined : h.api)
                    .then(load)
                }
              />
            </div>
          </div>
        ))}
      </div>

      <h3 className="sub-head">GitHub repository of a cycle repository</h3>
      {repos.length === 0 ? (
        <p className="empty">No cycle repository yet. Add or create one on the Cycles page.</p>
      ) : (
        <div className="secret-list">
          {repos.map((r) => (
            <div key={r.id} className="panel quiet" style={{ maxWidth: 620 }}>
              <div className="panel-body">
                <RepoLink
                  repo={r}
                  cycle={cycles?.find((c) => c.repoId === r.id && c.valid)}
                  onChanged={load}
                />
              </div>
            </div>
          ))}
        </div>
      )}

      <h3 className="sub-head">Discord webhook</h3>
      {secrets.targets.length === 0 ? (
        <p className="empty">
          No cycle posts to Discord. Add a Discord target to a cycle, then store its webhook here.
        </p>
      ) : (
        <div className="secret-list">
          {secrets.targets.map((t) => (
            <div key={t.reference} className="panel quiet" style={{ maxWidth: 620 }}>
              <div className="panel-body">
                <SecretForm
                  label={`Webhook for ${t.reference}`}
                  storedText={t.masked ? `Webhook stored: ${t.masked}` : "Webhook stored"}
                  emptyText={`No webhook stored (used by the cycle ${t.cycleId})`}
                  set={t.set}
                  placeholder="https://discord.com/api/webhooks/…"
                  help={`Used by the cycle ${t.cycleId}. Checked before it is stored; the address itself is never shown again.`}
                  problem={(value) => {
                    const r = parseWebhook(value);
                    return typeof r === "string" ? r : null;
                  }}
                  onStore={(value) => backend.setTargetSecret(t.reference, value).then(load)}
                  onClear={() => backend.clearTargetSecret(t.reference).then(load)}
                  onTest={() => backend.testDiscordWebhook(t.reference)}
                  testLabel="Test webhook"
                />
              </div>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}
