import { useCallback, useEffect, useState } from "react";
import { useBackend } from "../backend-context";
import {
  dueLabel,
  groupHeading,
  isEmptyBriefing,
  sourceLabel,
  statusLine,
  summaryLine,
} from "../briefing";
import { errorMessage } from "../hooks";
import type { Briefing, BriefingTask } from "../ipc/types";

/**
 * The briefing of a cycle: before a meeting, who still has to do what and which topics are open.
 * The last known state shows at once; a cloned repository is then updated in the background and
 * the screen is replaced by the fresh state. Offline, it keeps the old one and says "State as of".
 */
export function BriefingView(props: { cycleId: string; onBack: () => void }) {
  const { cycleId, onBack } = props;
  const backend = useBackend();
  const [briefing, setBriefing] = useState<Briefing | null>(null);
  // The screen loads when it opens, so it starts out refreshing.
  const [refreshing, setRefreshing] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [round, setRound] = useState(0);

  // Without a pull first, so the screen is there at once, then with one.
  useEffect(() => {
    let cancelled = false;
    backend
      .getBriefing(cycleId, false)
      .then((b) => {
        if (!cancelled) setBriefing(b);
        return backend.getBriefing(cycleId, true);
      })
      .then((b) => {
        if (!cancelled) setBriefing(b);
      })
      .catch((e) => {
        if (!cancelled) setError(errorMessage(e));
      })
      .finally(() => {
        if (!cancelled) setRefreshing(false);
      });
    return () => {
      cancelled = true;
    };
  }, [backend, cycleId, round]);

  const refresh = useCallback(() => {
    setError(null);
    setRefreshing(true);
    setRound((r) => r + 1);
  }, []);
  const status = briefing ? statusLine(briefing, refreshing) : null;

  return (
    <>
      <div className="page-head">
        <h2>
          <span className="glitch" data-text="Briefing">
            Briefing
          </span>
        </h2>
        <span className="chip">{briefing ? `${briefing.cycleName} · ${cycleId}` : cycleId}</span>
      </div>
      <div className="card-actions brief-actions">
        <button className="btn btn-small" onClick={onBack}>
          Back to cycles
        </button>
        <button className="btn btn-small" disabled={refreshing} onClick={refresh}>
          {refreshing ? "Updating…" : "Update"}
        </button>
      </div>

      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {!briefing && !error && <p className="empty">Loading the briefing…</p>}

      {briefing && status && (
        <>
          <p
            className={status.tone === "stale" ? "brief-status brief-stale" : "brief-status"}
            role="status"
          >
            {status.text}
          </p>
          <p className="brief-summary">{summaryLine(briefing)}</p>
          {briefing.warnings.length > 0 && (
            <ul className="brief-warnings" aria-label="Warnings">
              {briefing.warnings.map((w) => (
                <li key={w}>{w}</li>
              ))}
            </ul>
          )}
          {isEmptyBriefing(briefing) && (
            <p className="empty">Nothing is open for this cycle: no open task and no open topic.</p>
          )}

          {briefing.groups.length > 0 && <h3 className="sub-head">Open tasks by person</h3>}
          <ul className="cards brief-groups" aria-label="Open tasks by person">
            {briefing.groups.map((g) => (
              <li key={`${g.kind}:${g.participantId ?? g.name}`}>
                <section className="panel" aria-label={`Tasks of ${g.name}`}>
                  <div className="panel-body">
                    <h4 className="card-title">
                      {groupHeading(g)}
                      <span className="badge badge-recovered">{g.tasks.length}</span>
                      {g.overdueCount > 0 && (
                        <span className="badge badge-job-failed">{g.overdueCount} overdue</span>
                      )}
                    </h4>
                    <TaskList tasks={g.tasks} today={briefing.today} label={`${g.name} tasks`} />
                  </div>
                </section>
              </li>
            ))}
          </ul>

          {briefing.topics.length > 0 && <h3 className="sub-head">Open topics</h3>}
          {briefing.topics.length > 0 && (
            <ul className="cycle-rows" aria-label="Open topics">
              {briefing.topics.map((t) => (
                <li key={t.id} className="cycle-row">
                  <div className="cycle-main">
                    <span className="cycle-name">{t.text}</span>
                    <span className="person-meta">Raised in the meeting of {t.openedIn}</span>
                  </div>
                </li>
              ))}
            </ul>
          )}

          {briefing.done.length > 0 && (
            <details className="brief-done">
              <summary>Done ({briefing.done.length})</summary>
              <TaskList tasks={briefing.done} today={briefing.today} label="Done tasks" done />
            </details>
          )}
        </>
      )}
    </>
  );
}

function TaskList(props: { tasks: BriefingTask[]; today: string; label: string; done?: boolean }) {
  const { tasks, today, label, done } = props;
  return (
    <ul className="cycle-rows brief-tasks" aria-label={label}>
      {tasks.map((t) => (
        <li key={t.id} className={t.overdue ? "cycle-row brief-overdue" : "cycle-row"}>
          <div className="cycle-main">
            <span className={done ? "cycle-name brief-done-text" : "cycle-name"}>{t.text}</span>
            <span className="person-meta">
              {done ? sourceLabel(t) : `${dueLabel(t, today)} · ${sourceLabel(t)}`}
            </span>
          </div>
        </li>
      ))}
    </ul>
  );
}
