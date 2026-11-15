import { useState } from "react";
import { formatClock, matchPercent } from "../draft";
import type {
  CycleTasks,
  ProposedStatus,
  StatusDecision,
  StatusProposal,
  Topic,
} from "../ipc/types";
import {
  addableTargets,
  countProposals,
  decisionLabel,
  issueNumber,
  issueUrl,
  proposalEffect,
  statusVerb,
  taskOfTarget,
} from "../publish";

/**
 * The status changes the model found in the meeting: an open issue or topic that was said to be
 * done. Each one shows the quote it rests on and what approving does; nothing is written back
 * before it is approved, and nothing is ever approved by itself. Once the meeting is published
 * the decisions are read-only.
 */
export function StatusProposals(props: {
  proposals: StatusProposal[];
  locked: boolean;
  tasks: CycleTasks | null;
  topics: Topic[];
  transcriptPresent: boolean;
  /** Runs a change that answers with the new draft view. */
  onDecide: (proposalId: string, decision: StatusDecision, comment?: string) => Promise<boolean>;
  onAdd: (target: string, status: ProposedStatus, comment: string) => Promise<boolean>;
  onJump: (ms: number) => void;
}) {
  const { proposals, locked, tasks, topics } = props;
  const counts = countProposals(proposals);
  const options = addableTargets(proposals, tasks, topics);
  const [adding, setAdding] = useState(false);

  return (
    <section className="panel quiet" aria-label="Status changes">
      <div className="panel-body">
        <h3 className="sub-head tight">Status changes</h3>
        <p className="section-hint">
          Places where an open issue or topic was said to be done. Nothing is written back before
          you approve it, and the model never approves anything by itself.
        </p>
        {proposals.length > 0 && (
          <p className="status-count" role="status">
            {counts.approved} of {proposals.length} approved
            {counts.rejected > 0 && `, ${counts.rejected} rejected`}
            {counts.proposed > 0 && `, ${counts.proposed} waiting for your decision`}
          </p>
        )}
        {locked && (
          <p className="notice">
            The decisions are read-only: the meeting is published or queued for publishing.
          </p>
        )}
        {proposals.length === 0 && <p className="empty">The model found no status changes.</p>}
        <ul className="proposals" aria-label="Status change proposals">
          {proposals.map((p) => (
            <ProposalCard key={`${p.id}:${p.comment ?? ""}`} {...props} proposal={p} />
          ))}
        </ul>
        {!locked && options.length > 0 && !adding && (
          <button className="btn btn-small btn-ghost" onClick={() => setAdding(true)}>
            + Add a status change
          </button>
        )}
        {!locked && adding && (
          <AddProposalForm
            options={options}
            onAdd={async (target, status, comment) => {
              const ok = await props.onAdd(target, status, comment);
              if (ok) setAdding(false);
            }}
            onCancel={() => setAdding(false)}
          />
        )}
      </div>
    </section>
  );
}

