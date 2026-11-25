import { useCallback, useEffect, useMemo, useState } from "react";
import { useBackend } from "../backend-context";
import { dueLabel, plural, sourceLabel, statusLine, summaryLine } from "../briefing";
import { formatDateTimeUtc, formatDuration } from "../format";
import { errorMessage, useCycleFacts, useMeetings, useRecordingState } from "../hooks";
import type { Briefing, BriefingTask, CycleDetails, CycleTask, MeetingSummary } from "../ipc/types";
import { canProcess, jobStateLabel } from "../processing";
import { taskCounts, tasksAsOf } from "../publish";
import { CycleTasksView } from "./CycleTasks";
import { ImportMeetingModal } from "./ImportMeetingModal";
import { ProjectSettingsModal } from "./ProjectSettingsModal";

export type MeetingWithCycle = MeetingSummary;

interface TaskWithOwner {
  task: BriefingTask;
  assignee: string;
}

/** Allow only http/https URLs before rendering external anchor hrefs. */
export function isSafeHttpUrl(url: string | null | undefined): url is string {
  if (!url) return false;
  try {
    const parsed = new URL(url);
    return parsed.protocol === "https:" || parsed.protocol === "http:";
  } catch {
    return false;
  }
}

/**
 * Resolve each meeting's `cycleId` from `MeetingSummary.cycleId` when provided, falling back to
 * draft/voice metadata or session-local import records when older backends omit it.
 */
export function useMeetingCycles(
  meetings: MeetingSummary[],
  localImported?: ReadonlyMap<string, string>,
): { cycleMap: Record<string, string | null>; resolved: boolean } {
  const backend = useBackend();
  const [fallbackMap, setFallbackMap] = useState<Record<string, string | null>>({});
  const [resolvedIdsKey, setResolvedIdsKey] = useState("");

  const unresolvedIds = useMemo(
    () =>
      meetings.filter((m) => !localImported?.has(m.id) && m.cycleId === undefined).map((m) => m.id),
    [meetings, localImported],
  );

  const key = unresolvedIds.join(",");

  useEffect(() => {
    if (unresolvedIds.length === 0) return;
    let cancelled = false;
    void Promise.all(
      unresolvedIds.map(async (id): Promise<[string, string | null]> => {
        try {
          const d = await backend.getDraft(id);
          return [id, d.draft.cycle || null];
        } catch {
          // Draft may not exist yet for queued or standalone meetings.
        }
        try {
          const v = await backend.getMeetingVoices(id);
          const cid = v.participants.find((p) => p.cycleId)?.cycleId ?? null;
          return [id, cid];
        } catch {
          return [id, null];
        }
      }),
    ).then((entries) => {
      if (cancelled) return;
      setFallbackMap(Object.fromEntries(entries));
      setResolvedIdsKey(key);
    });
    return () => {
      cancelled = true;
    };
  }, [backend, key, unresolvedIds]);

  const cycleMap = useMemo(() => {
    const out: Record<string, string | null> = {};
    for (const m of meetings) {
      if (localImported?.has(m.id)) {
        out[m.id] = localImported.get(m.id) ?? null;
        continue;
      }
      if (m.cycleId !== undefined) {
        out[m.id] = m.cycleId;
        continue;
      }
      out[m.id] = fallbackMap[m.id] ?? null;
    }
    return out;
  }, [meetings, localImported, fallbackMap]);

  const resolved = unresolvedIds.length === 0 || resolvedIdsKey === key;
  return { cycleMap, resolved };
}

function MeetingBadges({ meeting }: { meeting: MeetingSummary }) {
  const stateLabel =
    meeting.state === "stopped"
      ? null
      : {
          recording: "Recording",
          recoverable: "Needs recovery",
          recovered: "Recovered",
        }[meeting.state];
  return (
    <>
      {stateLabel && <span className={`badge badge-${meeting.state}`}>{stateLabel}</span>}
      {meeting.job && (
        <span className={`badge badge-job-${meeting.job}`}>{jobStateLabel(meeting.job)}</span>
      )}
    </>
  );
}

