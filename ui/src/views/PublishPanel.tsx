import { useCallback, useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { useBackend } from "../backend-context";
import { errorMessage, useCycleFacts, useOutbox } from "../hooks";
import { isIpcError } from "../ipc/tauri";
import type {
  OutboxOp,
  PublishMode,
  PublishPlan,
  PublishTargetName,
  PublishTargetView,
  Topic,
  TopicsPlan,
} from "../ipc/types";
import {
  OP_STATE_LABELS,
  OP_TARGET_LABELS,
  STATUS_WRITE_BACK_NOTE,
  TARGET_LABELS,
  canCancel,
  canRetry,
  chosenTargets,
  committedOp,
  conflictRows,
  isCorrectionOp,
  lockedByOps,
  offlineText,
  opBadgeClass,
  opLink,
  opStatusLine,
  opsOf,
  otherBlocks,
  outboxHeadline,
  pendingStatusNote,
  planLines,
  plannedTopicLine,
  planTopicsSummary,
  publicRepoBlock,
  repoWarning,
  targetsSent,
  topicDifference,
  topicLine,
  topicsSummary,
} from "../publish";
import { Modal } from "./Modal";
import { RuleChoice } from "./RuleChoice";

/**
 * Publishing a meeting: where it goes and by which rule, what exactly will happen, the Publish
 * button, and the outbox of what was queued with its retries. The outbox is the truth about
 * what was sent: a failed or waiting operation can be retried or cancelled, and cancelling all of
 * them gives the draft back.
 */
export function PublishPanel(props: {
  meetingId: string;
  cycleId: string;
  /** Changes whenever the draft does, so the plan is made again. */
  draftVersion: unknown;
  /** A newer draft waits for a decision. */
  hasNewDraft: boolean;
}) {
  const { meetingId, cycleId, draftVersion, hasNewDraft } = props;
  const backend = useBackend();
  const outbox = useOutbox();
  const { topics, reload: reloadFacts } = useCycleFacts(cycleId);
  const [plan, setPlan] = useState<PublishPlan | null>(null);
  const [planError, setPlanError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [ticked, setTicked] = useState<Record<PublishTargetName, boolean>>({
    github: true,
    discord: true,
    notion: true,
  });
  const [confirming, setConfirming] = useState(false);
  const [ruleBusy, setRuleBusy] = useState(false);

  const allOps = useMemo(() => opsOf(outbox, meetingId), [outbox, meetingId]);
  // The operations of corrections have their own panel; here is the publication of the meeting.
  const ops = useMemo(() => allOps.filter((o) => !isCorrectionOp(o)), [allOps]);
  const locked = lockedByOps(allOps);

  const loadPlan = useCallback(() => {
    return backend
      .publishPlan(meetingId)
      .then((p) => {
        setPlanError(null);
        setPlan(p);
      })
      .catch((e) => {
        setPlan(null);
        setPlanError(errorMessage(e));
      });
  }, [backend, meetingId]);

  // The plan follows the draft (approving a status change changes what happens).
  useEffect(() => {
    void loadPlan();
  }, [loadPlan, draftVersion]);

  const publish = (targets: PublishTargetName[]) => {
    setError(null);
    setBusy(true);
    backend
      .publishMeeting(meetingId, targets)
      .then((r) => setPlan(r.plan))
      .catch((e) => {
        if (isIpcError(e) && e.code === "public_repo_unconfirmed") {
          void loadPlan().then(() => setConfirming(true));
        } else {
          setError(errorMessage(e));
        }
      })
      .finally(() => setBusy(false));
  };

  const confirmAndPublish = (targets: PublishTargetName[]) => {
    setError(null);
    setBusy(true);
    backend
      .confirmPublicRepo(cycleId)
      .then(() => backend.publishMeeting(meetingId, targets))
      .then((r) => setPlan(r.plan))
      .catch((e) => setError(errorMessage(e)))
      .finally(() => {
        setBusy(false);
        setConfirming(false);
      });
  };

  const act = (work: Promise<unknown>) => {
    setError(null);
    work.then(() => loadPlan()).catch((e) => setError(errorMessage(e)));
  };

  const setRule = (target: PublishTargetName, mode: PublishMode) => {
    setError(null);
    setRuleBusy(true);
    backend
      .setTargetRule(cycleId, target, mode)
      .then(() => loadPlan())
      .catch((e) => setError(errorMessage(e)))
      .finally(() => setRuleBusy(false));
  };

  // The topics change when the commit is done.
  const commitId = committedOp(ops)?.id ?? null;
  useEffect(() => {
    if (commitId !== null) reloadFacts();
  }, [commitId, reloadFacts]);

  const targets = chosenTargets(plan, ticked).filter((t) => t !== "notion");
  const publicBlock = publicRepoBlock(plan);
  const needsConfirm = publicBlock !== null && targets.includes("github");
  const blockers = otherBlocks(plan);
  const lines = plan ? planLines(plan, topics) : [];
  const offline = offlineText(outbox);
  const pending = pendingStatusNote(plan);
  const waiting = ops.some((o) => canRetry(o));
  // Publishing again is idempotent, but there is nothing to do when everything chosen was sent.
  const sent = targetsSent(ops);
  const allSent = targets.length > 0 && targets.every((t) => sent.has(t));
  const canPublish =
    plan !== null &&
    targets.length > 0 &&
    !allSent &&
    !busy &&
    !hasNewDraft &&
    blockers.length === 0;

  return (
    <section className="panel quiet publish-panel" aria-label="Publishing">
      <div className="panel-body">
        <div className="publish-head">
          <h3 className="sub-head tight">Publishing</h3>
          {plan?.repo.github && (
            <span className="chip" title="The GitHub repository the report goes to">
              {plan.repo.github} · {plan.repo.visibility}
            </span>
          )}
          {locked && <span className="chip chip-locked">Draft locked</span>}
        </div>

        {offline && (
          <div className="banner banner-offline" role="status">
            <span>{offline}</span>
          </div>
        )}
        {hasNewDraft && (
          <p className="notice">
            A newer draft waits for a decision. Accept it or keep yours, then publish.
          </p>
        )}
        {planError && (
          <p className="error" role="alert">
            {planError}
          </p>
        )}
        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}

        {plan && (
          <ul className="targets-list" aria-label="Targets">
            {/* Notion has no row until the Stage 7 UI is ported (WP-14c). */}
            {plan.targets
              .filter((t) => t.target !== "notion")
              .map((t) => (
                <TargetRow
                  key={t.target}
                  view={t}
                  ticked={ticked[t.target]}
                  onTick={(v) => setTicked({ ...ticked, [t.target]: v })}
                  onRule={(rule) => setRule(t.target, rule)}
                  ruleBusy={ruleBusy}
                />
              ))}
          </ul>
        )}

        {plan && (
          <>
            <h4 className="sub-head tight">
              {allSent ? "What was published" : "What will happen"}
            </h4>
            {lines.length === 0 ? (
              <p className="empty">Nothing: there is nothing to publish with these rules.</p>
            ) : (
              <ul
                className="plan-lines"
                aria-label={allSent ? "What was published" : "What will happen"}
              >
                {lines.map((l, i) => (
                  <li key={i}>{l}</li>
                ))}
              </ul>
            )}
            {/* Once the commit is done the result below shows what it changed. */}
            {!committedOp(ops) && <TopicsPlanView plan={plan.summary.topics ?? null} />}
            {pending && <p className="section-hint">{pending}</p>}
            {plan.notes.map((n, i) => (
              <p key={i} className="notice">
                {n}
              </p>
            ))}
            {blockers.map((b) => (
              <p key={b.code} className="error" role="alert">
                {b.message}
              </p>
            ))}
            {publicBlock && needsConfirm && !locked && (
              <p className="notice notice-warn">
                {repoWarning(publicBlock.visibility, plan.repo.github)} You confirm this once, when
                you press Publish.
              </p>
            )}
          </>
        )}

        <div className="actions publish-actions">
          <button
            className="btn btn-record"
            disabled={!canPublish}
            onClick={() => (needsConfirm ? setConfirming(true) : publish(targets))}
          >
            Publish
          </button>
        </div>

        {ops.length > 0 && (
          <Outbox
            ops={ops}
            now={outbox?.nowUnixMs ?? 0}
            offline={(outbox?.offline.length ?? 0) > 0}
            canRetryAny={waiting}
            onRetry={(id) => act(backend.retryOutboxOp(id))}
            onCancel={(id) => act(backend.cancelOutboxOp(id))}
          />
        )}

        <TopicsDiff ops={ops} topics={topics} />
      </div>

      {confirming && plan && (
        <PublicRepoDialog
          visibility={publicBlock?.visibility ?? "public"}
          repo={plan.repo.github}
          busy={busy}
          onCancel={() => setConfirming(false)}
          onConfirm={() => confirmAndPublish(targets.length > 0 ? targets : ["github"])}
        />
      )}
    </section>
  );
}

function TargetRow(props: {
  view: PublishTargetView;
  ticked: boolean;
  onTick: (v: boolean) => void;
  onRule: (rule: PublishMode) => void;
  ruleBusy: boolean;
}) {
  const { view } = props;
  const name = view.target === "github" ? "GitHub" : "Discord";
  return (
    <li className="target-row">
      <label className="check">
        <input
          type="checkbox"
          checked={view.available && props.ticked}
          disabled={!view.available}
          onChange={(e) => props.onTick(e.target.checked)}
        />
        <span>
          Publish to {name}
          <small className="target-what">{TARGET_LABELS[view.target]}</small>
        </span>
      </label>
      {view.mode === null ? (
        <p className="target-rule-help">
          The project has no Discord target. Add one in the project settings, under Integrations.
        </p>
      ) : (
        <>
          <RuleChoice
            name={name}
            target={view.target}
            value={view.mode}
            disabled={props.ruleBusy}
            onChange={props.onRule}
          />
          {view.target === "github" && (
            <span className="target-rule-help">{STATUS_WRITE_BACK_NOTE}</span>
          )}
        </>
      )}
    </li>
  );
}

function Outbox(props: {
  ops: OutboxOp[];
  now: number;
  offline: boolean;
  canRetryAny: boolean;
  onRetry: (id?: number) => void;
  onCancel: (id: number) => void;
}) {
  const { ops } = props;
  return (
    <div className="outbox">
      <div className="outbox-head">
        <h4 className="sub-head tight">Outbox</h4>
        <span className="outbox-headline" role="status">
          {outboxHeadline(ops)}
        </span>
        {props.canRetryAny && (
          <button className="btn btn-small" onClick={() => props.onRetry(undefined)}>
            Retry all now
          </button>
        )}
      </div>
      <ul className="ops" aria-label="Outbox">
        {ops.map((op) => (
          <OpRow key={op.id} op={op} {...props} />
        ))}
      </ul>
    </div>
  );
}

function OpRow(props: {
  op: OutboxOp;
  now: number;
  offline: boolean;
  onRetry: (id?: number) => void;
  onCancel: (id: number) => void;
}) {
  const { op } = props;
  const link = opLink(op);
  const line = opStatusLine(op, props.now, props.offline);
  return (
    <li className={`op op-${op.state}`}>
      <div className="op-main">
        <span className="op-label">{op.label}</span>
        <span className="chip">{OP_TARGET_LABELS[op.target]}</span>
        <span className={opBadgeClass(op.state)}>{OP_STATE_LABELS[op.state]}</span>
        {op.attempts > 0 && op.state !== "done" && (
          <span className="person-meta">
            {op.attempts === 1 ? "1 attempt" : `${op.attempts} attempts`}
          </span>
        )}
        {link && (
          <a href={link.href} target="_blank" rel="noreferrer noopener">
            {link.text}
          </a>
        )}
      </div>
      {line && (
        <p className={op.state === "failed" ? "op-line op-error" : "op-line"} role="status">
          {line}
        </p>
      )}
      {(canRetry(op) || canCancel(op)) && (
        <div className="op-actions">
          {canRetry(op) && (
            <button
              className="btn btn-small"
              aria-label={`Retry now: ${op.label}`}
              onClick={() => props.onRetry(op.id)}
            >
              Retry now
            </button>
          )}
          {canCancel(op) && (
            <button
              className="btn btn-small btn-ghost"
              aria-label={`Cancel: ${op.label}`}
              onClick={() => props.onCancel(op.id)}
            >
              Cancel
            </button>
          )}
        </div>
      )}
    </li>
  );
}

/**
 * `topics.yaml` before and after the commit, per topic, before anything is published: what the
 * remote has (as of the last fetch) against what the commit will write. A topic the remote
 * changed since the last pull and this meeting changes too is marked: the last write wins, so
 * the remote's version is replaced.
 */
function TopicsPlanView(props: { plan: TopicsPlan | null }) {
  const { plan } = props;
  if (!plan || plan.changes.length === 0) return null;
  const others =
    plan.unchanged === 0
      ? ""
      : `; ${plan.unchanged === 1 ? "1 other topic stays" : `${plan.unchanged} other topics stay`} as it is`;
  return (
    <div className="topics-diff topics-plan">
      <h4 className="sub-head tight">topics.yaml: before and after</h4>
      <p className="section-hint">
        {planTopicsSummary(plan)}
        {others}.{" "}
        {plan.againstRemote
          ? "Compared with the remote as of the last time it was fetched."
          : "Compared with this clone only."}
      </p>
      <ul className="conflicts" aria-label="Planned topic changes">
        {plan.changes.map((c) => (
          <li
            key={c.id}
            className={c.overridesRemote ? "conflict" : "conflict conflict-plain"}
            aria-label={`Topic ${c.id} ${c.change}`}
          >
            <p className="conflict-id">
              Topic {c.id}
              <span className="badge badge-job-queued">{c.change}</span>
              {c.change === "changed" &&
                topicDifference(c.before, c.after).map((d) => (
                  <span key={d} className="badge badge-job-queued">
                    {d} differs
                  </span>
                ))}
            </p>
            {c.before && (
              <p className="diff diff-del">
                <span className="diff-label">On the remote</span>
                {"− "}
                {plannedTopicLine(c.before)}
              </p>
            )}
            {c.after && (
              <p className="diff diff-add">
                <span className="diff-label">After the commit</span>
                {"+ "}
                {plannedTopicLine(c.after)}
              </p>
            )}
            {c.overridesRemote && (
              <p className="notice notice-warn" role="status">
                Someone else changed this topic since your last pull. The last write wins: the
                remote version above is replaced.
              </p>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}

/**
 * The change of `topics.yaml` the commit made. Where a colleague changed the same topic, the rule
 * is last write wins per topic: this meeting's version was kept, and both versions are shown so
 * the difference is not lost.
 */
function TopicsDiff(props: { ops: OutboxOp[]; topics: Topic[] }) {
  const commit = committedOp(props.ops);
  if (!commit) return null;
  const rows = conflictRows(commit.result, props.topics);
  const summary = topicsSummary(commit.result);
  if (!summary && rows.length === 0) return null;
  return (
    <div className="topics-diff">
      <h4 className="sub-head tight">topics.yaml</h4>
      {summary && <p className="section-hint">Changed in the commit: {summary}.</p>}
      {rows.length > 0 && (
        <>
          <p className="notice notice-warn" role="status">
            {rows.length === 1
              ? "1 topic was also changed by someone else since your last pull."
              : `${rows.length} topics were also changed by someone else since your last pull.`}{" "}
            The last write wins per topic: your version was kept.
          </p>
          <ul className="conflicts" aria-label="Topic conflicts">
            {rows.map((r) => (
              <li key={r.id} className="conflict" aria-label={`Conflict in topic ${r.id}`}>
                <p className="conflict-id">
                  Topic {r.id}
                  {!r.partial &&
                    topicDifference(r.theirs, r.ours).map((d) => (
                      <span key={d} className="badge badge-job-queued">
                        {d} differs
                      </span>
                    ))}
                </p>
                {r.partial ? (
                  <p className="diff diff-add">+ {topicLine(r.ours)}</p>
                ) : (
                  <>
                    <p className="diff diff-del">
                      <span className="diff-label">On the remote (replaced)</span>
                      {"− "}
                      {topicLine(r.theirs)}
                    </p>
                    <p className="diff diff-add">
                      <span className="diff-label">Yours (kept)</span>
                      {"+ "}
                      {topicLine(r.ours)}
                    </p>
                  </>
                )}
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}

/** The warning before the first publication to a public repository. It has to be confirmed. */
export function PublicRepoDialog(props: {
  visibility: string;
  repo: string | null;
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const [understood, setUnderstood] = useState(false);
  const name = props.repo ?? "this repository";
  // In the body, not in the panel: a panel clips what is inside it to its chamfered outline.
  return createPortal(
    <Modal title="Publish to a public repository?" onClose={props.onCancel} wide>
      <p className="modal-text modal-strong">{repoWarning(props.visibility, props.repo)}</p>
      <p className="modal-text">
        The report, the transcript and the screenshots become readable by anyone and stay in the
        history of the repository, even if you delete them later. Issues and Discord messages notify
        the people they mention and cannot be unsent. If the meeting was not meant for the public,
        cancel and publish to a private repository.
      </p>
      <label className="check">
        <input
          type="checkbox"
          checked={understood}
          onChange={(e) => setUnderstood(e.target.checked)}
        />
        <span>I understand that {name} is public and I want to publish to it.</span>
      </label>
      <p className="modal-text">
        This is asked once for this repository. It is remembered until you remove the repository.
      </p>
      <div className="actions">
        <button className="btn btn-ghost" autoFocus onClick={props.onCancel}>
          Cancel
        </button>
        <button
          className="btn btn-record"
          disabled={!understood || props.busy}
          onClick={props.onConfirm}
        >
          Publish to a public repository
        </button>
      </div>
    </Modal>,
    document.body,
  );
}
