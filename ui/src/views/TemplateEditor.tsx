import { useEffect, useState } from "react";
import { useBackend } from "../backend-context";
import { IDENTIFIER, emptyTemplate, issueText, templateDirty } from "../cycles";
import { errorMessage } from "../hooks";
import type { TemplateCheck, TemplateDto, TemplateSection, TemplateSummary } from "../ipc/types";

/**
 * Templates of one cycle: pick or create one, edit the front matter sections and the body.
 * The text is checked while typing (`validateTemplate`), so an unknown placeholder is shown
 * before saving, and Save stays disabled until the template is valid and changed.
 */
export function TemplateEditor(props: {
  cycleId: string;
  templates: TemplateSummary[];
  /** Called after a template was saved, so the list and the cycle's issues are read again. */
  onChanged: () => void;
}) {
  const { cycleId, templates, onChanged } = props;
  const backend = useBackend();
  // `saved` is what the cycle holds (null for a template that was never saved).
  const [saved, setSaved] = useState<TemplateDto | null>(null);
  const [edited, setEdited] = useState<TemplateDto | null>(null);
  const [check, setCheck] = useState<{ key: string; result: TemplateCheck } | null>(null);
  const [newName, setNewName] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const open = (name: string) => {
    setError(null);
    setMessage(null);
    backend
      .getTemplate(cycleId, name)
      .then((t) => {
        setSaved(t);
        setEdited(structuredClone(t));
        setCheck(null);
      })
      .catch((e) => setError(errorMessage(e)));
  };

  // Live validation, a moment after the last keystroke.
  const key = edited ? JSON.stringify(edited) : null;
  useEffect(() => {
    if (edited === null || key === null) return;
    let cancelled = false;
    const timer = setTimeout(() => {
      backend
        .validateTemplate(edited)
        .then((result) => !cancelled && setCheck({ key, result }))
        .catch((e) => !cancelled && setError(errorMessage(e)));
    }, 150);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
    // `key` stands for the content of `edited`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [backend, key]);

  const create = () => {
    const name = newName.trim();
    if (!IDENTIFIER.test(name)) return;
    if (templates.some((t) => t.name === name)) {
      setError(`A template named ${name} already exists.`);
      return;
    }
    setError(null);
    setMessage(null);
    setSaved(null);
    setEdited(emptyTemplate(name));
    setCheck(null);
    setNewName("");
  };

  const save = () => {
    if (!edited) return;
    setError(null);
    backend
      .saveTemplate(cycleId, edited)
      .then((r) => {
        setCheck({ key: JSON.stringify(edited), result: r });
        if (r.saved) {
          setSaved(structuredClone(edited));
          setMessage("Template saved");
          onChanged();
        }
      })
      .catch((e) => setError(errorMessage(e)));
  };

  const setSection = (i: number, p: Partial<TemplateSection>) =>
    edited &&
    setEdited({
      ...edited,
      sections: edited.sections.map((s, j) => (j === i ? { ...s, ...p } : s)),
    });

  const current = check && check.key === key ? check.result : null;
  const dirty = edited !== null && templateDirty(saved, edited);
  const nameProblem =
    newName.trim() !== "" && !IDENTIFIER.test(newName.trim())
      ? "Use letters, digits, - and _ (no spaces or dots)."
      : null;

  return (
    <div className="templates">
      <ul className="template-list" aria-label="Templates">
        {templates.map((t) => (
          <li key={t.name}>
            <button
              className={
                edited?.name === t.name ? "btn btn-small btn-stop" : "btn btn-small btn-ghost"
              }
              aria-pressed={edited?.name === t.name}
              onClick={() => open(t.name)}
            >
              {t.name}
              {t.sections.length > 0 && ` · ${t.sections.length} sections`}
            </button>
          </li>
        ))}
      </ul>
      <div className="template-new">
        <label>
          New template name
          <input
            value={newName}
            placeholder="weekly"
            onChange={(e) => setNewName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                create();
              }
            }}
          />
        </label>
        <button
          className="btn btn-small"
          type="button"
          disabled={newName.trim() === "" || nameProblem !== null}
          onClick={create}
        >
          New template
        </button>
        {nameProblem && <small className="field-issue">{nameProblem}</small>}
      </div>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}

      {edited && (
        <div className="template-edit" role="group" aria-label={`Template ${edited.name}`}>
          <h4 className="field-head">
            {edited.name}
            {saved === null && <span className="badge badge-job-queued">Not saved yet</span>}
          </h4>
          <h5 className="field-head">Sections (front matter)</h5>
          <ul className="rows" aria-label="Template sections">
            {edited.sections.map((s, i) => (
              <li key={i} className="row-grid">
                <label>
                  Id
                  <input
                    aria-label={`Section ${i + 1} id`}
                    value={s.id}
                    onChange={(e) => setSection(i, { id: e.target.value })}
                  />
                </label>
                <label>
                  Title
                  <input
                    aria-label={`Section ${i + 1} title`}
                    value={s.title}
                    onChange={(e) => setSection(i, { title: e.target.value })}
                  />
                </label>
                <label>
                  What belongs in it
                  <input
                    aria-label={`Section ${i + 1} description`}
                    value={s.description}
                    onChange={(e) => setSection(i, { description: e.target.value })}
                  />
                </label>
                <button
                  className="btn btn-small btn-ghost row-remove"
                  type="button"
                  aria-label={`Remove section ${i + 1}`}
                  onClick={() =>
                    setEdited({ ...edited, sections: edited.sections.filter((_, j) => j !== i) })
                  }
                >
                  Remove
                </button>
              </li>
            ))}
          </ul>
          <button
            className="btn btn-small"
            type="button"
            onClick={() =>
              setEdited({
                ...edited,
                sections: [...edited.sections, { id: "", title: "", description: "" }],
              })
            }
          >
            Add section
          </button>
          <label className="body-field">
            Body
            <textarea
              aria-label="Template body"
              className="code"
              rows={12}
              spellCheck={false}
              value={edited.body}
              onChange={(e) => {
                setMessage(null);
                setEdited({ ...edited, body: e.target.value });
              }}
            />
          </label>
          <details className="placeholders">
            <summary>Placeholders</summary>
            <p className="section-hint">
              {(
                current?.placeholders ??
                check?.result.placeholders ?? [
                  "title",
                  "date",
                  "participants",
                  "summary",
                  "summary_section",
                  "sections",
                  "section:<id>",
                  "tasks",
                  "decisions",
                  "topics",
                  "notes",
                  "ideas",
                  "budgets",
                  "transcript_link",
                  "report_link",
                ]
              ).map((p) => (
                <code key={p}>{`{${p}}`} </code>
              ))}
              Write {"{{"} and {"}}"} for a literal brace.
            </p>
          </details>
          {current && current.issues.length > 0 && (
            <div className="issues" role="alert" aria-label="Template problems">
              <ul>
                {current.issues.map((i, n) => (
                  <li key={n}>{issueText(i)}</li>
                ))}
              </ul>
            </div>
          )}
          {current?.valid && <p className="valid-note">The template is valid.</p>}
          <div className="actions">
            {message && <span role="status">{message}</span>}
            <button
              className="btn btn-record"
              disabled={!dirty || current === null || !current.valid}
              onClick={save}
            >
              Save template
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
