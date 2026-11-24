import { useEffect, useState } from "react";
import { useBackend } from "../backend-context";
import { HelpPopover } from "../components/HelpPopover";
import { cycleIdError, newCycleError } from "../cycles";
import { errorMessage } from "../hooks";
import type { LanguageSetting, ReportMode } from "../ipc/types";
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

/** Guided 2-step modal wizard for creating a new project and optional local auto-export. */
export function ProjectWizard({ onClose, onCreated }: ProjectWizardProps) {
  const backend = useBackend();
  const [step, setStep] = useState<1 | 2>(1);
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

  const submit = async () => {
    if (step1Problem) {
      setError(step1Problem);
      setStep(1);
      return;
    }
    setError(null);
    setSubmitting(true);
    try {
      const created = await backend.wizardCreateProject({
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
      onCreated(created.id);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSubmitting(false);
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

      {step === 1 && (
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
                <input
                  aria-label="Existing local folder path"
                  value={repoPath}
                  placeholder="C:\Users\me\projects\team-notes"
                  onChange={(e) => setRepoPath(e.target.value)}
                />
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

      {step === 2 && (
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

          <label>
            <span className="field-label-row">
              Default report mode
              <HelpPopover title="Default report mode" recommended="Auto">
                <p>{MODE_HELP[defaultReportMode]}</p>
              </HelpPopover>
            </span>
            <select
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
          </label>

          <label>
            <span className="field-label-row">
              GitHub repository (optional)
              <HelpPopover
                title="GitHub repository"
                text="Link an owner/name repository to publish reports and track action items as GitHub issues."
              />
            </span>
            <input
              aria-label="GitHub repository"
              value={githubRepo}
              placeholder="owner/name"
              onChange={(e) => setGithubRepo(e.target.value)}
            />
          </label>

          <label>
            <span className="field-label-row">
              Discord webhook reference (optional)
              <HelpPopover
                title="Discord webhook reference"
                text="Enter a cred: reference name stored in your Windows Credential Manager so the secret URL is never committed to Git."
                recommended="cred:discord/project"
              />
            </span>
            <input
              aria-label="Discord webhook reference"
              value={discordWebhookRef}
              placeholder="cred:discord/project"
              onChange={(e) => setDiscordWebhookRef(e.target.value)}
            />
          </label>

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
                <input
                  aria-label="Local notes folder"
                  value={localSaveDir}
                  placeholder={defaultSaveDir || "Documents\\BlahBlahBot\\notes"}
                  onChange={(e) => setLocalSaveDir(e.target.value)}
                />
                <small>Works completely offline with zero Git or cloud setup required.</small>
              </label>
            )}
          </fieldset>

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
    </Modal>
  );
}
