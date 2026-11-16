import { useCallback, useEffect, useState } from "react";
import { useBackend } from "../backend-context";
import { errorMessage, useOutbox } from "../hooks";
import { isIpcError } from "../ipc/tauri";
import type { Correction, Corrections, DraftItem, PublishPlan } from "../ipc/types";
import {
  CORRECTION_STATE_LABELS,
  correctionAbout,
  correctionBadgeClass,
  correctionProblem,
  correctionRuleHelp,
  correctionStatusLine,
  today,
  unqueued,
} from "../publish";
import { PublicRepoDialog } from "./PublishPanel";

function clip(text: string, max: number): string {
  const flat = text.replace(/\s+/g, " ").trim();
  return flat.length <= max ? flat : `${flat.slice(0, max - 1).trimEnd()}…`;
}

/**
 * Corrections to a published meeting. The draft of a published meeting is locked: what was
 * published is what was approved. What changed afterwards is written here, kept with the meeting,
 * published as a new file in the meeting's folder (and as a comment on the issue of the task it
 * is about), and given to the next meeting as current information. A correction that was queued
 * is part of the record: it cannot be edited or deleted.
 */
export function CorrectionsPanel(props: {
  meetingId: string;
  cycleId: string;
  items: DraftItem[];
}) {
  const { meetingId, cycleId, items } = props;
  const backend = useBackend();
  const outbox = useOutbox();
  const [view, setView] = useState<Corrections | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notes, setNotes] = useState<string[]>([]);
  const [adding, setAdding] = useState(false);
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState<{ ids: string[]; plan: PublishPlan | null } | null>(
    null,
  );

  const load = useCallback(() => {
    return backend
      .listCorrections(meetingId)
      .then(setView)
      .catch((e) => setError(errorMessage(e)));
  }, [backend, meetingId]);

  // The states follow the outbox: the operations of a correction are outbox operations.
  useEffect(() => {
    void load();
  }, [load, outbox]);

  // The rule of the github target is part of the cycle: a change shows at once.
  useEffect(() => {
    let off: (() => void) | undefined;
    let cancelled = false;
    void backend
      .onCyclesChanged(() => void load())
      .then((u) => {
        if (cancelled) u();
        else off = u;
      });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [backend, load]);

  const run = (work: Promise<unknown>) => {
    setError(null);
    return work.then(() => load()).catch((e) => setError(errorMessage(e)));
  };

  const publish = (ids: string[]) => {
    setError(null);
    setNotes([]);
    setBusy(true);
    backend
      .publishCorrections(meetingId, ids.length === 0 ? undefined : ids)
      .then((r) => {
        setView(r.corrections);
        setNotes(r.notes);
      })
      .catch((e) => {
        if (isIpcError(e) && e.code === "public_repo_unconfirmed") {
          void backend
            .publishPlan(meetingId, ["github"])
            .then((plan) => setConfirming({ ids, plan }))
            .catch(() => setConfirming({ ids, plan: null }));
        } else {
          setError(errorMessage(e));
        }
      })
      .finally(() => setBusy(false));
  };

  const confirmAndPublish = () => {
    if (!confirming) return;
    const { ids } = confirming;
    setBusy(true);
    setError(null);
    backend
      .confirmPublicRepo(cycleId)
      .then(() => backend.publishCorrections(meetingId, ids.length === 0 ? undefined : ids))
      .then((r) => setView(r.corrections))
      .catch((e) => setError(errorMessage(e)))
      .finally(() => {
        setBusy(false);
        setConfirming(null);
      });
  };

  if (!view || !view.locked) return null;
  const list = view.corrections;
  const waiting = unqueued(list);
  const canPublish = view.inRepository && view.rule !== "off";

  return (
    <section className="panel quiet corrections-panel" aria-label="Corrections">
      <div className="panel-body">
        <div className="publish-head">
          <h3 className="sub-head tight">Corrections</h3>
          <span className="chip">
            {list.length === 0
              ? "none"
              : list.length === 1
                ? "1 correction"
                : `${list.length} corrections`}
          </span>
          {!adding && (
            <button className="btn btn-small" onClick={() => setAdding(true)}>
              Add correction
            </button>
          )}
        </div>
        <p className="section-hint">
          This draft is locked because the meeting was published, so it stays as it was approved. If
          something changed afterwards, add a correction. It is kept with the meeting, published as
          a new file in the meeting&apos;s folder (and as a comment on the issue, when it is about a
          task), and the next meeting is given it as current information.
        </p>
        <p className="target-rule-help">{correctionRuleHelp(view.rule)}</p>
        {!view.inRepository && (
          <p className="notice">
            This meeting is not in the repository, so its corrections are only kept on this
            computer. They still reach the next meeting.
          </p>
        )}
        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}
        {notes.map((n) => (
          <p key={n} className="notice">
            {n}
          </p>
        ))}

        {adding && (
          <CorrectionForm
            items={items}
            onCancel={() => setAdding(false)}
            onAdd={(text, item, issue, date) =>
              backend
                .addCorrection(
                  meetingId,
                  text,
                  item || undefined,
                  issue || undefined,
                  date || undefined,
                )
                .then((v) => {
                  setView(v);
                  setAdding(false);
                  return null;
                })
                .catch((e) => errorMessage(e))
            }
          />
        )}

        {list.length > 0 && (
          <ul className="corrections" aria-label="Corrections of this meeting">
            {list.map((c) => (
              <CorrectionRow
                key={c.id}
                c={c}
                rule={view.rule}
                canPublish={canPublish}
                busy={busy}
                onPublish={() => publish([c.id])}
                onDelete={() => void run(backend.deleteCorrection(meetingId, c.id))}
                onRetry={(op) => void run(backend.retryOutboxOp(op))}
                onCancel={(opIds) =>
                  void run(
                    opIds.reduce<Promise<unknown>>(
                      (done, id) => done.then(() => backend.cancelOutboxOp(id)),
                      Promise.resolve(),
                    ),
                  )
                }
              />
            ))}
          </ul>
        )}
        {waiting.length > 1 && canPublish && (
          <div className="actions publish-actions">
            <button className="btn btn-record" disabled={busy} onClick={() => publish([])}>
              Publish all {waiting.length}
            </button>
          </div>
        )}
      </div>
      {confirming && (
        <PublicRepoDialog
          visibility={confirming.plan?.repo.visibility ?? "public"}
          repo={confirming.plan?.repo.github ?? null}
          busy={busy}
          onCancel={() => setConfirming(null)}
          onConfirm={confirmAndPublish}
        />
      )}
    </section>
  );
}