/** Shared meeting card/row used in ProjectDetailView and standalone meetings in ProjectsView. */
export function MeetingRowCard(props: {
  meeting: MeetingSummary;
  recordingMeetingId: string | null;
  onOpen: (meetingId: string) => void;
  onProcess: (meetingId: string) => void;
  onRecover: (meetingId: string) => void;
  onDelete: (meetingId: string) => void;
}) {
  const { meeting: m, recordingMeetingId, onOpen, onProcess, onRecover, onDelete } = props;
  const headline = m.shortTitle?.trim() || m.title;
  const showSubtitle = Boolean(m.shortTitle?.trim() && m.shortTitle.trim() !== m.title);
  const safeUrl = isSafeHttpUrl(m.reportUrl) ? m.reportUrl : null;

  return (
    <li className="cycle-row">
      <div className="cycle-main">
        <span className="cycle-name">
          <button
            type="button"
            className="seg-text"
            style={{ fontWeight: 700, textAlign: "left" }}
            onClick={() => onOpen(m.id)}
          >
            {headline}
          </button>{" "}
          <MeetingBadges meeting={m} />
        </span>
        <span className="person-meta">
          {formatDateTimeUtc(m.startedUnixMs)} UTC
          {m.durationMs !== null && ` · ${formatDuration(m.durationMs)}`}
          {` · ${plural(m.trackCount, "track", "tracks")}`}
          {m.screenshotCount > 0 && ` · ${plural(m.screenshotCount, "slide/shot", "slides/shots")}`}
          {showSubtitle && ` · ${m.title}`}
        </span>
      </div>

      <div className="card-actions" style={{ marginTop: 0 }}>
        {safeUrl && (
          <a
            className="btn btn-small"
            href={safeUrl}
            target="_blank"
            rel="noopener noreferrer"
            onClick={(e) => e.stopPropagation()}
          >
            View on GitHub
          </a>
        )}
        <button className="btn btn-small" type="button" onClick={() => onOpen(m.id)}>
          Open
        </button>
        {m.state === "recoverable" && (
          <button className="btn btn-small btn-stop" type="button" onClick={() => onRecover(m.id)}>
            Recover
          </button>
        )}
        <button
          className="btn btn-small btn-stop"
          type="button"
          disabled={!canProcess(m, recordingMeetingId)}
          onClick={() => onProcess(m.id)}
        >
          Process
        </button>
        <button className="btn btn-small btn-ghost" type="button" onClick={() => onDelete(m.id)}>
          Delete
        </button>
      </div>
    </li>
  );
}

function sortTasksByUrgency(briefing: Briefing): TaskWithOwner[] {
  const items: TaskWithOwner[] = [];
  for (const g of briefing.groups) {
    for (const task of g.tasks) {
      items.push({ task, assignee: g.name });
    }
  }
  return items.sort((a, b) => {
    if (a.task.overdue !== b.task.overdue) return a.task.overdue ? -1 : 1;
    if (a.task.due !== b.task.due) {
      if (a.task.due === null) return 1;
      if (b.task.due === null) return -1;
      return a.task.due.localeCompare(b.task.due);
    }
    return a.task.text.localeCompare(b.task.text);
  });
}

function TaskRow({ item, today, done }: { item: TaskWithOwner; today: string; done?: boolean }) {
  const { task, assignee } = item;
  const safeUrl = isSafeHttpUrl(task.url) ? task.url : null;
  return (
    <li className={task.overdue && !done ? "cycle-row brief-overdue" : "cycle-row"}>
      <div className="cycle-main">
        <span className={done ? "cycle-name brief-done-text" : "cycle-name"}>{task.text}</span>
        <span className="person-meta">
          {assignee}
          {" · "}
          {done ? sourceLabel(task) : `${dueLabel(task, today)} · ${sourceLabel(task)}`}
        </span>
      </div>
      {safeUrl && (
        <a
          className="btn btn-small btn-ghost"
          href={safeUrl}
          target="_blank"
          rel="noopener noreferrer"
        >
          Issue
        </a>
      )}
    </li>
  );
}

