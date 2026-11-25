import { useState } from "react";
import { useBackend } from "../backend-context";
import { errorMessage, useCycleFacts } from "../hooks";
import type { CycleTask } from "../ipc/types";
import { taskCounts, tasksAsOf } from "../publish";

/**
 * The tasks of a project as its GitHub issues stand: open ones, and done ones (an issue that is
 * closed, by a person or by an approved status change, counts as done and is not handed to the
 * model again). The state is the one of the last pull of the repository.
 */
export function CycleTasksView(props: { cycleId: string; repoId: string }) {
  const { cycleId, repoId } = props;
  const backend = useBackend();
  const { tasks, reload } = useCycleFacts(cycleId);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = () => {
    setBusy(true);
    setError(null);
    setMessage(null);
    backend
      .pullCycleRepo(repoId)
      .then((r) => {
        if (r.outcome === "failed") setError(r.error ?? "The repository could not be updated.");
        else setMessage("Read the issues again.");
        reload();
      })
      .catch((e) => setError(errorMessage(e)))
      .finally(() => setBusy(false));
  };

  if (!tasks) return null;
  if (!tasks.repo) {
    return (
      <p className="empty">
        This project&apos;s repository is not linked to a GitHub repository, so there are no issues
        to show. Link it in the project settings, under Integrations.
      </p>
    );
  }
  return (
    <>
      <div className="outbox-head">
        <span className="outbox-headline" role="status">
          {tasks.repo} · {taskCounts(tasks)}. {tasksAsOf(tasks)}
        </span>
        <button className="btn btn-small" disabled={busy} onClick={refresh}>
          Refresh from GitHub
        </button>
        {message && <span role="status">{message}</span>}
      </div>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {tasks.open.length === 0 && tasks.done.length === 0 && (
        <p className="empty">No tasks yet. Issues appear here after a meeting was published.</p>
      )}
      {tasks.open.length > 0 && (
        <ul className="tasks-list" aria-label="Open tasks">
          {tasks.open.map((t) => (
            <TaskRow key={t.number} task={t} done={false} />
          ))}
        </ul>
      )}
      {tasks.done.length > 0 && (
        <ul className="tasks-list" aria-label="Done tasks">
          {tasks.done.map((t) => (
            <TaskRow key={t.number} task={t} done />
          ))}
        </ul>
      )}
    </>
  );
}

function TaskRow({ task, done }: { task: CycleTask; done: boolean }) {
  return (
    <li className={done ? "task task-done" : "task"}>
      <span className={done ? "badge badge-job-done" : "badge badge-job-running"}>
        {done ? "Done" : "Open"}
      </span>
      {task.url ? (
        <a href={task.url} target="_blank" rel="noreferrer noopener">
          #{task.number}
        </a>
      ) : (
        <span>#{task.number}</span>
      )}
      <span className="task-title">{task.title}</span>
      {task.assignee && <span className="chip">{task.assignee}</span>}
    </li>
  );
}
