import { useEffect, useId, useState } from "react";
import { useBackend } from "../backend-context";
import { BrowseButton } from "../components/BrowseButton";
import { HelpPopover } from "../components/HelpPopover";
import { cycleIdError, newCycleError } from "../cycles";
import { errorMessage } from "../hooks";
import type {
  GithubRepoStatus,
  LanguageSetting,
  ReportMode,
  WizardCreateProjectInput,
  WizardCreateResult,
} from "../ipc/types";
import { MODE_HELP, MODE_NAMES, REPORT_MODES } from "../report-gate";
import { Modal } from "./Modal";

export interface ProjectWizardProps {
  onClose: () => void;
  onCreated: (cycleId: string) => void;
}

/** Generate a valid uppercase identifier suggestion from a human-readable project name. */
export function suggestProjectId(name: string): string {
  const cleaned = name
    .trim()
    .toUpperCase()
    .replace(/[^A-Z0-9_-]+/g, "-")
    .replace(/^[-_]+|[-_]+$/g, "");
  if (cleaned === "") return "";
  return /^[A-Z0-9]/.test(cleaned) ? cleaned : `P-${cleaned}`;
}

const REPO_PART = /^[A-Za-z0-9_.-]+$/;
const DOTS_ONLY = /^\.+$/;

/** The repository name proposed for a project: its id as a valid, lower-case GitHub name. */
export function suggestRepoName(id: string): string {
  const cleaned = id
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/^[-.]+|[-.]+$/g, "");
  return cleaned === "" ? "meeting-notes" : cleaned;
}

export type GithubRepoPlan = { owner: string | null; name: string } | { error: string };

/**
 * What would be created on GitHub: the `owner/name` field when it is filled (the owner is then
 * an organization, or the token account itself when it is the same login), otherwise the project
 * id as the name in the account of the saved token.
 */
export function planGithubRepo(githubRepo: string, id: string): GithubRepoPlan {
  const text = githubRepo.trim();
  if (text === "") return { owner: null, name: suggestRepoName(id) };
  const [owner, name, ...rest] = text.split("/");
  if (
    rest.length > 0 ||
    name === undefined ||
    !REPO_PART.test(owner) ||
    !REPO_PART.test(name) ||
    DOTS_ONLY.test(owner) ||
    DOTS_ONLY.test(name) ||
    name.endsWith(".git")
  ) {
    return { error: "Write the GitHub repository as owner/name, or leave it empty." };
  }
  return { owner, name };
}

type Phase = "edit" | "confirm" | "done";

function describeRepo(plan: { owner: string | null; name: string }): string {
  return plan.owner === null ? `${plan.name} (in your account)` : `${plan.owner}/${plan.name}`;
}