function CycleIssueRow({ task, done }: { task: CycleTask; done: boolean }) {
  const safeUrl = isSafeHttpUrl(task.url) ? task.url : null;
  return (
    <li className="cycle-row">
      <div className="cycle-main">
        <span className={done ? "cycle-name brief-done-text" : "cycle-name"}>
          <span className={done ? "badge badge-job-done" : "badge badge-job-running"}>
            {done ? "Done" : "Open"}
          </span>{" "}
          #{task.number} {task.title}
        </span>
        {(task.assignee || task.meeting) && (
          <span className="person-meta">
            {[task.assignee, task.meeting ? `Meeting ${task.meeting}` : null]
              .filter(Boolean)
              .join(" · ")}
          </span>
        )}
      </div>
      {safeUrl && (
        <a
          className="btn btn-small btn-ghost"
          href={safeUrl}
          target="_blank"
          rel="noopener noreferrer"
        >
          Issue
        </a>
      )}
    </li>
  );
}

export interface ProjectDetailViewProps {
  cycleId: string;
  onBack: () => void;
  onOpenMeeting: (meetingId: string) => void;
}

/** Project workspace showing upcoming discussion topics, urgent tasks, and project meetings. */
export function ProjectDetailView({ cycleId, onBack, onOpenMeeting }: ProjectDetailViewProps) {
  const backend = useBackend();
  const { meetings, reload: reloadMeetings } = useMeetings();
  const { tasks, reload: reloadTasks } = useCycleFacts(cycleId);
  const recording = useRecordingState();

  const [details, setDetails] = useState<CycleDetails | null>(null);
  const [briefing, setBriefing] = useState<Briefing | null>(null);
  const [refreshing, setRefreshing] = useState(true);
  const [showAllTasks, setShowAllTasks] = useState(false);
  const [syncingIssues, setSyncingIssues] = useState(false);
  const [issueSyncNote, setIssueSyncNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [importOpen, setImportOpen] = useState(false);
  const [importedMap, setImportedMap] = useState<Map<string, string>>(() => new Map());

  const { cycleMap } = useMeetingCycles(meetings, importedMap);

  const loadCycleDetails = useCallback(() => {
    void backend
      .getCycle(cycleId)
      .then(setDetails)
      .catch((e) => setError(errorMessage(e)));
  }, [backend, cycleId]);

  const loadBriefing = useCallback(
    (forcePull: boolean) => {
      setError(null);
      setRefreshing(true);
      const run = forcePull
        ? backend.getBriefing(cycleId, true)
        : backend.getBriefing(cycleId, false).then((cached) => {
            setBriefing(cached);
            return backend.getBriefing(cycleId, true);
          });
      void run
        .then(setBriefing)
        .catch((e) => setError(errorMessage(e)))
        .finally(() => setRefreshing(false));
    },
    [backend, cycleId],
  );

  useEffect(() => {
    loadCycleDetails();
    let cancelled = false;
    void backend
      .getBriefing(cycleId, false)
      .then((cached) => {
        if (!cancelled) setBriefing(cached);
        return backend.getBriefing(cycleId, true);
      })
      .then((fresh) => {
        if (!cancelled) setBriefing(fresh);
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
  }, [backend, cycleId, loadCycleDetails]);

  const projectMeetings = useMemo(
    () => meetings.filter((m) => cycleMap[m.id] === cycleId),
    [meetings, cycleMap, cycleId],
  );

  const sortedTasks = useMemo(() => (briefing ? sortTasksByUrgency(briefing) : []), [briefing]);
  const topTasks = sortedTasks.slice(0, 5);
  const extraTasks = sortedTasks.slice(5);
  const status = briefing ? statusLine(briefing, refreshing) : null;

  const actOnMeeting = (p: Promise<unknown>) => {
    setError(null);
    void p.then(reloadMeetings).catch((e) => setError(errorMessage(e)));
  };

  const handleSyncIssues = async () => {
    if (!details?.repoId) return;
    setError(null);
    setIssueSyncNote(null);
    setSyncingIssues(true);
    try {
      const r = await backend.pullCycleRepo(details.repoId);
      if (r.outcome === "failed") {
        setError(r.error ?? "Could not sync GitHub issues.");
      } else {
        setIssueSyncNote("GitHub issues synced.");
      }
      reloadTasks();
      loadBriefing(false);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSyncingIssues(false);
    }
  };

  const title = details?.cycle.name ?? briefing?.cycleName ?? cycleId;

  return (
    <>
      <div className="page-head">
        <h2>
          <span className="glitch" data-text={title}>
            {title}
          </span>
        </h2>
        <span className="head-actions">
          <span className="chip">{cycleId}</span>
          <button
            type="button"
            className="btn btn-small"
            aria-label="Project settings"
            onClick={() => setSettingsOpen(true)}
          >
            ⚙ Settings
          </button>
          <button
            type="button"
            className="btn btn-small btn-record"
            onClick={() => setImportOpen(true)}
          >
            + Add meeting
          </button>
          <button type="button" className="btn btn-small btn-ghost" onClick={onBack}>
            Back to projects
          </button>
        </span>
      </div>

      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}

      <div className="project-top-grid">
        <section className="panel" aria-label="What to discuss next">
          <div className="panel-body">
            <div className="publish-head">
              <h3 className="sub-head">What to discuss next</h3>
              {briefing?.offline && <span className="badge badge-job-failed">Offline</span>}
              <button
                type="button"
                className="btn btn-small"
                disabled={refreshing}
                onClick={() => loadBriefing(true)}
              >
                {refreshing ? "Refreshing…" : "Refresh"}
              </button>
            </div>

            {status && (
              <p
                className={status.tone === "stale" ? "brief-status brief-stale" : "brief-status"}
                role="status"
              >
                {status.text}
              </p>
            )}

            {briefing?.warnings && briefing.warnings.length > 0 && (
              <ul className="brief-warnings" aria-label="Briefing warnings">
                {briefing.warnings.map((w) => (
                  <li key={w}>{w}</li>
                ))}
              </ul>
            )}

            {!briefing && !error && <p className="empty">Loading project briefing…</p>}

            {briefing && briefing.topics.length === 0 && (
              <p className="empty">No open discussion topics right now.</p>
            )}

            {briefing && briefing.topics.length > 0 && (
              <ul className="cycle-rows" aria-label="Open topics">
                {briefing.topics.map((t) => (
                  <li key={t.id} className="cycle-row">
                    <div className="cycle-main">
                      <span className="cycle-name">{t.text}</span>
                      <span className="person-meta">Raised in meeting {t.openedIn}</span>
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </section>

        <section className="panel" aria-label="Action items and tasks">
          <div className="panel-body">
            <div className="publish-head">
              <h3 className="sub-head">Action items &amp; Tasks</h3>
              {briefing && <span className="chip">{summaryLine(briefing)}</span>}
              <span className="head-actions">
                <button
                  type="button"
                  className="btn btn-small btn-ghost"
                  aria-pressed={showAllTasks}
                  onClick={() => setShowAllTasks((v) => !v)}
                >
                  {showAllTasks ? "Urgent briefing" : "All tasks"}
                </button>
                <button
                  type="button"
                  className="btn btn-small"
                  disabled={syncingIssues || !details?.repoId}
                  onClick={() => void handleSyncIssues()}
                >
                  {syncingIssues ? "Syncing…" : "Sync GitHub issues"}
                </button>
              </span>
            </div>

            {issueSyncNote && (
              <p className="brief-status" role="status">
                {issueSyncNote}
              </p>
            )}

            {showAllTasks ? (
              !tasks ? (
                <p className="empty">Loading GitHub issues…</p>
              ) : !tasks.repo ? (
                <p className="empty">
                  This project&apos;s repository is not linked to GitHub. Link it in Project
                  Settings → Integrations.
                </p>
              ) : (
                <>
                  <p className="brief-status" role="status">
                    {tasks.repo} · {taskCounts(tasks)}. {tasksAsOf(tasks)}
                  </p>
                  {tasks.open.length === 0 && tasks.done.length === 0 && (
                    <p className="empty">
                      No GitHub issues yet. Issues appear here after a meeting is published.
                    </p>
                  )}
                  {tasks.open.length > 0 && (
                    <ul className="cycle-rows" aria-label="Open GitHub issues">
                      {tasks.open.map((t) => (
                        <CycleIssueRow key={t.number} task={t} done={false} />
                      ))}
                    </ul>
                  )}
                  {tasks.done.length > 0 && (
                    <details className="brief-done" open>
                      <summary>Done ({tasks.done.length})</summary>
                      <ul className="cycle-rows" aria-label="Done GitHub issues">
                        {tasks.done.map((t) => (
                          <CycleIssueRow key={t.number} task={t} done />
                        ))}
                      </ul>
                    </details>
                  )}
                </>
              )
            ) : (
              <>
                {!briefing && !error && <p className="empty">Loading tasks…</p>}

                {briefing && sortedTasks.length === 0 && (
                  <p className="empty">No open action items for this project.</p>
                )}

                {briefing && topTasks.length > 0 && (
                  <ul className="cycle-rows" aria-label="Top action items">
                    {topTasks.map((item) => (
                      <TaskRow key={item.task.id} item={item} today={briefing.today} />
                    ))}
                  </ul>
                )}

                {briefing && extraTasks.length > 0 && (
                  <details className="brief-done">
                    <summary>
                      Show {extraTasks.length} more {extraTasks.length === 1 ? "task" : "tasks"}
                    </summary>
                    <ul className="cycle-rows" aria-label="More action items">
                      {extraTasks.map((item) => (
                        <TaskRow key={item.task.id} item={item} today={briefing.today} />
                      ))}
                    </ul>
                  </details>
                )}

                {briefing && briefing.done.length > 0 && (
                  <details className="brief-done">
                    <summary>Done ({briefing.done.length})</summary>
                    <ul className="cycle-rows" aria-label="Done tasks">
                      {briefing.done.map((t) => (
                        <TaskRow
                          key={t.id}
                          item={{ task: t, assignee: "Completed" }}
                          today={briefing.today}
                          done
                        />
                      ))}
                    </ul>
                  </details>
                )}
              </>
            )}
          </div>
        </section>
        {details?.repoId && tasks?.repo && (
          <section className="panel" aria-label="Tasks">
            <div className="panel-body">
              <div className="publish-head">
                <h3 className="sub-head">Tasks</h3>
              </div>
              <CycleTasksView cycleId={cycleId} repoId={details.repoId} />
            </div>
          </section>
        )}
      </div>

      <section aria-label="Project meetings">
        <div className="page-head">
          <h3 className="sub-head">Meetings</h3>
          <span className="chip">{plural(projectMeetings.length, "meeting", "meetings")}</span>
        </div>

        {projectMeetings.length === 0 ? (
          <p className="empty">
            No meetings in this project yet. Record a meeting or click &ldquo;+ Add meeting&rdquo;
            to import an audio or video file.
          </p>
        ) : (
          <ul className="cycle-rows cycle-rows-wide" aria-label="Meetings list">
            {projectMeetings.map((m) => (
              <MeetingRowCard
                key={m.id}
                meeting={m}
                recordingMeetingId={recording.meetingId}
                onOpen={onOpenMeeting}
                onProcess={(id) => actOnMeeting(backend.processMeeting(id))}
                onRecover={(id) => actOnMeeting(backend.recoverMeeting(id))}
                onDelete={(id) => actOnMeeting(backend.deleteMeeting(id))}
              />
            ))}
          </ul>
        )}
      </section>

      {settingsOpen && (
        <ProjectSettingsModal
          cycleId={cycleId}
          onClose={() => setSettingsOpen(false)}
          onSaved={() => {
            loadCycleDetails();
            loadBriefing(false);
            reloadTasks();
          }}
        />
      )}

      {importOpen && (
        <ImportMeetingModal
          cycleId={cycleId}
          onClose={() => setImportOpen(false)}
          onImported={(imported) => {
            setImportedMap((prev) => {
              const next = new Map(prev);
              next.set(imported.summary.id, cycleId);
              return next;
            });
            setImportOpen(false);
            reloadMeetings();
          }}
        />
      )}
    </>
  );
}
