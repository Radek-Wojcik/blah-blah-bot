import { type ReactNode, useCallback, useEffect, useMemo, useState } from "react";
import { useBackend } from "../backend-context";
import {
  type TypeRow,
  duplicateTypes,
  emptyParticipant,
  isDirty,
  issueText,
  issuesOf,
  typeRows,
  withTypeRows,
} from "../cycles";
import { errorMessage } from "../hooks";
import type {
  CycleDetails,
  CycleDto,
  CycleIssue,
  CycleParticipant,
  LanguageSetting,
  PublishMode,
  ReportMode,
} from "../ipc/types";
import { MODE_HELP, MODE_NAMES, REPORT_MODES } from "../report-gate";
import { CycleTasksView } from "./CycleTasks";
import { TemplateEditor } from "./TemplateEditor";

const LANGUAGES: { value: LanguageSetting; label: string }[] = [
  { value: "auto", label: "Auto" },
  { value: "pl", label: "Polish" },
  { value: "en", label: "English" },
];

const PUBLISH: { value: PublishMode; label: string }[] = [
  { value: "on-approval", label: "After approval in the draft" },
  { value: "auto", label: "Automatically" },
  { value: "off", label: "Never" },
];

/** The problems found at one path, shown next to the field. */
function Problems({ issues, path }: { issues: CycleIssue[]; path: string }) {
  const here = issuesOf(issues, path);
  if (here.length === 0) return null;
  return (
    <small className="field-issue" role="alert">
      {here.map((i) => i.message).join(" ")}
    </small>
  );
}

function Section(props: { id: string; title: string; hint?: string; children: ReactNode }) {
  return (
    <section className="editor-section" aria-labelledby={`sec-${props.id}`}>
      <h3 id={`sec-${props.id}`} className="sub-head">
        {props.title}
      </h3>
      {props.hint && <p className="section-hint">{props.hint}</p>}
      <div className="panel quiet">
        <div className="panel-body">{props.children}</div>
      </div>
    </section>
  );
}

/**
 * The editor of one cycle: general settings, participants, glossary, report defaults,
 * templates and publishing targets. Save is only possible while something changed, and the
 * problems the backend finds are shown at the top and next to the field they are about.
 */
