import { useCallback, useEffect, useId, useState } from "react";
import { useBackend } from "../backend-context";
import { errorMessage, useCycles } from "../hooks";
import type {
  CycleListItem,
  CycleRepoView,
  CycleTargets,
  PublishMode,
  PublishTargetName,
  PublishingSecrets,
} from "../ipc/types";
import {
  DEFAULT_PUBLISH_RULE,
  STATUS_WRITE_BACK_NOTE,
  botTokenProblem,
  notionTokenProblem,
  parseWebhook,
  tokenProblem,
} from "../publish";
import { DiscordInviteBox } from "./DiscordInviteBox";
import { RuleChoice } from "./RuleChoice";

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
 * The rule of each target of one cycle. A change is saved to `cycle.yaml` at once, with nothing
 * else in the file touched. Discord is only there when the cycle has a Discord target.
 */
function CycleRules(props: { cycle: CycleListItem; version: unknown }) {
  const { cycle } = props;
  const backend = useBackend();
  const [targets, setTargets] = useState<CycleTargets | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    backend
      .getCycle(cycle.id)
      .then((d) => {
        if (!cancelled) setTargets(d.cycle.targets);
      })
      .catch((e) => {
        if (!cancelled) setError(errorMessage(e));
      });
    return () => {
      cancelled = true;
    };
  }, [backend, cycle.id, props.version]);

  const change = (target: PublishTargetName, mode: PublishMode) => {
    setError(null);
    setBusy(true);
    backend
      .setTargetRule(cycle.id, target, mode)
      .then(setTargets)
      .catch((e) => setError(errorMessage(e)))
      .finally(() => setBusy(false));
  };

  if (!targets) return error ? <p className="error">{error}</p> : null;
  const github = targets.github?.publish ?? DEFAULT_PUBLISH_RULE.github;
  const discord = targets.discord?.publish ?? null;
  const notion = targets.notion?.publish ?? null;
  const row = (target: PublishTargetName, label: string, mode: PublishMode) => (
    <div className="rule-row">
      <span className="field-label">{label}</span>
      <RuleChoice
        name={`${label}, cycle ${cycle.name}`}
        target={target}
        value={mode}
        disabled={busy}
        onChange={(m) => change(target, m)}
      />
    </div>
  );
  return (
    <div className="panel quiet" style={{ maxWidth: 620 }}>
      <div className="panel-body">
        <p className="token-state">
          {cycle.name} <span className="person-meta">{cycle.id}</span>
        </p>
        {row("github", "GitHub", github)}
        {discord !== null ? (
          row("discord", "Discord", discord)
        ) : (
          <p className="target-rule-help">
            No Discord target: add one in the settings of the cycle to choose a rule for it.
          </p>
        )}
        {notion !== null ? (
          row("notion", "Notion", notion)
        ) : (
          <p className="target-rule-help">
            No Notion target: add one in the settings of the cycle to choose a rule for it.
          </p>
        )}
        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}
      </div>
    </div>
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
      {(() => {
        const webhooks = secrets.targets.filter((t) => t.kind === "discord");
        return webhooks.length === 0 ? (
          <p className="empty">
            No cycle uses a Discord webhook. Add a Discord webhook target to a cycle, then store its
            webhook here.
          </p>
        ) : (
          <div className="secret-list">
            {webhooks.map((t) => (
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
        );
      })()}

      <h3 className="sub-head">Discord bot</h3>
      {(() => {
        const bots = secrets.targets.filter((t) => t.kind === "discord-bot");
        return (
          <div className="secret-list">
            {bots.length === 0 ? (
              <p className="empty">
                No cycle uses a Discord bot. Switch a cycle&apos;s Discord target to Bot mode, then
                store its bot token here.
              </p>
            ) : (
              bots.map((t) => (
                <div key={t.reference} className="panel quiet" style={{ maxWidth: 620 }}>
                  <div className="panel-body">
                    <SecretForm
                      label={`Bot token for ${t.reference}`}
                      storedText={t.masked ? `Bot token stored: ${t.masked}` : "Bot token stored"}
                      emptyText={`No bot token stored (used by the cycle ${t.cycleId})`}
                      set={t.set}
                      placeholder="MTIzNDU2Nzg5MDEyMzQ1Njc4…"
                      help={`Used by the cycle ${t.cycleId}. Checked before it is stored; the token itself is never shown again.`}
                      problem={botTokenProblem}
                      onStore={(value) => backend.setTargetSecret(t.reference, value).then(load)}
                      onClear={() => backend.clearTargetSecret(t.reference).then(load)}
                      onTest={async () => {
                        const r = await backend.testDiscordBot(t.cycleId);
                        const detail =
                          r.missing.length > 0 && !r.message.includes(r.missing[0])
                            ? ` Missing permissions: ${r.missing.join(", ")}.`
                            : "";
                        return { ok: r.ok, message: `${r.message}${detail}` };
                      }}
                      testLabel="Test bot"
                    />
                  </div>
                </div>
              ))
            )}
            <div className="panel quiet" style={{ maxWidth: 620 }}>
              <div className="panel-body">
                <DiscordInviteBox />
              </div>
            </div>
          </div>
        );
      })()}

      <h3 className="sub-head">Notion integration</h3>
      {(() => {
        const notions = secrets.targets.filter((t) => t.kind === "notion");
        return notions.length === 0 ? (
          <p className="empty">
            No cycle exports tasks to Notion. Add a Notion target to a cycle, then store its
            integration token here.
          </p>
        ) : (
          <div className="secret-list">
            {notions.map((t) => (
              <div key={t.reference} className="panel quiet" style={{ maxWidth: 620 }}>
                <div className="panel-body">
                  <SecretForm
                    label={`Notion token for ${t.reference}`}
                    storedText={t.masked ? `Token stored: ${t.masked}` : "Token stored"}
                    emptyText={`No Notion token stored (used by the cycle ${t.cycleId})`}
                    set={t.set}
                    placeholder="ntn_…"
                    help={`Used by the cycle ${t.cycleId}. Share the Notion database with this integration (... > Connections).`}
                    problem={notionTokenProblem}
                    onStore={(value) => backend.setTargetSecret(t.reference, value).then(load)}
                    onClear={() => backend.clearTargetSecret(t.reference).then(load)}
                    onTest={async () => {
                      const r = await backend.testNotionAccess(t.cycleId);
                      return { ok: r.ok, message: r.message };
                    }}
                    testLabel="Test access"
                  />
                </div>
              </div>
            ))}
          </div>
        );
      })()}

      <h3 className="sub-head">When to publish</h3>
      <p className="section-hint">
        Per cycle and target: Off never publishes, On approval waits for you to press Publish in the
        draft, Automatic publishes as soon as the draft is complete. {STATUS_WRITE_BACK_NOTE}
      </p>
      {(cycles ?? []).filter((c) => c.valid).length === 0 ? (
        <p className="empty">No usable cycle yet. Create one on the Cycles page.</p>
      ) : (
        <div className="secret-list">
          {(cycles ?? [])
            .filter((c) => c.valid)
            .map((c) => (
              <CycleRules key={c.id} cycle={c} version={cycles} />
            ))}
        </div>
      )}
    </section>
  );
}
