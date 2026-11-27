import { useCallback, useEffect, useId, useMemo, useState } from "react";
import { useBackend } from "../backend-context";
import { AudioRetentionHelp } from "../components/AudioRetentionHelp";
import { BrowseButton } from "../components/BrowseButton";
import { HelpPopover } from "../components/HelpPopover";
import {
  type TypeRow,
  duplicateTypes,
  emptyParticipant,
  isDirty,
  issueText,
  issuesOf,
  typeRows,
  withTypeRows,
} from "../cycles";
import { errorMessage } from "../hooks";
import type {
  CycleDetails,
  CycleDto,
  CycleIssue,
  CycleParticipant,
  CycleRepoView,
  DetectedModels,
  LanguageSetting,
  ProjectPrefs,
  PublishMode,
  PublishingSecrets,
  ReportMode,
} from "../ipc/types";
import { DEFAULT_PUBLISH_RULE, parseWebhook } from "../publish";
import { MODE_HELP, MODE_NAMES, REPORT_MODES } from "../report-gate";
import { Modal } from "./Modal";
import { DeleteProjectSection } from "./DeleteProjectSection";
import { RuleChoice } from "./RuleChoice";
import { TemplateEditor } from "./TemplateEditor";

type SettingsTab = "local" | "people" | "report" | "integrations";

const LANGUAGES: { value: LanguageSetting; label: string }[] = [
  { value: "auto", label: "Auto" },
  { value: "en", label: "English" },
  { value: "pl", label: "Polish" },
];

const PUBLISH_OPTIONS: { value: PublishMode; label: string }[] = [
  { value: "on-approval", label: "After approval in the draft" },
  { value: "auto", label: "Automatically" },
  { value: "off", label: "Never" },
];

function FieldProblems({ issues, path }: { issues: CycleIssue[]; path: string }) {
  const here = issuesOf(issues, path);
  if (here.length === 0) return null;
  return (
    <small className="field-issue" role="alert">
      {here.map((i) => i.message).join(" ")}
    </small>
  );
}

export interface ProjectSettingsModalProps {
  cycleId: string;
  onClose: () => void;
  onSaved?: () => void;
  onDeleted?: () => void;
}

/**
 * Combined modal for project settings: machine-local preferences (local auto-export & LLM
 * override) plus shared cycle configuration (participants, glossary, report defaults, targets).
 */