export function CycleEditor(props: { cycleId: string; onBack: () => void }) {
  const { cycleId, onBack } = props;
  const backend = useBackend();
  const [details, setDetails] = useState<CycleDetails | null>(null);
  const [cycle, setCycle] = useState<CycleDto | null>(null);
  const [rows, setRows] = useState<TypeRow[]>([]);
  const [issues, setIssues] = useState<CycleIssue[]>([]);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const adopt = useCallback((d: CycleDetails) => {
    setDetails(d);
    setCycle(structuredClone(d.cycle));
    setRows(typeRows(d.cycle.report.byType));
    setIssues(d.issues);
  }, []);

  useEffect(() => {
    let cancelled = false;
    backend
      .getCycle(cycleId)
      .then((d) => !cancelled && adopt(d))
      .catch((e) => !cancelled && setError(errorMessage(e)));
    return () => {
      cancelled = true;
    };
  }, [backend, cycleId, adopt]);

  const edited = useMemo(() => (cycle ? withTypeRows(cycle, rows) : null), [cycle, rows]);
  const dirty = details !== null && edited !== null && isDirty(details.cycle, edited);
  const dupTypes = duplicateTypes(rows);

  if (!details || !cycle || !edited) {
    return (
      <>
        <EditorHead cycleId={cycleId} onBack={onBack} />
        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}
      </>
    );
  }

  const patch = (p: Partial<CycleDto>) => setCycle({ ...cycle, ...p });
  const setParticipant = (i: number, p: Partial<CycleParticipant>) =>
    patch({ participants: cycle.participants.map((x, j) => (j === i ? { ...x, ...p } : x)) });
  const setReport = (p: Partial<CycleDto["report"]>) =>
    patch({ report: { ...cycle.report, ...p } });
  const setTargets = (p: Partial<CycleDto["targets"]>) =>
    patch({ targets: { ...cycle.targets, ...p } });

  const save = () => {
    setSaving(true);
    setError(null);
    setMessage(null);
    backend
      .saveCycle(edited)
      .then((r) => {
        if (r.saved && r.details) {
          adopt(r.details);
          setMessage(r.warnings.length > 0 ? `Saved. ${r.warnings.join(" ")}` : "Saved");
        } else {
          setIssues(r.issues);
        }
      })
      .catch((e) => setError(errorMessage(e)))
      .finally(() => setSaving(false));
  };

  const templateNames = details.templates.map((t) => t.name);
  const discordRef = cycle.targets.discord;
  const github = cycle.targets.github;
  const notion = cycle.targets.notion;

  return (
    <>
      <EditorHead
        cycleId={cycleId}
        onBack={onBack}
        dirty={dirty}
        saving={saving}
        onSave={save}
        message={message}
      />
      <p className="card-when">
        {details.cycle.name} · repository {details.repoId}
        {details.stateAsOf && ` · state as of ${details.stateAsOf}`}
        {details.offline && " · offline"}
      </p>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {issues.length > 0 && (
        <div className="issues" role="alert" aria-label="Problems">
          <strong>
            {issues.length === 1 ? "1 problem" : `${issues.length} problems`} to fix before this
            cycle can be saved or used:
          </strong>
          <ul>
            {issues.map((i, n) => (
              <li key={n}>{issueText(i)}</li>
            ))}
          </ul>
        </div>
      )}

      <Section id="general" title="General">
        <div className="form">
          <label>
            Cycle id
            <input value={cycle.id} readOnly aria-readonly="true" />
            <small>The id is the folder name and cannot be changed.</small>
          </label>
          <label>
            Name
            <input value={cycle.name} onChange={(e) => patch({ name: e.target.value })} />
            <Problems issues={issues} path="name" />
          </label>
          <label>
            Transcription language
            <select
              value={cycle.languages.asr}
              onChange={(e) =>
                patch({ languages: { ...cycle.languages, asr: e.target.value as LanguageSetting } })
              }
            >
              {LANGUAGES.map((l) => (
                <option key={l.value} value={l.value}>
                  {l.label}
                </option>
              ))}
            </select>
          </label>
          <label>
            Report language
            <select
              value={cycle.languages.output}
              onChange={(e) =>
                patch({
                  languages: { ...cycle.languages, output: e.target.value as LanguageSetting },
                })
              }
            >
              {LANGUAGES.map((l) => (
                <option key={l.value} value={l.value}>
                  {l.label}
                </option>
              ))}
            </select>
            <small>Auto follows the dominant language of each meeting.</small>
          </label>
        </div>
      </Section>

      <Section
        id="participants"
        title="Participants"
        hint="The people of this cycle. Voice profiles and task assignees refer to the id."
      >
        <ul className="rows" aria-label="Participants">
          {cycle.participants.map((p, i) => {
            const at = `participants[${i}]`;
            const kind = p.discord ? (p.discord.roleId !== undefined ? "role" : "user") : "";
            const discordId = p.discord?.userId ?? p.discord?.roleId ?? "";
            return (
              <li key={i} className="row-grid">
                <label>
                  Id
                  <input
                    aria-label={`Participant ${i + 1} id`}
                    value={p.id}
                    onChange={(e) => setParticipant(i, { id: e.target.value })}
                  />
                  <Problems issues={issues} path={`${at}.id`} />
                </label>
                <label>
                  Name
                  <input
                    aria-label={`Participant ${i + 1} name`}
                    value={p.name}
                    onChange={(e) => setParticipant(i, { name: e.target.value })}
                  />
                  <Problems issues={issues} path={`${at}.name`} />
                </label>
                <label>
                  GitHub login
                  <input
                    aria-label={`Participant ${i + 1} GitHub login`}
                    value={p.github ?? ""}
                    onChange={(e) => setParticipant(i, { github: e.target.value || undefined })}
                  />
                  <Problems issues={issues} path={`${at}.github`} />
                </label>
                <label>
                  Discord
                  <select
                    aria-label={`Participant ${i + 1} Discord kind`}
                    value={kind}
                    onChange={(e) => {
                      const v = e.target.value;
                      setParticipant(i, {
                        discord:
                          v === ""
                            ? undefined
                            : v === "role"
                              ? { roleId: discordId }
                              : { userId: discordId },
                      });
                    }}
                  >
                    <option value="">None</option>
                    <option value="user">User id</option>
                    <option value="role">Role id</option>
                  </select>
                </label>
                {kind !== "" && (
                  <label>
                    Discord id
                    <input
                      aria-label={`Participant ${i + 1} Discord id`}
                      value={discordId}
                      inputMode="numeric"
                      onChange={(e) =>
                        setParticipant(i, {
                          discord:
                            kind === "role"
                              ? { roleId: e.target.value }
                              : { userId: e.target.value },
                        })
                      }
                    />
                    <Problems issues={issues} path={`${at}.discord.user_id`} />
                    <Problems issues={issues} path={`${at}.discord.role_id`} />
                    <Problems issues={issues} path={`${at}.discord`} />
                  </label>
                )}
                <button
                  className="btn btn-small btn-ghost row-remove"
                  type="button"
                  aria-label={`Remove participant ${i + 1}`}
                  onClick={() =>
                    patch({ participants: cycle.participants.filter((_, j) => j !== i) })
                  }
                >
                  Remove
                </button>
              </li>
            );
          })}
        </ul>
        <button
          className="btn btn-small"
          type="button"
          onClick={() => patch({ participants: [...cycle.participants, emptyParticipant()] })}
        >
          Add participant
        </button>
      </Section>

      <Section
        id="glossary"
        title="Glossary"
        hint="Terms the model should spell right. A hint says what the term means."
      >
        <ul className="rows" aria-label="Glossary">
          {cycle.glossary.map((g, i) => (
            <li key={i} className="row-grid">
              <label>
                Term
                <input
                  aria-label={`Glossary term ${i + 1}`}
                  value={g.term}
                  onChange={(e) =>
                    patch({
                      glossary: cycle.glossary.map((x, j) =>
                        j === i ? { ...x, term: e.target.value } : x,
                      ),
                    })
                  }
                />
                <Problems issues={issues} path={`glossary[${i}].term`} />
              </label>
              <label>
                Hint
                <input
                  aria-label={`Glossary hint ${i + 1}`}
                  value={g.hint ?? ""}
                  onChange={(e) =>
                    patch({
                      glossary: cycle.glossary.map((x, j) =>
                        j === i ? { ...x, hint: e.target.value || undefined } : x,
                      ),
                    })
                  }
                />
              </label>
              <button
                className="btn btn-small btn-ghost row-remove"
                type="button"
                aria-label={`Remove glossary term ${i + 1}`}
                onClick={() => patch({ glossary: cycle.glossary.filter((_, j) => j !== i) })}
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
        <button
          className="btn btn-small"
          type="button"
          onClick={() => patch({ glossary: [...cycle.glossary, { term: "" }] })}
        >
          Add term
        </button>
      </Section>

      <Section id="report" title="Report" hint="What a recording of this cycle starts with.">
        <div className="form">
          <label>
            Default mode
            <select
              value={cycle.report.defaultMode}
              onChange={(e) => setReport({ defaultMode: e.target.value as ReportMode })}
            >
              {REPORT_MODES.map((m) => (
                <option key={m} value={m}>
                  {MODE_NAMES[m]}
                </option>
              ))}
            </select>
            <small>{MODE_HELP[cycle.report.defaultMode]}</small>
            <Problems issues={issues} path="report.default_mode" />
          </label>
          <label>
            Default template
            <select
              value={cycle.report.defaultTemplate}
              onChange={(e) => setReport({ defaultTemplate: e.target.value })}
            >
              {!templateNames.includes(cycle.report.defaultTemplate) && (
                <option value={cycle.report.defaultTemplate}>
                  {cycle.report.defaultTemplate} (missing)
                </option>
              )}
              {templateNames.map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </select>
            <Problems issues={issues} path="report.default_template" />
          </label>
          <label>
            Instruction
            <textarea
              rows={3}
              value={cycle.report.instruction}
              placeholder="Group tasks by person, keep it short."
              onChange={(e) => setReport({ instruction: e.target.value })}
            />
            <small>Given to the model with every extraction of this cycle.</small>
          </label>
        </div>
        <h4 className="field-head">Meeting types</h4>
        <p className="section-hint">
          Each meeting type picks a template. The type is chosen before recording in by-type mode.
        </p>
        <ul className="rows" aria-label="Meeting types">
          {rows.map((r, i) => (
            <li key={i} className="row-grid">
              <label>
                Type
                <input
                  aria-label={`Meeting type ${i + 1}`}
                  value={r.type}
                  onChange={(e) =>
                    setRows(rows.map((x, j) => (j === i ? { ...x, type: e.target.value } : x)))
                  }
                />
              </label>
              <label>
                Template
                <select
                  aria-label={`Template of meeting type ${i + 1}`}
                  value={r.template}
                  onChange={(e) =>
                    setRows(rows.map((x, j) => (j === i ? { ...x, template: e.target.value } : x)))
                  }
                >
                  {!templateNames.includes(r.template) && (
                    <option value={r.template}>{r.template || "Choose…"}</option>
                  )}
                  {templateNames.map((n) => (
                    <option key={n} value={n}>
                      {n}
                    </option>
                  ))}
                </select>
                <Problems issues={issues} path={`report.by_type.${r.type}`} />
              </label>
              <button
                className="btn btn-small btn-ghost row-remove"
                type="button"
                aria-label={`Remove meeting type ${i + 1}`}
                onClick={() => setRows(rows.filter((_, j) => j !== i))}
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
        {dupTypes.length > 0 && (
          <p className="error" role="alert">
            The meeting type {dupTypes.join(", ")} is listed twice; only the last one is kept.
          </p>
        )}
        <button
          className="btn btn-small"
          type="button"
          onClick={() => setRows([...rows, { type: "", template: cycle.report.defaultTemplate }])}
        >
          Add meeting type
        </button>
      </Section>

      <Section
        id="templates"
        title="Templates"
        hint="A template is Markdown with placeholders; its front matter can declare sections."
      >
        <TemplateEditor
          cycleId={cycleId}
          templates={details.templates}
          onChanged={() =>
            void backend
              .getCycle(cycleId)
              .then((d) =>
                setDetails((cur) =>
                  cur ? { ...cur, templates: d.templates, issues: d.issues, valid: d.valid } : d,
                ),
              )
          }
        />
      </Section>

      <Section
        id="targets"
        title="Targets"
        hint="Where a report is published, and when: after approval (when you press Publish in the draft), automatically when the draft is complete, or never. Secrets are never stored here: Discord and Notion refer to them as cred:name."
      >
        <div className="form targets">
          <fieldset>
            <legend>GitHub</legend>
            <label className="check">
              <input
                type="checkbox"
                checked={github != null}
                onChange={(e) =>
                  setTargets({
                    github: e.target.checked ? { path: "meetings", publish: "on-approval" } : null,
                  })
                }
              />
              Commit reports to the repository
            </label>
            {github && (
              <>
                <label>
                  Folder for meeting reports
                  <input
                    value={github.path}
                    onChange={(e) => setTargets({ github: { ...github, path: e.target.value } })}
                  />
                  <Problems issues={issues} path="targets.github.path" />
                </label>
                <label className="check">
                  <input
                    type="checkbox"
                    checked={github.issues !== false}
                    onChange={(e) =>
                      setTargets({
                        github: { ...github, issues: e.target.checked ? undefined : false },
                      })
                    }
                  />
                  Create issues for tasks and write back approved status changes
                </label>
                <label>
                  Publish
                  <select
                    aria-label="GitHub publish"
                    value={github.publish}
                    onChange={(e) =>
                      setTargets({ github: { ...github, publish: e.target.value as PublishMode } })
                    }
                  >
                    {PUBLISH.map((p) => (
                      <option key={p.value} value={p.value}>
                        {p.label}
                      </option>
                    ))}
                  </select>
                </label>
              </>
            )}
          </fieldset>
          <fieldset>
            <legend>Discord</legend>
            <label className="check">
              <input
                type="checkbox"
                checked={discordRef != null}
                onChange={(e) =>
                  setTargets({
                    discord: e.target.checked ? { webhook: "cred:", publish: "on-approval" } : null,
                  })
                }
              />
              Post a summary to a channel
            </label>
            {discordRef && (
              <>
                <label>
                  Webhook reference
                  <input
                    value={discordRef.webhook}
                    placeholder="cred:discord/nomad"
                    onChange={(e) =>
                      setTargets({ discord: { ...discordRef, webhook: e.target.value } })
                    }
                  />
                  <small>
                    The name of a secret in the Windows Credential Manager, written cred:name. The
                    webhook address itself is never stored in the repository.
                  </small>
                  <Problems issues={issues} path="targets.discord.webhook" />
                </label>
                <label>
                  Message template
                  <select
                    aria-label="Discord template"
                    value={discordRef.template ?? ""}
                    onChange={(e) =>
                      setTargets({
                        discord: { ...discordRef, template: e.target.value || undefined },
                      })
                    }
                  >
                    <option value="">Same as the report</option>
                    {templateNames.map((n) => (
                      <option key={n} value={n}>
                        {n}
                      </option>
                    ))}
                  </select>
                  <Problems issues={issues} path="targets.discord.template" />
                </label>
                <label>
                  Publish
                  <select
                    aria-label="Discord publish"
                    value={discordRef.publish}
                    onChange={(e) =>
                      setTargets({
                        discord: { ...discordRef, publish: e.target.value as PublishMode },
                      })
                    }
                  >
                    {PUBLISH.map((p) => (
                      <option key={p.value} value={p.value}>
                        {p.label}
                      </option>
                    ))}
                  </select>
                </label>
              </>
            )}
          </fieldset>
          <fieldset>
            <legend>Notion</legend>
            <label className="check">
              <input
                type="checkbox"
                checked={notion != null}
                onChange={(e) =>
                  setTargets({
                    notion: e.target.checked ? { database: "", publish: "on-approval" } : null,
                  })
                }
              />
              Add the report to a database
            </label>
            {notion && (
              <>
                <label>
                  Database
                  <input
                    value={notion.database}
                    placeholder="cred:notion/nomad-db"
                    onChange={(e) =>
                      setTargets({ notion: { ...notion, database: e.target.value } })
                    }
                  />
                  <small>A database id or a cred: reference to it.</small>
                  <Problems issues={issues} path="targets.notion.database" />
                </label>
                <label>
                  Publish
                  <select
                    aria-label="Notion publish"
                    value={notion.publish}
                    onChange={(e) =>
                      setTargets({ notion: { ...notion, publish: e.target.value as PublishMode } })
                    }
                  >
                    {PUBLISH.map((p) => (
                      <option key={p.value} value={p.value}>
                        {p.label}
                      </option>
                    ))}
                  </select>
                </label>
              </>
            )}
          </fieldset>
        </div>
      </Section>

      <Section
        id="tasks"
        title="Tasks"
        hint="The tasks of this cycle that became GitHub issues. A closed issue counts as done."
      >
        <CycleTasksView cycleId={cycleId} repoId={details.repoId} />
      </Section>

      <h3 className="sub-head">Open topics</h3>
      {details.topics.length === 0 ? (
        <p className="empty">No topics yet.</p>
      ) : (
        <ul className="topics" aria-label="Topics">
          {details.topics.map((t) => (
            <li key={t.id}>
              <span
                className={t.status === "open" ? "badge badge-job-running" : "badge badge-excluded"}
              >
                {t.status}
              </span>{" "}
              {t.text} <span className="person-meta">opened {t.openedIn}</span>
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

function EditorHead(props: {
  cycleId: string;
  onBack: () => void;
  dirty?: boolean;
  saving?: boolean;
  onSave?: () => void;
  message?: string | null;
}) {
  return (
    <div className="page-head">
      <h2>
        <span className="glitch" data-text="Cycle">
          Cycle
        </span>
      </h2>
      <span className="head-actions">
        {props.message && <span role="status">{props.message}</span>}
        <button
          className="btn btn-small btn-record"
          disabled={!props.dirty || props.saving}
          onClick={props.onSave}
        >
          Save
        </button>
        <button className="btn btn-small btn-ghost" onClick={props.onBack}>
          Back to cycles
        </button>
      </span>
    </div>
  );
}