function ProposalCard(props: {
  proposal: StatusProposal;
  locked: boolean;
  tasks: CycleTasks | null;
  transcriptPresent: boolean;
  onDecide: (proposalId: string, decision: StatusDecision, comment?: string) => Promise<boolean>;
  onJump: (ms: number) => void;
}) {
  const { proposal: p, locked, tasks } = props;
  const [comment, setComment] = useState(p.comment ?? "");
  const number = issueNumber(p.target);
  const task = taskOfTarget(tasks, p.target);
  const href = number === null || !tasks?.repo ? null : (task?.url ?? issueUrl(tasks.repo, number));
  const unverified = p.source !== null && !p.source.verified;
  const saveComment = () => {
    if (locked || comment.trim() === (p.comment ?? "")) return;
    void props.onDecide(p.id, p.decision, comment);
  };
  const decide = (decision: StatusDecision) =>
    // Pressing the decision that is set takes it back.
    void props.onDecide(p.id, p.decision === decision ? "proposed" : decision, comment);

  return (
    <li className={`proposal proposal-${p.decision}`} aria-label={`Status change ${p.target}`}>
      <div className="proposal-head">
        <span className={`badge ${p.status === "done" ? "badge-job-done" : "badge-job-queued"}`}>
          {p.status === "done" ? "Done" : "Reopen"}
        </span>
        {href ? (
          <a className="proposal-target" href={href} target="_blank" rel="noreferrer noopener">
            {p.target}
          </a>
        ) : (
          <span className="proposal-target">{p.target}</span>
        )}
        {task && <span className="proposal-title">{task.title}</span>}
        {p.edited && (
          <span
            className="edited-dot"
            role="img"
            aria-label="Edited by you"
            title="Edited by you"
          />
        )}
        <span className={`badge badge-decision-${p.decision}`}>{decisionLabel(p)}</span>
      </div>
      <p className="proposal-reason">{p.reason}</p>
      {p.source ? (
        <>
          <blockquote className="proposal-quote">{p.source.quote}</blockquote>
          <p className="proposal-meta">
            {p.source.speaker} · {formatClock(p.source.tStartMs)} · match{" "}
            {matchPercent(p.source.match)}
            {unverified && (
              <span
                className="mark mark-unverified"
                title="The quote was not found in the transcript"
              >
                {" "}
                unverified quote
              </span>
            )}
            {props.transcriptPresent && (
              <>
                {" "}
                <button
                  className="btn btn-small btn-ghost"
                  aria-label={`Jump to the quote of ${p.target}`}
                  onClick={() => props.onJump(p.source!.tStartMs)}
                >
                  Jump to transcript
                </button>
              </>
            )}
          </p>
        </>
      ) : (
        <p className="proposal-meta">Added by hand: there is no quote.</p>
      )}
      <p className="proposal-effect">{proposalEffect(p)}</p>
      <label className="proposal-comment">
        Comment for {p.target}
        <textarea
          rows={2}
          value={comment}
          disabled={locked}
          placeholder="Posted with the change (optional)"
          onChange={(e) => setComment(e.target.value)}
          onBlur={saveComment}
        />
      </label>
      <div className="proposal-actions">
        <button
          className={p.decision === "approved" ? "btn btn-small btn-stop" : "btn btn-small"}
          aria-pressed={p.decision === "approved"}
          aria-label={`Approve ${p.target}`}
          disabled={locked}
          onClick={() => decide("approved")}
        >
          {p.decision === "approved" ? "Approved" : "Approve"}
        </button>
        <button
          className="btn btn-small btn-ghost"
          aria-pressed={p.decision === "rejected"}
          aria-label={`Reject ${p.target}`}
          disabled={locked}
          onClick={() => decide("rejected")}
        >
          {p.decision === "rejected" ? "Rejected" : "Reject"}
        </button>
        <span className="proposal-verb">{statusVerb(p.status)}</span>
      </div>
    </li>
  );
}

function AddProposalForm(props: {
  options: { value: string; label: string }[];
  onAdd: (target: string, status: ProposedStatus, comment: string) => Promise<void>;
  onCancel: () => void;
}) {
  const [target, setTarget] = useState(props.options[0]?.value ?? "");
  const [status, setStatus] = useState<ProposedStatus>("done");
  const [comment, setComment] = useState("");
  return (
    <form
      className="add-form proposal-add"
      aria-label="Add a status change"
      onSubmit={(e) => {
        e.preventDefault();
        if (target !== "") void props.onAdd(target, status, comment);
      }}
      onKeyDown={(e) => e.key === "Escape" && props.onCancel()}
    >
      <select
        aria-label="Issue or topic"
        value={target}
        autoFocus
        onChange={(e) => setTarget(e.target.value)}
      >
        {props.options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
      <select
        aria-label="New status"
        value={status}
        onChange={(e) => setStatus(e.target.value as ProposedStatus)}
      >
        <option value="done">Done</option>
        <option value="open">Reopen</option>
      </select>
      <input
        aria-label="Comment of the new status change"
        value={comment}
        placeholder="Comment (optional)"
        onChange={(e) => setComment(e.target.value)}
      />
      <button className="btn btn-small" type="submit" disabled={target === ""}>
        Add
      </button>
      <button className="btn btn-small btn-ghost" type="button" onClick={props.onCancel}>
        Cancel
      </button>
    </form>
  );
}