export function ProjectSettingsModal({
  cycleId,
  onClose,
  onSaved,
  onDeleted,
}: ProjectSettingsModalProps) {
  const backend = useBackend();
  const [tab, setTab] = useState<SettingsTab>("local");
  const [prefs, setPrefs] = useState<ProjectPrefs | null>(null);
  const [savedPrefs, setSavedPrefs] = useState<ProjectPrefs | null>(null);
  const [detected, setDetected] = useState<DetectedModels | null>(null);
  /** Whether the global setting keeps the audio; shown next to "use the global setting". */
  const [globalKeepsAudio, setGlobalKeepsAudio] = useState<boolean | null>(null);

  const [details, setDetails] = useState<CycleDetails | null>(null);
  const [cycle, setCycle] = useState<CycleDto | null>(null);
  const [rows, setRows] = useState<TypeRow[]>([]);
  const [issues, setIssues] = useState<CycleIssue[]>([]);

  const [savingPrefs, setSavingPrefs] = useState(false);
  const [savingCycle, setSavingCycle] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const [repos, setRepos] = useState<CycleRepoView[]>([]);
  const [secrets, setSecrets] = useState<PublishingSecrets | null>(null);
  const [githubRepoInput, setGithubRepoInput] = useState("");
  const [githubApiInput, setGithubApiInput] = useState("");
  const [repoBusy, setRepoBusy] = useState(false);
  const [repoMsg, setRepoMsg] = useState<{ text: string; kind: "status" | "error" } | null>(null);

  const [webhookInput, setWebhookInput] = useState("");
  const [webhookBusy, setWebhookBusy] = useState(false);
  const [webhookMsg, setWebhookMsg] = useState<{ text: string; kind: "status" | "error" } | null>(
    null,
  );
  const [storedWebhookRefs, setStoredWebhookRefs] = useState<Set<string>>(() => new Set());
  const [openTargets, setOpenTargets] = useState<{
    github: boolean;
    discord: boolean;
    notion: boolean;
  }>({ github: false, discord: false, notion: false });

  const projectIdInputId = useId();
  const reportFolderInputId = useId();
  const webhookRefInputId = useId();
  const webhookSecretInputId = useId();
  const notionDbInputId = useId();

  const adoptCycle = useCallback((d: CycleDetails) => {
    setDetails(d);
    setCycle(structuredClone(d.cycle));
    setRows(typeRows(d.cycle.report.byType));
    setIssues(d.issues);
  }, []);

  const loadIntegrations = useCallback(
    (repoId?: string) => {
      void Promise.all([backend.listCycleRepos(), backend.publishingSecrets()])
        .then(([repoList, sec]) => {
          setRepos(repoList);
          setSecrets(sec);
          if (repoId) {
            const match = repoList.find((r) => r.id === repoId);
            if (match) {
              setGithubRepoInput(match.githubRepo ?? "");
              setGithubApiInput(match.githubApi ?? "");
            }
          }
        })
        .catch(() => undefined);
    },
    [backend],
  );

  useEffect(() => {
    let cancelled = false;
    void backend
      .getProjectPrefs(cycleId)
      .then((p) => {
        if (cancelled) return;
        setPrefs(p);
        setSavedPrefs(structuredClone(p));
      })
      .catch((e) => !cancelled && setError(errorMessage(e)));
    void backend
      .getCycle(cycleId)
      .then((d) => {
        if (cancelled) return;
        adoptCycle(d);
        loadIntegrations(d.repoId);
      })
      .catch((e) => !cancelled && setError(errorMessage(e)));
    void backend
      .autoDetectModels()
      .then((d) => !cancelled && setDetected(d))
      .catch(() => undefined);
    void backend
      .getProcessingSettings()
      .then((p) => !cancelled && setGlobalKeepsAudio(p.audioRetention.mode !== "delete"))
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [backend, cycleId, adoptCycle, loadIntegrations]);

  const editedCycle = useMemo(() => (cycle ? withTypeRows(cycle, rows) : null), [cycle, rows]);
  const cycleDirty =
    details !== null && editedCycle !== null && isDirty(details.cycle, editedCycle);
  const prefsDirty =
    prefs !== null && savedPrefs !== null && JSON.stringify(prefs) !== JSON.stringify(savedPrefs);
  const dupTypes = duplicateTypes(rows);

  const saveLocalPrefs = async () => {
    if (!prefs) return;
    setError(null);
    setMessage(null);
    setSavingPrefs(true);
    try {
      const next = await backend.saveProjectPrefs({
        ...prefs,
        llmKindOverride: prefs.llmKindOverride || null,
        llmModelOverride: prefs.llmKindOverride ? prefs.llmModelOverride?.trim() || null : null,
        audioRetention: prefs.audioRetention || null,
      });
      setPrefs(next);
      setSavedPrefs(structuredClone(next));
      setMessage("Local preferences saved.");
      onSaved?.();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSavingPrefs(false);
    }
  };

  const saveCycleSettings = async () => {
    if (!editedCycle) return;
    setError(null);
    setMessage(null);
    setSavingCycle(true);
    try {
      const res = await backend.saveCycle(editedCycle);
      if (res.saved && res.details) {
        adoptCycle(res.details);
        setMessage(
          res.warnings.length > 0
            ? `Project settings saved. ${res.warnings.join(" ")}`
            : "Project settings saved.",
        );
        onSaved?.();
      } else {
        setIssues(res.issues);
        setError("Fix the highlighted project settings before saving.");
      }
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSavingCycle(false);
    }
  };

  const patchCycle = (p: Partial<CycleDto>) => cycle && setCycle({ ...cycle, ...p });
  const setParticipant = (i: number, p: Partial<CycleParticipant>) =>
    cycle &&
    patchCycle({ participants: cycle.participants.map((x, j) => (j === i ? { ...x, ...p } : x)) });
  const setReport = (p: Partial<CycleDto["report"]>) =>
    cycle && patchCycle({ report: { ...cycle.report, ...p } });
  const setTargets = (p: Partial<CycleDto["targets"]>) =>
    cycle && patchCycle({ targets: { ...cycle.targets, ...p } });

  const templateNames = details?.templates.map((t) => t.name) ?? [];
  const github = cycle?.targets.github;
  const discordRef = cycle?.targets.discord;
  const notion = cycle?.targets.notion;
  const currentRepo = details ? repos.find((r) => r.id === details.repoId) : undefined;
  const repoLinkDirty =
    currentRepo !== undefined &&
    (githubRepoInput.trim() !== (currentRepo.githubRepo ?? "") ||
      githubApiInput.trim() !== (currentRepo.githubApi ?? ""));

  const trimmedWebhookRef = discordRef?.webhook.trim() ?? "";
  const validWebhookRef = /^cred:[A-Za-z0-9][A-Za-z0-9_./-]*$/.test(trimmedWebhookRef);
  const matchedSecret = validWebhookRef
    ? secrets?.targets.find((t) => t.reference === trimmedWebhookRef)
    : undefined;
  const isWebhookSet = Boolean(matchedSecret?.set || storedWebhookRefs.has(trimmedWebhookRef));

  const saveGithubLink = async () => {
    if (!details) return;
    setRepoMsg(null);
    setRepoBusy(true);
    try {
      const saved = await backend.setGithubRepo(
        details.repoId,
        githubRepoInput.trim(),
        githubApiInput.trim(),
      );
      setGithubRepoInput(saved.githubRepo ?? "");
      setGithubApiInput(saved.githubApi ?? "");
      setRepos((cur) => cur.map((r) => (r.id === saved.id ? saved : r)));
      setRepoMsg({ text: "GitHub repository link saved.", kind: "status" });
      onSaved?.();
    } catch (e) {
      setRepoMsg({ text: errorMessage(e), kind: "error" });
    } finally {
      setRepoBusy(false);
    }
  };

  const testGithubLink = async () => {
    setRepoMsg(null);
    setRepoBusy(true);
    try {
      const r = await backend.testGithubAccess(cycleId);
      setRepoMsg({ text: r.message, kind: r.ok ? "status" : "error" });
    } catch (e) {
      setRepoMsg({ text: errorMessage(e), kind: "error" });
    } finally {
      setRepoBusy(false);
    }
  };

  const storeDiscordWebhook = async () => {
    if (!validWebhookRef) return;
    const problem = parseWebhook(webhookInput);
    if (typeof problem === "string") {
      setWebhookMsg({ text: problem, kind: "error" });
      return;
    }
    setWebhookMsg(null);
    setWebhookBusy(true);
    try {
      await backend.setTargetSecret(trimmedWebhookRef, webhookInput);
      setWebhookInput("");
      setStoredWebhookRefs((cur) => new Set(cur).add(trimmedWebhookRef));
      const sec = await backend.publishingSecrets();
      setSecrets(sec);
      setWebhookMsg({ text: "Webhook stored.", kind: "status" });
      onSaved?.();
    } catch (e) {
      setWebhookMsg({ text: errorMessage(e), kind: "error" });
    } finally {
      setWebhookBusy(false);
    }
  };

  const clearDiscordWebhook = async () => {
    if (!validWebhookRef) return;
    setWebhookMsg(null);
    setWebhookBusy(true);
    try {
      await backend.clearTargetSecret(trimmedWebhookRef);
      setStoredWebhookRefs((cur) => {
        const next = new Set(cur);
        next.delete(trimmedWebhookRef);
        return next;
      });
      const sec = await backend.publishingSecrets();
      setSecrets(sec);
      setWebhookMsg({ text: "Webhook removed.", kind: "status" });
      onSaved?.();
    } catch (e) {
      setWebhookMsg({ text: errorMessage(e), kind: "error" });
    } finally {
      setWebhookBusy(false);
    }
  };

  const testDiscordSecret = async () => {
    if (!validWebhookRef) return;
    setWebhookMsg(null);
    setWebhookBusy(true);
    try {
      const res = await backend.testDiscordWebhook(trimmedWebhookRef);
      setWebhookMsg({ text: res.message, kind: res.ok ? "status" : "error" });
    } catch (e) {
      setWebhookMsg({ text: errorMessage(e), kind: "error" });
    } finally {
      setWebhookBusy(false);
    }
  };

  return (
    <Modal title={`Project Settings · ${cycle?.name ?? cycleId}`} onClose={onClose} wide>
      <div className="pane-toggle" role="tablist" aria-label="Project settings sections">
        <button
          type="button"
          role="tab"
          aria-selected={tab === "local"}
          className={tab === "local" ? "btn btn-small btn-stop" : "btn btn-small btn-ghost"}
          onClick={() => setTab("local")}
        >
          Local &amp; AI
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === "people"}
          className={tab === "people" ? "btn btn-small btn-stop" : "btn btn-small btn-ghost"}
          onClick={() => setTab("people")}
        >
          People &amp; Glossary
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === "report"}
          className={tab === "report" ? "btn btn-small btn-stop" : "btn btn-small btn-ghost"}
          onClick={() => setTab("report")}
        >
          Report &amp; Templates
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === "integrations"}
          className={tab === "integrations" ? "btn btn-small btn-stop" : "btn btn-small btn-ghost"}
          onClick={() => setTab("integrations")}
        >
          Integrations
        </button>
      </div>

      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {message && (
        <p className="valid-note" role="status">
          {message}
        </p>
      )}

      {tab === "local" && prefs && (
        <form
          className="form"
          aria-label="Machine-local project preferences"
          onSubmit={(e) => {
            e.preventDefault();
            void saveLocalPrefs();
          }}
        >
          <p className="section-hint">
            These preferences stay on this computer only and are never committed to the project
            repository.
          </p>

          <fieldset>
            <legend>
              Local folder auto-save{" "}
              <HelpPopover
                title="Local folder auto-save"
                text="Automatically export a standalone Markdown (.md) copy of each finished meeting note to a folder on this PC."
              />
            </legend>
            <label className="check">
              <input
                type="checkbox"
                checked={prefs.localSaveEnabled}
                onChange={(e) => {
                  const checked = e.target.checked;
                  setPrefs({
                    ...prefs,
                    localSaveEnabled: checked,
                    localSaveDir:
                      checked && prefs.localSaveDir.trim() === "" && detected?.defaultLocalSaveDir
                        ? detected.defaultLocalSaveDir
                        : prefs.localSaveDir,
                  });
                }}
              />
              Auto-export finished notes as Markdown (.md) for this project
            </label>
            <label>
              Local export folder
              <span className="path-row">
                <input
                  aria-label="Local export folder"
                  value={prefs.localSaveDir}
                  placeholder={detected?.defaultLocalSaveDir || "Documents\\BlahBlahBot\\notes"}
                  onChange={(e) => setPrefs({ ...prefs, localSaveDir: e.target.value })}
                />
                <BrowseButton
                  target={{ folder: true }}
                  label="Browse for the local export folder"
                  title="Choose the export folder"
                  onPicked={(path) => setPrefs({ ...prefs, localSaveDir: path })}
                />
              </span>
              <small>Leave empty to fall back to the global folder in Settings.</small>
            </label>
          </fieldset>

          <fieldset>
            <legend>
              AI model override{" "}
              <HelpPopover
                title="Per-project AI override"
                text="Override the global language model for this project only — for example, use a smaller local model or a remote API for a specific project."
                recommended="Use global model (Default)"
              />
            </legend>
            <label>
              Model backend for this project
              <select
                aria-label="Model backend override"
                value={prefs.llmKindOverride ?? ""}
                onChange={(e) =>
                  setPrefs({
                    ...prefs,
                    llmKindOverride: e.target.value === "" ? null : e.target.value,
                  })
                }
              >
                <option value="">Use global model (Default)</option>
                <option value="managed">Managed local model (llama-server)</option>
                <option value="external">External server (OpenAI-compatible)</option>
                <option value="cloud">Cloud API (per-meeting confirmation)</option>
              </select>
            </label>
            {prefs.llmKindOverride && (
              <label>
                Model name or GGUF path (optional)
                <input
                  aria-label="Model override value"
                  list="project-detected-models"
                  value={prefs.llmModelOverride ?? ""}
                  placeholder={
                    prefs.llmKindOverride === "managed"
                      ? "D:\\Models\\gemma-4-12b-q4_k_m.gguf"
                      : "e.g. gpt-4o-mini or local-model"
                  }
                  onChange={(e) => setPrefs({ ...prefs, llmModelOverride: e.target.value || null })}
                />
                {detected && detected.llmModels.length > 0 && (
                  <datalist id="project-detected-models">
                    {detected.llmModels.map((m) => (
                      <option key={m} value={m} />
                    ))}
                  </datalist>
                )}
              </label>
            )}
          </fieldset>

          <fieldset>
            <legend>
              Audio after processing <AudioRetentionHelp />
            </legend>
            <label>
              Keep audio of this project&apos;s meetings
              <select
                aria-label="Audio retention override"
                value={prefs.audioRetention ?? ""}
                onChange={(e) =>
                  setPrefs({
                    ...prefs,
                    audioRetention:
                      e.target.value === "" ? null : (e.target.value as "keep" | "delete"),
                  })
                }
              >
                <option value="">
                  Use the global setting
                  {globalKeepsAudio === null
                    ? ""
                    : globalKeepsAudio
                      ? " (currently: keep audio)"
                      : " (currently: delete audio)"}
                </option>
                <option value="keep">Keep the audio</option>
                <option value="delete">Delete the audio after processing</option>
              </select>
              <small>
                Deleting happens only after a meeting was processed successfully and its report is
                ready. The transcript, report, key screenshots and voice sample clips are always
                kept.
              </small>
            </label>
          </fieldset>

          <div className="actions">
            <button className="btn btn-ghost" type="button" onClick={onClose}>
              Close
            </button>
            <button className="btn btn-record" type="submit" disabled={!prefsDirty || savingPrefs}>
              {savingPrefs ? "Saving…" : "Save local preferences"}
            </button>
          </div>
        </form>
      )}

      {tab === "local" && (
        <DeleteProjectSection
          cycleId={cycleId}
          projectName={cycle?.name ?? cycleId}
          onDeleted={() => {
            onDeleted?.();
            if (!onDeleted) onClose();
          }}
        />
      )}

      {tab !== "local" && cycle && details && (
        <div className="form">
          {issues.length > 0 && (
            <div className="issues" role="alert" aria-label="Problems">
              <strong>
                {issues.length === 1 ? "1 problem" : `${issues.length} problems`} in project config:
              </strong>
              <ul>
                {issues.map((i, n) => (
                  <li key={n}>{issueText(i)}</li>
                ))}
              </ul>
            </div>
          )}

          {tab === "people" && (
            <>
              <div className="form-row">
                <div className="field-block">
                  <div className="field-label-row">
                    <label htmlFor={projectIdInputId}>Project ID</label>
                    <HelpPopover
                      title="Project ID"
                      text="The folder name in the repository and issue label prefix. It cannot be renamed after creation."
                    />
                  </div>
                  <input id={projectIdInputId} value={cycle.id} readOnly aria-readonly="true" />
                </div>
                <label>
                  Project name
                  <input
                    aria-label="Project name"
                    value={cycle.name}
                    onChange={(e) => patchCycle({ name: e.target.value })}
                  />
                  <FieldProblems issues={issues} path="name" />
                </label>
              </div>

              <h4 className="field-head">Participants</h4>
              <p className="section-hint">
                People in this project. Voice recognition and task assignees link to each
                person&apos;s ID.
              </p>
              <ul className="rows" aria-label="Participants">
                {cycle.participants.map((p, i) => {
                  const at = `participants[${i}]`;
                  const kind = p.discord ? (p.discord.roleId !== undefined ? "role" : "user") : "";
                  const discordId = p.discord?.userId ?? p.discord?.roleId ?? "";
                  return (
                    <li key={i} className="row-grid">
                      <label>
                        ID
                        <input
                          aria-label={`Participant ${i + 1} id`}
                          value={p.id}
                          onChange={(e) => setParticipant(i, { id: e.target.value })}
                        />
                        <FieldProblems issues={issues} path={`${at}.id`} />
                      </label>
                      <label>
                        Name
                        <input
                          aria-label={`Participant ${i + 1} name`}
                          value={p.name}
                          onChange={(e) => setParticipant(i, { name: e.target.value })}
                        />
                        <FieldProblems issues={issues} path={`${at}.name`} />
                      </label>
                      <label>
                        GitHub login
                        <input
                          aria-label={`Participant ${i + 1} GitHub login`}
                          value={p.github ?? ""}
                          onChange={(e) =>
                            setParticipant(i, { github: e.target.value || undefined })
                          }
                        />
                        <FieldProblems issues={issues} path={`${at}.github`} />
                      </label>
                      <label>
                        Discord
                        <select
                          aria-label={`Participant ${i + 1} Discord kind`}
                          value={kind}
                          onChange={(e) => {
                            const v = e.target.value;
                            setParticipant(i, {
                              discord:
                                v === ""
                                  ? undefined
                                  : v === "role"
                                    ? { roleId: discordId }
                                    : { userId: discordId },
                            });
                          }}
                        >
                          <option value="">None</option>
                          <option value="user">User ID</option>
                          <option value="role">Role ID</option>
                        </select>
                      </label>
                      {kind !== "" && (
                        <label>
                          Discord ID
                          <input
                            aria-label={`Participant ${i + 1} Discord id`}
                            value={discordId}
                            inputMode="numeric"
                            onChange={(e) =>
                              setParticipant(i, {
                                discord:
                                  kind === "role"
                                    ? { roleId: e.target.value }
                                    : { userId: e.target.value },
                              })
                            }
                          />
                          <FieldProblems issues={issues} path={`${at}.discord.user_id`} />
                          <FieldProblems issues={issues} path={`${at}.discord.role_id`} />
                        </label>
                      )}
                      <button
                        className="btn btn-small btn-ghost row-remove"
                        type="button"
                        aria-label={`Remove participant ${i + 1}`}
                        onClick={() =>
                          patchCycle({
                            participants: cycle.participants.filter((_, j) => j !== i),
                          })
                        }
                      >
                        Remove
                      </button>
                    </li>
                  );
                })}
              </ul>
              <button
                className="btn btn-small"
                type="button"
                onClick={() =>
                  patchCycle({ participants: [...cycle.participants, emptyParticipant()] })
                }
              >
                Add participant
              </button>

              <h4 className="field-head">Glossary</h4>
              <p className="section-hint">
                Project acronyms or domain terms to spell accurately in transcripts and notes.
              </p>
              <ul className="rows" aria-label="Glossary">
                {cycle.glossary.map((g, i) => (
                  <li key={i} className="row-grid">
                    <label>
                      Term
                      <input
                        aria-label={`Glossary term ${i + 1}`}
                        value={g.term}
                        onChange={(e) =>
                          patchCycle({
                            glossary: cycle.glossary.map((x, j) =>
                              j === i ? { ...x, term: e.target.value } : x,
                            ),
                          })
                        }
                      />
                      <FieldProblems issues={issues} path={`glossary[${i}].term`} />
                    </label>
                    <label>
                      Hint
                      <input
                        aria-label={`Glossary hint ${i + 1}`}
                        value={g.hint ?? ""}
                        onChange={(e) =>
                          patchCycle({
                            glossary: cycle.glossary.map((x, j) =>
                              j === i ? { ...x, hint: e.target.value || undefined } : x,
                            ),
                          })
                        }
                      />
                    </label>
                    <button
                      className="btn btn-small btn-ghost row-remove"
                      type="button"
                      aria-label={`Remove glossary term ${i + 1}`}
                      onClick={() =>
                        patchCycle({ glossary: cycle.glossary.filter((_, j) => j !== i) })
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
                onClick={() => patchCycle({ glossary: [...cycle.glossary, { term: "" }] })}
              >
                Add term
              </button>
            </>
          )}

          {tab === "report" && (
            <>
              <div className="form-row">
                <label>
                  Transcription language
                  <select
                    aria-label="Transcription language"
                    value={cycle.languages.asr}
                    onChange={(e) =>
                      patchCycle({
                        languages: { ...cycle.languages, asr: e.target.value as LanguageSetting },
                      })
                    }
                  >
                    {LANGUAGES.map((l) => (
                      <option key={l.value} value={l.value}>
                        {l.label}
                      </option>
                    ))}
                  </select>
                </label>
                <label>
                  Report language
                  <select
                    aria-label="Report language"
                    value={cycle.languages.output}
                    onChange={(e) =>
                      patchCycle({
                        languages: {
                          ...cycle.languages,
                          output: e.target.value as LanguageSetting,
                        },
                      })
                    }
                  >
                    {LANGUAGES.map((l) => (
                      <option key={l.value} value={l.value}>
                        {l.label}
                      </option>
                    ))}
                  </select>
                </label>
              </div>

              <label>
                Custom AI instruction
                <textarea
                  rows={3}
                  value={cycle.report.instruction}
                  placeholder="Group tasks by person, keep summaries concise."
                  onChange={(e) => setReport({ instruction: e.target.value })}
                />
              </label>

              <details className="advanced-section">
                <summary>Advanced</summary>
                <div className="form" style={{ marginTop: "0.75rem" }}>
                  <div className="form-row">
                    <div className="field-block">
                      <div className="field-label-row">
                        <label htmlFor="project-default-mode">Default report mode</label>
                        <HelpPopover
                          title="Default report mode"
                          text="Controls how the AI structures each report: Fixed uses the built-in format, Template uses a custom section layout, and Auto picks based on the meeting type. Leave on Auto unless you want every meeting in this project to use one fixed layout."
                          recommended="Auto"
                        />
                      </div>
                      <select
                        id="project-default-mode"
                        aria-label="Default report mode"
                        value={cycle.report.defaultMode}
                        onChange={(e) => setReport({ defaultMode: e.target.value as ReportMode })}
                      >
                        {REPORT_MODES.map((m) => (
                          <option key={m} value={m}>
                            {MODE_NAMES[m]}
                          </option>
                        ))}
                      </select>
                      <small>{MODE_HELP[cycle.report.defaultMode]}</small>
                    </div>
                    <div className="field-block">
                      <div className="field-label-row">
                        <label htmlFor="project-default-template">Default template</label>
                        <HelpPopover
                          title="Default template"
                          text="The section template used when a meeting uses Template mode or has no matching meeting type. Change this only if you created a custom template below."
                          recommended="default"
                        />
                      </div>
                      <select
                        id="project-default-template"
                        aria-label="Default template"
                        value={cycle.report.defaultTemplate}
                        onChange={(e) => setReport({ defaultTemplate: e.target.value })}
                      >
                        {!templateNames.includes(cycle.report.defaultTemplate) && (
                          <option value={cycle.report.defaultTemplate}>
                            {cycle.report.defaultTemplate} (missing)
                          </option>
                        )}
                        {templateNames.map((n) => (
                          <option key={n} value={n}>
                            {n}
                          </option>
                        ))}
                      </select>
                    </div>
                  </div>

                  <div className="field-label-row">
                    <h4 className="field-head" style={{ margin: 0 }}>
                      Meeting types
                    </h4>
                    <HelpPopover
                      title="Meeting types"
                      text="Named meeting kinds (such as standup or review) that map automatically to a specific report template when selected before recording. Add entries here only if your team uses different report layouts for different recurring meetings."
                    />
                  </div>
                  <ul className="rows" aria-label="Meeting types">
                    {rows.map((r, i) => (
                      <li key={i} className="row-grid">
                        <label>
                          Type
                          <input
                            aria-label={`Meeting type ${i + 1}`}
                            value={r.type}
                            onChange={(e) =>
                              setRows(
                                rows.map((x, j) => (j === i ? { ...x, type: e.target.value } : x)),
                              )
                            }
                          />
                        </label>
                        <label>
                          Template
                          <select
                            aria-label={`Template of meeting type ${i + 1}`}
                            value={r.template}
                            onChange={(e) =>
                              setRows(
                                rows.map((x, j) =>
                                  j === i ? { ...x, template: e.target.value } : x,
                                ),
                              )
                            }
                          >
                            {!templateNames.includes(r.template) && (
                              <option value={r.template}>{r.template || "Choose…"}</option>
                            )}
                            {templateNames.map((n) => (
                              <option key={n} value={n}>
                                {n}
                              </option>
                            ))}
                          </select>
                        </label>
                        <button
                          className="btn btn-small btn-ghost row-remove"
                          type="button"
                          onClick={() => setRows(rows.filter((_, j) => j !== i))}
                        >
                          Remove
                        </button>
                      </li>
                    ))}
                  </ul>
                  {dupTypes.length > 0 && (
                    <p className="error" role="alert">
                      Duplicate meeting types: {dupTypes.join(", ")}.
                    </p>
                  )}
                  <button
                    className="btn btn-small"
                    type="button"
                    onClick={() =>
                      setRows([...rows, { type: "", template: cycle.report.defaultTemplate }])
                    }
                  >
                    Add meeting type
                  </button>
                </div>
              </details>

              <h4 className="field-head">Templates</h4>
              <TemplateEditor
                cycleId={cycleId}
                templates={details.templates}
                onChanged={() =>
                  void backend
                    .getCycle(cycleId)
                    .then((d) =>
                      setDetails((cur) =>
                        cur
                          ? { ...cur, templates: d.templates, issues: d.issues, valid: d.valid }
                          : d,
                      ),
                    )
                }
              />
            </>
          )}

          {tab === "integrations" && (
            <div className="form targets">
              <fieldset>
                <legend>GitHub</legend>
                <div
                  className="integration-row"
                  style={{
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "space-between",
                    gap: "0.75rem",
                  }}
                >
                  <label className="check" style={{ margin: 0 }}>
                    <input
                      type="checkbox"
                      checked={github != null}
                      onChange={(e) =>
                        setTargets({
                          github: e.target.checked
                            ? { path: "meetings", publish: DEFAULT_PUBLISH_RULE.github }
                            : null,
                        })
                      }
                    />
                    Commit meeting reports to the repository
                  </label>
                  <button
                    type="button"
                    className="btn btn-small btn-ghost"
                    aria-label="GitHub details"
                    aria-expanded={openTargets.github}
                    onClick={() => setOpenTargets((cur) => ({ ...cur, github: !cur.github }))}
                  >
                    {openTargets.github ? "Hide" : "Details"}
                  </button>
                </div>
                {openTargets.github && (
                  <div className="form" style={{ marginTop: "0.75rem" }}>
                    <div
                      className="form"
                      role="form"
                      aria-label={`GitHub repository of ${details.repoId}`}
                    >
                      <label>
                        GitHub repository of {details.repoId} (owner/name)
                        <input
                          value={githubRepoInput}
                          placeholder="owner/name"
                          autoComplete="off"
                          onChange={(e) => {
                            setGithubRepoInput(e.target.value);
                            setRepoMsg(null);
                          }}
                        />
                        <small>
                          Leave empty when the Git remote is already on github.com; it is read from
                          the remote automatically.
                        </small>
                      </label>
                      <label>
                        GitHub API address (optional)
                        <input
                          aria-label="GitHub API address"
                          value={githubApiInput}
                          placeholder="https://api.github.com"
                          autoComplete="off"
                          onChange={(e) => {
                            setGithubApiInput(e.target.value);
                            setRepoMsg(null);
                          }}
                        />
                        <small>Only for GitHub Enterprise servers. Empty uses github.com.</small>
                      </label>
                      {repoMsg && (
                        <p
                          className={repoMsg.kind === "error" ? "error" : "secret-result"}
                          role={repoMsg.kind === "error" ? "alert" : "status"}
                        >
                          {repoMsg.text}
                        </p>
                      )}
                      <div className="actions">
                        <button
                          type="button"
                          className="btn btn-small btn-ghost"
                          disabled={repoBusy || repoLinkDirty}
                          title={
                            repoLinkDirty ? "Save the repository link before testing." : undefined
                          }
                          onClick={() => void testGithubLink()}
                        >
                          Test access
                        </button>
                        <button
                          type="button"
                          className="btn btn-small"
                          disabled={repoBusy || !repoLinkDirty}
                          onClick={() => void saveGithubLink()}
                        >
                          Save link
                        </button>
                      </div>
                    </div>
                    {github && (
                      <>
                        <div className="field-block">
                          <div className="field-label-row">
                            <label htmlFor={reportFolderInputId}>Folder for meeting reports</label>
                            <HelpPopover
                              title="Report folder"
                              text="Relative folder inside the Git repository where Markdown reports are committed (use forward slashes)."
                              recommended="meetings"
                            />
                          </div>
                          <input
                            id={reportFolderInputId}
                            value={github.path}
                            onChange={(e) =>
                              setTargets({ github: { ...github, path: e.target.value } })
                            }
                          />
                          <FieldProblems issues={issues} path="targets.github.path" />
                        </div>
                        <label className="check">
                          <input
                            type="checkbox"
                            checked={github.issues !== false}
                            onChange={(e) =>
                              setTargets({
                                github: { ...github, issues: e.target.checked ? undefined : false },
                              })
                            }
                          />
                          Create GitHub issues for tasks and write back approved status changes
                        </label>
                        <div className="field-group">
                          <span className="field-label">When to publish</span>
                          <RuleChoice
                            name="GitHub"
                            target="github"
                            value={github.publish ?? DEFAULT_PUBLISH_RULE.github}
                            onChange={(publish) => setTargets({ github: { ...github, publish } })}
                          />
                        </div>
                      </>
                    )}
                  </div>
                )}
              </fieldset>

              <fieldset>
                <legend>Discord</legend>
                <div
                  className="integration-row"
                  style={{
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "space-between",
                    gap: "0.75rem",
                  }}
                >
                  <label className="check" style={{ margin: 0 }}>
                    <input
                      type="checkbox"
                      checked={discordRef != null}
                      onChange={(e) =>
                        setTargets({
                          discord: e.target.checked
                            ? {
                                webhook: `cred:discord/${cycle.id.toLowerCase()}`,
                                publish: DEFAULT_PUBLISH_RULE.discord,
                              }
                            : null,
                        })
                      }
                    />
                    Post a summary to a Discord channel
                  </label>
                  <button
                    type="button"
                    className="btn btn-small btn-ghost"
                    aria-label="Discord details"
                    aria-expanded={openTargets.discord && discordRef != null}
                    onClick={() => setOpenTargets((cur) => ({ ...cur, discord: !cur.discord }))}
                  >
                    {openTargets.discord && discordRef != null ? "Hide" : "Details"}
                  </button>
                </div>
                {openTargets.discord && discordRef && (
                  <div className="form" style={{ marginTop: "0.75rem" }}>
                    <div className="field-block">
                      <div className="field-label-row">
                        <label htmlFor={webhookRefInputId}>Webhook credential reference</label>
                        <HelpPopover
                          title="Credential reference"
                          text="Use cred:name to reference a Discord webhook stored safely in Windows Credential Manager. Never paste raw webhook URLs here."
                          recommended="cred:discord/project"
                        />
                      </div>
                      <input
                        id={webhookRefInputId}
                        value={discordRef.webhook}
                        placeholder="cred:discord/nomad"
                        onChange={(e) =>
                          setTargets({ discord: { ...discordRef, webhook: e.target.value } })
                        }
                      />
                      <FieldProblems issues={issues} path="targets.discord.webhook" />
                    </div>

                    {validWebhookRef && (
                      <div
                        className="secret-form"
                        role="form"
                        aria-label={`Webhook for ${trimmedWebhookRef}`}
                      >
                        <p className="token-state">
                          {isWebhookSet
                            ? matchedSecret?.masked
                              ? `Webhook stored: ${matchedSecret.masked}`
                              : "Webhook stored"
                            : "No webhook stored"}
                        </p>
                        <label htmlFor={webhookSecretInputId}>
                          Discord webhook URL
                          <input
                            id={webhookSecretInputId}
                            aria-label={`Webhook for ${trimmedWebhookRef}`}
                            type="password"
                            autoComplete="off"
                            spellCheck={false}
                            value={webhookInput}
                            placeholder={
                              isWebhookSet
                                ? "Enter a new value to replace it"
                                : "https://discord.com/api/webhooks/…"
                            }
                            onChange={(e) => {
                              setWebhookInput(e.target.value);
                              setWebhookMsg(null);
                            }}
                          />
                          <small>
                            Stored in Windows Credential Manager under {trimmedWebhookRef}; the URL
                            is never written to Git or shown again.
                          </small>
                        </label>
                        {webhookMsg && (
                          <p
                            className={webhookMsg.kind === "error" ? "error" : "secret-result"}
                            role={webhookMsg.kind === "error" ? "alert" : "status"}
                          >
                            {webhookMsg.text}
                          </p>
                        )}
                        <div className="actions">
                          <button
                            type="button"
                            className="btn btn-small btn-ghost"
                            disabled={!isWebhookSet || webhookBusy}
                            onClick={() => void testDiscordSecret()}
                          >
                            Test webhook
                          </button>
                          <button
                            type="button"
                            className="btn btn-small btn-ghost"
                            disabled={!isWebhookSet || webhookBusy}
                            onClick={() => void clearDiscordWebhook()}
                          >
                            Clear
                          </button>
                          <button
                            type="button"
                            className="btn btn-small"
                            disabled={webhookInput.trim() === "" || webhookBusy}
                            onClick={() => void storeDiscordWebhook()}
                          >
                            Store
                          </button>
                        </div>
                      </div>
                    )}

                    <label>
                      Message template
                      <select
                        aria-label="Discord template"
                        value={discordRef.template ?? ""}
                        onChange={(e) =>
                          setTargets({
                            discord: { ...discordRef, template: e.target.value || undefined },
                          })
                        }
                      >
                        <option value="">Same as report</option>
                        {templateNames.map((n) => (
                          <option key={n} value={n}>
                            {n}
                          </option>
                        ))}
                      </select>
                    </label>
                    <div className="field-group">
                      <span className="field-label">When to publish</span>
                      <RuleChoice
                        name="Discord"
                        target="discord"
                        value={discordRef.publish ?? DEFAULT_PUBLISH_RULE.discord}
                        onChange={(publish) => setTargets({ discord: { ...discordRef, publish } })}
                      />
                    </div>
                  </div>
                )}
              </fieldset>

              <fieldset>
                <legend>Notion</legend>
                <div
                  className="integration-row"
                  style={{
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "space-between",
                    gap: "0.75rem",
                  }}
                >
                  <label className="check" style={{ margin: 0 }}>
                    <input
                      type="checkbox"
                      checked={notion != null}
                      onChange={(e) =>
                        setTargets({
                          notion: e.target.checked
                            ? { database: "", publish: "on-approval" }
                            : null,
                        })
                      }
                    />
                    Add the report to a Notion database
                  </label>
                  <button
                    type="button"
                    className="btn btn-small btn-ghost"
                    aria-label="Notion details"
                    aria-expanded={openTargets.notion && notion != null}
                    onClick={() => setOpenTargets((cur) => ({ ...cur, notion: !cur.notion }))}
                  >
                    {openTargets.notion && notion != null ? "Hide" : "Details"}
                  </button>
                </div>
                {openTargets.notion && notion && (
                  <div className="form" style={{ marginTop: "0.75rem" }}>
                    <div className="field-block">
                      <div className="field-label-row">
                        <label htmlFor={notionDbInputId}>Database ID or cred: reference</label>
                        <HelpPopover
                          title="Notion database"
                          text="Provide a Notion database ID or a cred: reference stored in Windows Credential Manager."
                        />
                      </div>
                      <input
                        id={notionDbInputId}
                        value={notion.database}
                        placeholder="cred:notion/project-db"
                        onChange={(e) =>
                          setTargets({ notion: { ...notion, database: e.target.value } })
                        }
                      />
                      <FieldProblems issues={issues} path="targets.notion.database" />
                    </div>
                    <label>
                      When to publish
                      <select
                        aria-label="Notion publish"
                        value={notion.publish}
                        onChange={(e) =>
                          setTargets({
                            notion: { ...notion, publish: e.target.value as PublishMode },
                          })
                        }
                      >
                        {PUBLISH_OPTIONS.map((p) => (
                          <option key={p.value} value={p.value}>
                            {p.label}
                          </option>
                        ))}
                      </select>
                    </label>
                  </div>
                )}
              </fieldset>
            </div>
          )}

          <div className="actions">
            <button className="btn btn-ghost" type="button" onClick={onClose}>
              Close
            </button>
            <button
              className="btn btn-record"
              type="button"
              disabled={!cycleDirty || savingCycle}
              onClick={() => void saveCycleSettings()}
            >
              {savingCycle ? "Saving…" : "Save project settings"}
            </button>
          </div>
        </div>
      )}
    </Modal>
  );
}
