import { useState } from "react";
import { useBackend } from "../backend-context";
import { newCycleError, repoState } from "../cycles";
import { errorMessage, useCycles } from "../hooks";
import type { CycleListItem, CycleRepoView, PullResult, RepoKind } from "../ipc/types";
import { MODE_NAMES } from "../report-gate";

const PULL_TEXT: Record<PullResult["outcome"], string> = {
  updated: "Updated from the remote.",
  upToDate: "Already up to date.",
  noRemote: "This repository has no remote to pull from.",
  failed: "Could not update.",
};

/** The cycles of every registered repository, with the repositories, and the form for a new cycle. */
export function CyclesView(props: {
  onOpenCycle: (cycleId: string) => void;
  onOpenBriefing: (cycleId: string) => void;
}) {
  const { onOpenCycle, onOpenBriefing } = props;
  const backend = useBackend();
  const { cycles, repos, reload } = useCycles();
  const [error, setError] = useState<string | null>(null);

  const [kind, setKind] = useState<RepoKind>("clone");
  const [where, setWhere] = useState("");
  const [adding, setAdding] = useState(false);

  const [newId, setNewId] = useState("");
  const [newName, setNewName] = useState("");
  const [newRepo, setNewRepo] = useState("");
  const [pulled, setPulled] = useState<Record<string, string>>({});
  const [pulling, setPulling] = useState<string | null>(null);

  const addRepo = () => {
    setError(null);
    setAdding(true);
    backend
      .addCycleRepo(kind === "clone" ? { kind, url: where.trim() } : { kind, path: where.trim() })
      .then(() => {
        setWhere("");
        reload();
      })
      .catch((e) => setError(errorMessage(e)))
      .finally(() => setAdding(false));
  };

  const pull = (repo: CycleRepoView) => {
    setError(null);
    setPulling(repo.id);
    backend
      .pullCycleRepo(repo.id)
      .then((r) => {
        const detail = r.error ? ` ${r.error}` : "";
        const warnings = r.warnings.map((w) => ` Warning: ${w}`).join("");
        setPulled((p) => ({ ...p, [repo.id]: `${PULL_TEXT[r.outcome]}${detail}${warnings}` }));
        reload();
      })
      .catch((e) => setError(errorMessage(e)))
      .finally(() => setPulling(null));
  };

  const create = () => {
    setError(null);
    backend
      .createCycle(newId.trim(), newName.trim(), newRepo === "" ? undefined : newRepo)
      .then((d) => {
        setNewId("");
        setNewName("");
        reload();
        onOpenCycle(d.cycle.id);
      })
      .catch((e) => setError(errorMessage(e)));
  };

  const createProblem = newCycleError(newId, newName);
  const byRepo = (repoId: string): CycleListItem[] =>
    (cycles ?? []).filter((c) => c.repoId === repoId);

  return (
    <>
      <div className="page-head">
        <h2>
          <span className="glitch" data-text="Cycles">
            Cycles
          </span>
        </h2>
        <span className="chip">{cycles ? cycles.length : 0} cycles</span>
      </div>
      <p className="notice">
        A cycle is a series of meetings that share participants, vocabulary and report format. It
        lives in a Git repository, so reports and tasks can be read and changed without this
        application. Edits made here are saved to the working folder and are not committed.
      </p>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}

      <h3 className="sub-head">Repositories</h3>
      {repos.length === 0 ? (
        <p className="empty">No repository yet. Create a cycle below or add a repository.</p>
      ) : (
        <ul className="repos" aria-label="Cycle repositories">
          {repos.map((repo) => (
            <li key={repo.id}>
              <article className="panel quiet repo" aria-label={`Repository ${repo.id}`}>
                <div className="panel-body">
                  <h4 className="card-title">
                    {repo.id}
                    <span className="badge badge-recovered">
                      {repo.kind === "clone" ? "Clone" : "Local"}
                    </span>
                    {repo.offline && <span className="badge badge-job-failed">Offline</span>}
                    {!repo.available && <span className="badge badge-job-failed">Missing</span>}
                  </h4>
                  <p className="card-when">{repo.remote ?? repo.path}</p>
                  <p className="repo-state">
                    {repoState(repo)}
                    {repo.offline && " · showing the state of the last successful pull"}
                  </p>
                  {repo.lastError && <p className="error">{repo.lastError}</p>}
                  {pulled[repo.id] && (
                    <p className="repo-state" role="status">
                      {pulled[repo.id]}
                    </p>
                  )}
                  <ul className="cycle-rows" aria-label={`Cycles of ${repo.id}`}>
                    {byRepo(repo.id).map((c) => (
                      <li key={c.id} className="cycle-row">
                        <div className="cycle-main">
                          <span className="cycle-name">
                            {c.name} <span className="cycle-id">{c.id}</span>
                          </span>
                          <span className="person-meta">
                            {c.participantsCount} participants
                            {c.defaultMode && ` · default mode ${MODE_NAMES[c.defaultMode]}`}
                          </span>
                          {!c.valid && (
                            <span className="error">
                              {c.issues[0]
                                ? `${c.issues[0].path}: ${c.issues[0].message}`
                                : "Cannot be read."}
                            </span>
                          )}
                        </div>
                        <span
                          className={c.valid ? "badge badge-recovered" : "badge badge-job-failed"}
                        >
                          {c.valid ? "Valid" : "Needs fixing"}
                        </span>
                        <button
                          className="btn btn-small"
                          aria-label={`Briefing ${c.id}`}
                          onClick={() => onOpenBriefing(c.id)}
                        >
                          Briefing
                        </button>
                        <button
                          className="btn btn-small"
                          aria-label={`Edit ${c.id}`}
                          onClick={() => onOpenCycle(c.id)}
                        >
                          Edit
                        </button>
                      </li>
                    ))}
                    {byRepo(repo.id).length === 0 && <li className="empty">No cycles in it.</li>}
                  </ul>
                  <div className="card-actions">
                    {repo.remote !== null && (
                      <button
                        className="btn btn-small"
                        aria-label={`Pull ${repo.id}`}
                        disabled={pulling === repo.id}
                        onClick={() => pull(repo)}
                      >
                        {pulling === repo.id ? "Pulling…" : "Pull"}
                      </button>
                    )}
                  </div>
                </div>
              </article>
            </li>
          ))}
        </ul>
      )}

      <h3 className="sub-head">Add a repository</h3>
      <div className="panel quiet" style={{ maxWidth: 620 }}>
        <div className="panel-body">
          <form
            className="form"
            onSubmit={(e) => {
              e.preventDefault();
              addRepo();
            }}
          >
            <label>
              Repository
              <select value={kind} onChange={(e) => setKind(e.target.value as RepoKind)}>
                <option value="clone">Clone from an address</option>
                <option value="local">Use an existing folder</option>
              </select>
            </label>
            <label>
              {kind === "clone" ? "Repository address" : "Folder path"}
              <input
                value={where}
                placeholder={
                  kind === "clone" ? "https://example.org/team/cycles.git" : "C:\\Users\\me\\cycles"
                }
                onChange={(e) => setWhere(e.target.value)}
              />
              <small>
                {kind === "clone"
                  ? "Credentials come from the Git credential manager, never from this address."
                  : "The folder must exist; its cycles are listed above."}
              </small>
            </label>
            <div className="actions">
              <button className="btn" type="submit" disabled={adding || where.trim() === ""}>
                {adding ? (kind === "clone" ? "Cloning…" : "Adding…") : "Add repository"}
              </button>
            </div>
          </form>
        </div>
      </div>

      <h3 className="sub-head">Create a cycle</h3>
      <div className="panel quiet" style={{ maxWidth: 620 }}>
        <div className="panel-body">
          <form
            className="form"
            onSubmit={(e) => {
              e.preventDefault();
              if (createProblem === null) create();
            }}
          >
            <label>
              Cycle id
              <input value={newId} placeholder="NOMAD" onChange={(e) => setNewId(e.target.value)} />
              <small>Used in file names and as the issue label. Letters, digits, - and _.</small>
            </label>
            <label>
              Name
              <input
                value={newName}
                placeholder="Nomad rover"
                onChange={(e) => setNewName(e.target.value)}
              />
            </label>
            <label>
              Repository
              <select value={newRepo} onChange={(e) => setNewRepo(e.target.value)}>
                <option value="">Default local repository</option>
                {repos.map((r) => (
                  <option key={r.id} value={r.id}>
                    {r.id}
                  </option>
                ))}
              </select>
            </label>
            {newId !== "" && createProblem && <p className="error">{createProblem}</p>}
            <div className="actions">
              <button className="btn btn-record" type="submit" disabled={createProblem !== null}>
                Create cycle
              </button>
            </div>
          </form>
        </div>
      </div>
    </>
  );
}