/** Guided 2-step modal wizard for creating a new project and optional local auto-export. */
export function ProjectWizard({ onClose, onCreated }: ProjectWizardProps) {
  const backend = useBackend();
  const [step, setStep] = useState<1 | 2>(1);
  const [phase, setPhase] = useState<Phase>("edit");
  const [githubStatus, setGithubStatus] = useState<GithubRepoStatus | null>(null);
  const [createGithub, setCreateGithub] = useState(false);
  const [result, setResult] = useState<WizardCreateResult | null>(null);
  const [name, setName] = useState("");
  const [id, setId] = useState("");
  const [idTouched, setIdTouched] = useState(false);
  const [mode, setMode] = useState<"auto" | "existing">("auto");
  const [repoPath, setRepoPath] = useState("");
  const [remoteUrl, setRemoteUrl] = useState("");

  const [outputLanguage, setOutputLanguage] = useState<LanguageSetting>("auto");
  const [defaultReportMode, setDefaultReportMode] = useState<ReportMode>("auto");
  const [githubRepo, setGithubRepo] = useState("");
  const [discordWebhookRef, setDiscordWebhookRef] = useState("");
  const [localSaveEnabled, setLocalSaveEnabled] = useState(false);
  const [localSaveDir, setLocalSaveDir] = useState("");
  const [defaultSaveDir, setDefaultSaveDir] = useState("");

  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const defaultModeId = useId();
  const githubRepoId = useId();
  const discordRefId = useId();

  useEffect(() => {
    let cancelled = false;
    void backend
      .autoDetectModels()
      .then((detected) => {
        if (!cancelled && detected.defaultLocalSaveDir) {
          setDefaultSaveDir(detected.defaultLocalSaveDir);
          setLocalSaveDir((cur) => (cur === "" ? detected.defaultLocalSaveDir : cur));
        }
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [backend]);

  useEffect(() => {
    let cancelled = false;
    void backend
      .githubRepoStatus()
      .then((status) => {
        if (!cancelled) setGithubStatus(status);
      })
      .catch(() => {
        if (!cancelled) setGithubStatus({ tokenSet: false, host: "" });
      });
    return () => {
      cancelled = true;
    };
  }, [backend]);

  const handleNameChange = (nextName: string) => {
    setName(nextName);
    if (!idTouched) {
      setId(suggestProjectId(nextName));
    }
  };

  const handleIdChange = (nextId: string) => {
    setIdTouched(true);
    setId(nextId);
  };

  const idProblem = id.trim() !== "" ? cycleIdError(id) : null;
  const step1Problem =
    newCycleError(id, name) ??
    (mode === "existing" && repoPath.trim() === "" && remoteUrl.trim() === ""
      ? "Enter an existing folder path or a remote repository URL."
      : null);

  const goNext = () => {
    if (step1Problem) {
      setError(step1Problem);
      return;
    }
    setError(null);
    setStep(2);
  };

  const buildRequest = (): WizardCreateProjectInput => ({
    mode,
    id: id.trim(),
    name: name.trim(),
    repoPath: repoPath.trim() || null,
    remoteUrl: mode === "existing" ? remoteUrl.trim() || null : null,
    outputLanguage,
    defaultReportMode,
    githubRepo: githubRepo.trim() || null,
    discordWebhookRef: discordWebhookRef.trim() || null,
    localSaveEnabled,
    localSaveDir: localSaveEnabled ? localSaveDir.trim() || defaultSaveDir || null : null,
  });

  const wantsGithubRepo = mode === "auto" && createGithub;
  const githubPlan = planGithubRepo(githubRepo, id);
  const githubPlanError = wantsGithubRepo && "error" in githubPlan ? githubPlan.error : null;
  const tokenSet = githubStatus?.tokenSet === true;

  /** Plain creation: a local folder, with the GitHub field only as a link. */
  const createLocal = async () => {
    setError(null);
    setSubmitting(true);
    try {
      const created = await backend.wizardCreateProject(buildRequest());
      onCreated(created.id);
    } catch (e) {
      setError(errorMessage(e));
      setPhase("edit");
    } finally {
      setSubmitting(false);
    }
  };

  /**
   * Creation with a private GitHub repository. Called only after the confirm step, or when no
   * token is saved (then nothing is sent and the answer explains the local-only result).
   */
  const createWithGithub = async () => {
    if ("error" in githubPlan) {
      setError(githubPlan.error);
      setPhase("edit");
      return;
    }
    setError(null);
    setSubmitting(true);
    try {
      const created = await backend.wizardCreateProjectWithGithub(buildRequest(), {
        name: githubPlan.name,
        owner: githubPlan.owner,
        confirmed: true,
      });
      setResult(created);
      setPhase("done");
    } catch (e) {
      setError(errorMessage(e));
      setPhase("edit");
    } finally {
      setSubmitting(false);
    }
  };

  const submit = async () => {
    if (step1Problem) {
      setError(step1Problem);
      setStep(1);
      return;
    }
    if (!wantsGithubRepo) {
      await createLocal();
      return;
    }
    if (githubPlanError) {
      setError(githubPlanError);
      return;
    }
    setError(null);
    if (tokenSet) {
      // The person sees exactly what would be created before anything is sent.
      setPhase("confirm");
    } else {
      await createWithGithub();
    }
  };

  return (
    <Modal title="Create New Project" onClose={onClose} wide>
      <ol className="wizard-steps" aria-label="Wizard steps">
        <li
          className={step === 1 ? "wizard-step wizard-step-active" : "wizard-step wizard-step-done"}
        >
          1. Name &amp; Repository
        </li>
        <li className={step === 2 ? "wizard-step wizard-step-active" : "wizard-step"}>
          2. Language &amp; Saving
        </li>
      </ol>

      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}

      {phase === "edit" && step === 1 && (
        <form
          className="form"
          aria-label="Project details"
          onSubmit={(e) => {
            e.preventDefault();
            goNext();
          }}
        >
          <label>
            Project name
            <input
              aria-label="Project name"
              value={name}
              placeholder="e.g. Nomad Rover"
              autoFocus
              onChange={(e) => handleNameChange(e.target.value)}
            />
            <small>A friendly name shown across your projects and meeting notes.</small>
          </label>

          <label>
            Project ID
            <input
              aria-label="Project ID"
              value={id}
              placeholder="e.g. NOMAD-ROVER"
              onChange={(e) => handleIdChange(e.target.value)}
            />
            <small>
              Auto-generated from the name. Used for folder names and issue labels (letters, digits,
              - and _).
            </small>
            {idProblem && (
              <span className="field-issue" role="alert">
                {idProblem}
              </span>
            )}
          </label>

          <div className="field-group">
            <span className="field-label-row">
              <span className="field-label">Repository setup</span>
              <HelpPopover
                title="Project repository"
                recommended="Create & auto-setup new repository"
              >
                <p>
                  Projects store meeting notes, templates, and tasks in a Git folder so nothing is
                  locked inside the app.
                </p>
              </HelpPopover>
            </span>

            <div className="import-mode-cards" role="radiogroup" aria-label="Repository setup">
              <button
                type="button"
                role="radio"
                aria-checked={mode === "auto"}
                className={
                  mode === "auto" ? "import-mode-card import-mode-card-active" : "import-mode-card"
                }
                onClick={() => setMode("auto")}
              >
                <strong>Create &amp; auto-setup new repository (Recommended)</strong>
                <span className="person-meta">
                  Automatically sets up a local Git project folder ready for notes right away.
                </span>
              </button>

              <button
                type="button"
                role="radio"
                aria-checked={mode === "existing"}
                className={
                  mode === "existing"
                    ? "import-mode-card import-mode-card-active"
                    : "import-mode-card"
                }
                onClick={() => setMode("existing")}
              >
                <strong>Connect existing repository / URL</strong>
                <span className="person-meta">
                  Clone a shared Git repository URL or point to an existing local project folder.
                </span>
              </button>
            </div>
          </div>

          {mode === "existing" && (
            <>
              <label>
                Remote repository URL
                <input
                  aria-label="Remote repository URL"
                  value={remoteUrl}
                  placeholder="https://github.com/org/team-notes.git"
                  onChange={(e) => setRemoteUrl(e.target.value)}
                />
                <small>
                  Optional if linking a local folder below. Uses Git credential manager.
                </small>
              </label>

              <label>
                Existing local folder path
                <span className="path-row">
                  <input
                    aria-label="Existing local folder path"
                    value={repoPath}
                    placeholder="C:\Users\me\projects\team-notes"
                    onChange={(e) => setRepoPath(e.target.value)}
                  />
                  <BrowseButton
                    target={{ folder: true }}
                    label="Browse for the existing local folder"
                    title="Choose the project folder"
                    onPicked={setRepoPath}
                  />
                </span>
                <small>
                  Folder on this computer (or destination folder if also cloning a remote URL).
                </small>
              </label>
            </>
          )}

          <div className="actions">
            <button className="btn btn-ghost" type="button" onClick={onClose}>
              Cancel
            </button>
            <button className="btn btn-record" type="submit" disabled={step1Problem !== null}>
              Next
            </button>
          </div>
        </form>
      )}

      {phase === "edit" && step === 2 && (
        <form
          className="form"
          aria-label="Language and saving options"
          onSubmit={(e) => {
            e.preventDefault();
            void submit();
          }}
        >
          <label>
            Report language
            <select
              aria-label="Report language"
              value={outputLanguage}
              onChange={(e) => setOutputLanguage(e.target.value as LanguageSetting)}
            >
              <option value="auto">Auto (match meeting language)</option>
              <option value="en">English</option>
              <option value="pl">Polish</option>
            </select>
          </label>

          <div className="field-block">
            <div className="field-label-row">
              <label htmlFor={defaultModeId}>Default report mode</label>
              <HelpPopover title="Default report mode" recommended="Auto">
                <p>{MODE_HELP[defaultReportMode]}</p>
              </HelpPopover>
            </div>
            <select
              id={defaultModeId}
              aria-label="Default report mode"
              value={defaultReportMode}
              onChange={(e) => setDefaultReportMode(e.target.value as ReportMode)}
            >
              {REPORT_MODES.map((m) => (
                <option key={m} value={m}>
                  {MODE_NAMES[m]}
                </option>
              ))}
            </select>
            <small>{MODE_HELP[defaultReportMode]}</small>
          </div>

          <div className="field-block">
            <div className="field-label-row">
              <label htmlFor={githubRepoId}>GitHub repository (optional)</label>
              <HelpPopover
                title="GitHub repository"
                text="Link an owner/name repository to publish reports and track action items as GitHub issues."
              />
            </div>
            <input
              id={githubRepoId}
              aria-label="GitHub repository"
              value={githubRepo}
              placeholder="owner/name"
              onChange={(e) => setGithubRepo(e.target.value)}
            />
          </div>

          <div className="field-block">
            <div className="field-label-row">
              <label htmlFor={discordRefId}>Discord webhook reference (optional)</label>
              <HelpPopover
                title="Discord webhook reference"
                text="Enter a cred: reference name stored in your Windows Credential Manager so the secret URL is never committed to Git."
                recommended="cred:discord/project"
              />
            </div>
            <input
              id={discordRefId}
              aria-label="Discord webhook reference"
              value={discordWebhookRef}
              placeholder="cred:discord/project"
              onChange={(e) => setDiscordWebhookRef(e.target.value)}
            />
            <small>
              Discord is never set up automatically. Create the webhook in Discord yourself and save
              it in Settings &gt; Connectors; this field only names it.
            </small>
          </div>

          <fieldset>
            <legend>Local folder auto-save</legend>
            <label className="check">
              <input
                type="checkbox"
                checked={localSaveEnabled}
                onChange={(e) => {
                  const checked = e.target.checked;
                  setLocalSaveEnabled(checked);
                  if (checked && localSaveDir.trim() === "" && defaultSaveDir !== "") {
                    setLocalSaveDir(defaultSaveDir);
                  }
                }}
              />
              Automatically save a Markdown (.md) note to a local folder after processing
            </label>
            {localSaveEnabled && (
              <label>
                Local notes folder
                <span className="path-row">
                  <input
                    aria-label="Local notes folder"
                    value={localSaveDir}
                    placeholder={defaultSaveDir || "Documents\\BlahBlahBot\\notes"}
                    onChange={(e) => setLocalSaveDir(e.target.value)}
                  />
                  <BrowseButton
                    target={{ folder: true }}
                    label="Browse for the local notes folder"
                    title="Choose the notes folder"
                    onPicked={setLocalSaveDir}
                  />
                </span>
                <small>Works completely offline with zero Git or cloud setup required.</small>
              </label>
            )}
          </fieldset>

          {mode === "auto" && (
            <fieldset aria-label="New private GitHub repository">
              <legend>New GitHub repository (optional)</legend>
              <label className="check">
                <input
                  type="checkbox"
                  aria-label="Create a private GitHub repository"
                  checked={createGithub}
                  onChange={(e) => setCreateGithub(e.target.checked)}
                />
                Also create a private GitHub repository for this project
              </label>
              {createGithub && githubPlanError && (
                <span className="field-issue" role="alert">
                  {githubPlanError}
                </span>
              )}
              {createGithub && !githubPlanError && "name" in githubPlan && (
                <small>
                  Will create: <strong>{describeRepo(githubPlan)}</strong>, private.
                </small>
              )}
              {githubStatus !== null &&
                (tokenSet ? (
                  <small>
                    A saved GitHub token was found. You will confirm what is created before anything
                    is sent to GitHub.
                  </small>
                ) : (
                  <small>
                    No GitHub token is saved yet, so only a local Git folder is created. Save a
                    token in Settings &gt; Connectors to create repositories here.
                  </small>
                ))}
            </fieldset>
          )}

          <div className="actions">
            <button
              className="btn btn-ghost"
              type="button"
              disabled={submitting}
              onClick={() => {
                setError(null);
                setStep(1);
              }}
            >
              Back
            </button>
            <button className="btn btn-record" type="submit" disabled={submitting}>
              {submitting ? "Creating…" : "Create project"}
            </button>
          </div>
        </form>
      )}

      {phase === "confirm" && "name" in githubPlan && (
        <section className="form" aria-label="Confirm the new GitHub repository">
          <p>
            Blah Blah Bot will now create this repository on GitHub with your saved token, then
            create the project folder and link them.
          </p>
          <dl className="wizard-summary">
            <dt>Repository</dt>
            <dd>{describeRepo(githubPlan)}</dd>
            <dt>Visibility</dt>
            <dd>Private: only you and the people you invite can see it.</dd>
            <dt>Owner</dt>
            <dd>
              {githubPlan.owner === null
                ? "Your GitHub account (the one the saved token belongs to)"
                : `${githubPlan.owner} (an organization, or your own account when it is your login)`}
            </dd>
            <dt>Project</dt>
            <dd>
              {name.trim()} ({id.trim()})
            </dd>
          </dl>
          <p className="person-meta">
            Nothing has been sent to GitHub yet. The token is used only for this request and is
            never shown. If GitHub refuses, the project is still created as a local Git folder.
          </p>
          <div className="actions">
            <button
              className="btn btn-ghost"
              type="button"
              disabled={submitting}
              onClick={() => setPhase("edit")}
            >
              Back
            </button>
            <button
              className="btn btn-ghost"
              type="button"
              disabled={submitting}
              onClick={() => void createLocal()}
            >
              Create local project only
            </button>
            <button
              className="btn btn-record"
              type="button"
              disabled={submitting}
              onClick={() => void createWithGithub()}
            >
              {submitting ? "Creating…" : "Create repository & project"}
            </button>
          </div>
        </section>
      )}

      {phase === "done" && result && (
        <section className="form" aria-label="Project created">
          <p>
            <strong>Project {result.cycle.name} was created.</strong>
          </p>
          <p
            className={result.github.status === "created" ? "notice" : "notice notice-warn"}
            role="status"
          >
            {result.github.message}
          </p>
          {result.github.url && <p className="person-meta">{result.github.url}</p>}
          <div className="actions">
            <button
              className="btn btn-record"
              type="button"
              onClick={() => onCreated(result.cycle.id)}
            >
              Open project
            </button>
          </div>
        </section>
      )}
    </Modal>
  );
}