function CorrectionForm(props: {
  items: DraftItem[];
  onCancel: () => void;
  /** Resolves with an error message, or null when the correction was added. */
  onAdd: (text: string, item: string, issue: string, date: string) => Promise<string | null>;
}) {
  const [text, setText] = useState("");
  const [item, setItem] = useState("");
  const [issue, setIssue] = useState("");
  const [date, setDate] = useState(today());
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const problem = correctionProblem(text, date);

  return (
    <form
      className="form correction-form"
      aria-label="New correction"
      onSubmit={(e) => {
        e.preventDefault();
        if (problem) return;
        setSaving(true);
        setError(null);
        void props.onAdd(text, item, issue, date).then((message) => {
          setSaving(false);
          setError(message);
        });
      }}
    >
      <label>
        What changed
        <textarea
          aria-label="Correction text"
          rows={3}
          maxLength={2000}
          value={text}
          autoFocus
          placeholder="For example: the order was placed on Friday, the delivery moves to 1 December."
          onChange={(e) => setText(e.target.value)}
        />
      </label>
      <div className="form-row">
        <label>
          About
          <select
            aria-label="Correction about"
            value={item}
            onChange={(e) => setItem(e.target.value)}
          >
            <option value="">Nothing in particular</option>
            {props.items.map((i) => (
              <option key={i.id} value={i.id}>
                {i.id}: {clip(i.text, 60)}
              </option>
            ))}
          </select>
        </label>
        <label>
          Issue (optional)
          <input
            aria-label="Correction issue"
            value={issue}
            placeholder="#12"
            onChange={(e) => setIssue(e.target.value)}
          />
        </label>
        <label>
          Date
          <input
            aria-label="Correction date"
            type="date"
            value={date}
            onChange={(e) => setDate(e.target.value)}
          />
        </label>
      </div>
      <small>
        An item with an issue is commented on that issue; name an issue here to comment on another
        one. The text becomes public together with the meeting.
      </small>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <div className="actions">
        <button className="btn btn-record" type="submit" disabled={saving || problem !== null}>
          Add correction
        </button>
        <button className="btn btn-ghost" type="button" onClick={props.onCancel}>
          Cancel
        </button>
      </div>
    </form>
  );
}

function CorrectionRow(props: {
  c: Correction;
  rule: Corrections["rule"];
  canPublish: boolean;
  busy: boolean;
  onPublish: () => void;
  onDelete: () => void;
  onRetry: (opId: number) => void;
  /** The operations to drop, the comment first (it follows the file). */
  onCancel: (opIds: number[]) => void;
}) {
  const { c } = props;
  const about = correctionAbout(c);
  const line = correctionStatusLine(c, props.rule);
  const waitingStep =
    c.state === "failed" || (c.state === "queued" && c.file?.lastError)
      ? c.file
      : c.comment?.state === "failed"
        ? c.comment
        : null;
  return (
    <li className={`correction correction-${c.state}`} aria-label={`Correction ${c.id}`}>
      <div className="op-main">
        <span className="op-label">
          <strong>{c.id}</strong> · {c.date}
          {about && <span className="person-meta"> · {about}</span>}
        </span>
        <span className={correctionBadgeClass(c.state)}>{CORRECTION_STATE_LABELS[c.state]}</span>
        {c.url && (
          <a href={c.url} target="_blank" rel="noreferrer noopener">
            {c.fileName ?? "correction"}
          </a>
        )}
      </div>
      <p className="correction-text">{c.text}</p>
      {line && (
        <p className={c.state === "failed" ? "op-line op-error" : "op-line"} role="status">
          {line}
        </p>
      )}
      <div className="op-actions">
        {c.state === "local" && props.canPublish && (
          <button
            className="btn btn-small"
            aria-label={`Publish correction ${c.id}`}
            disabled={props.busy}
            onClick={props.onPublish}
          >
            Publish
          </button>
        )}
        {waitingStep && (
          <button
            className="btn btn-small"
            aria-label={`Retry correction ${c.id} now`}
            onClick={() => props.onRetry(waitingStep.opId)}
          >
            Retry now
          </button>
        )}
        {c.file && c.file.state !== "done" && c.file.state !== "running" && (
          <button
            className="btn btn-small btn-ghost"
            aria-label={`Cancel correction ${c.id}`}
            onClick={() =>
              props.onCancel(
                [c.comment?.state === "done" ? null : c.comment?.opId, c.file!.opId].filter(
                  (id): id is number => id !== null && id !== undefined,
                ),
              )
            }
          >
            Cancel
          </button>
        )}
        {c.canDelete && (
          <button
            className="btn btn-small btn-ghost"
            aria-label={`Delete correction ${c.id}`}
            onClick={props.onDelete}
          >
            Delete
          </button>
        )}
      </div>
    </li>
  );
}
