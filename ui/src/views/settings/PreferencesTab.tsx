import { useCallback, useEffect, useMemo, useState } from "react";
import { useBackend } from "../../backend-context";
import { HelpPopover } from "../../components/HelpPopover";
import { errorMessage, useCycles } from "../../hooks";
import type {
  CycleListItem,
  CycleTargets,
  PublishMode,
  PublishTargetName,
  Settings,
} from "../../ipc/types";
import { DEFAULT_PUBLISH_RULE, STATUS_WRITE_BACK_NOTE } from "../../publish";
import { BrowserExtension } from "../BrowserExtension";
import { DetectionSettingsView } from "../DetectionSettings";
import { RuleChoice } from "../RuleChoice";
import type { EditableTabProps } from "../Settings";

function normalizeSettings(s: Settings, fallbackSaveDir: string): Settings {
  const localSaveDir =
    s.localSaveDir && s.localSaveDir.trim() !== "" ? s.localSaveDir : fallbackSaveDir;
  return {
    ...s,
    localSaveEnabled: Boolean(s.localSaveEnabled),
    localSaveDir,
  };
}

function isPrefsDirty(saved: Settings | null, draft: Settings | null): boolean {
  if (!saved || !draft) return false;
  if (
    saved.recordingsDir !== draft.recordingsDir ||
    saved.recordHotkey !== draft.recordHotkey ||
    saved.screenshotHotkey !== draft.screenshotHotkey ||
    Boolean(saved.localSaveEnabled) !== Boolean(draft.localSaveEnabled) ||
    (saved.localSaveDir ?? "") !== (draft.localSaveDir ?? "")
  ) {
    return true;
  }
  const a = saved.bridge;
  const b = draft.bridge;
  if (!a || !b) return a !== b;
  return a.enabled !== b.enabled || a.port !== b.port || a.autoAddTabs !== b.autoAddTabs;
}

function ProjectPublishRules({ cycle, version }: { cycle: CycleListItem; version: unknown }) {
  const backend = useBackend();
  const [targets, setTargets] = useState<CycleTargets | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    backend
      .getCycle(cycle.id)
      .then((d) => {
        if (!cancelled) setTargets(d.cycle.targets);
      })
      .catch((e) => {
        if (!cancelled) setError(errorMessage(e));
      });
    return () => {
      cancelled = true;
    };
  }, [backend, cycle.id, version]);

  const change = (target: PublishTargetName, mode: PublishMode) => {
    setError(null);
    setBusy(true);
    backend
      .setTargetRule(cycle.id, target, mode)
      .then(setTargets)
      .catch((e) => setError(errorMessage(e)))
      .finally(() => setBusy(false));
  };

  if (!targets) return error ? <p className="error">{error}</p> : null;
  const github = targets.github?.publish ?? DEFAULT_PUBLISH_RULE.github;
  const discord = targets.discord?.publish ?? null;

  return (
    <div className="panel quiet">
      <div className="panel-body">
        <p className="token-state">
          {cycle.name} <span className="person-meta">{cycle.id}</span>
        </p>
        <div className="rule-row">
          <span className="field-label">GitHub</span>
          <RuleChoice
            name={`GitHub, project ${cycle.name}`}
            target="github"
            value={github}
            disabled={busy}
            onChange={(m) => change("github", m)}
          />
        </div>
        {discord !== null ? (
          <div className="rule-row">
            <span className="field-label">Discord</span>
            <RuleChoice
              name={`Discord, project ${cycle.name}`}
              target="discord"
              value={discord}
              disabled={busy}
              onChange={(m) => change("discord", m)}
            />
          </div>
        ) : (
          <p className="target-rule-help">
            No Discord target on this project. Configure Discord inside Project Settings if needed.
          </p>
        )}
        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}
      </div>
    </div>
  );
}

/**
 * Preferences settings tab: Local Folder Auto-Save, app hotkeys & recordings folder,
 * Chrome extension bridge settings, per-project publish defaults, and meeting detection.
 */
export function PreferencesTab({ onDirtyChange, registerHandle }: EditableTabProps) {
  const backend = useBackend();
  const { cycles } = useCycles();
  const [saved, setSaved] = useState<Settings | null>(null);
  const [draft, setDraft] = useState<Settings | null>(null);
  const [message, setMessage] = useState<{ text: string; kind: "status" | "error" } | null>(null);

  useEffect(() => {
    let cancelled = false;
    void Promise.all([backend.getSettings(), backend.autoDetectModels()])
      .then(([s, detected]) => {
        if (cancelled) return;
        const normalized = normalizeSettings(s, detected.defaultLocalSaveDir);
        setSaved(structuredClone(normalized));
        setDraft(structuredClone(normalized));
      })
      .catch((err) => {
        if (!cancelled) setMessage({ text: errorMessage(err), kind: "error" });
      });
    return () => {
      cancelled = true;
    };
  }, [backend]);

  const dirty = useMemo(() => isPrefsDirty(saved, draft), [saved, draft]);

  useEffect(() => {
    onDirtyChange(dirty);
  }, [dirty, onDirtyChange]);

  useEffect(() => () => onDirtyChange(false), [onDirtyChange]);

  const save = useCallback(async (): Promise<boolean> => {
    if (!draft) return false;
    setMessage(null);
    try {
      await backend.saveSettings(draft);
      setSaved(structuredClone(draft));
      setMessage({ text: "Preferences saved.", kind: "status" });
      return true;
    } catch (err) {
      setMessage({ text: errorMessage(err), kind: "error" });
      return false;
    }
  }, [backend, draft]);

  const discard = useCallback(() => {
    if (saved) setDraft(structuredClone(saved));
    setMessage(null);
  }, [saved]);

  useEffect(() => {
    registerHandle({ save, discard });
    return () => registerHandle(null);
  }, [registerHandle, save, discard]);

  if (!draft) return null;
  const set = (patch: Partial<Settings>) => setDraft({ ...draft, ...patch });
  const bridge = draft.bridge;
  const setBridge = (patch: Partial<NonNullable<Settings["bridge"]>>) =>
    bridge && set({ bridge: { ...bridge, ...patch } });
  const validCycles = (cycles ?? []).filter((c) => c.valid);

  return (
    <div className="settings-pane">
      <div className="panel quiet">
        <div className="panel-body">
          <form
            className="form"
            aria-label="Preferences"
            onSubmit={(e) => {
              e.preventDefault();
              void save();
            }}
          >
            <fieldset className="llm-group">
              <legend>
                <span className="field-label-row">
                  Local folder auto-save
                  <HelpPopover
                    title="Local Folder Auto-Save"
                    recommended="Enable to keep an offline Markdown archive of every meeting"
                  >
                    <p>
                      Writes finished meeting notes and transcripts as plain Markdown files to your
                      chosen local folder automatically. Works completely independently of GitHub or
                      Discord.
                    </p>
                  </HelpPopover>
                </span>
              </legend>
              <label className="check">
                <input
                  type="checkbox"
                  checked={Boolean(draft.localSaveEnabled)}
                  onChange={(e) => set({ localSaveEnabled: e.target.checked })}
                />
                Automatically save meeting notes &amp; transcripts to a local folder
              </label>
              <label>
                Local notes folder
                <input
                  value={draft.localSaveDir ?? ""}
                  placeholder="Documents\BlahBlahBot\notes"
                  onChange={(e) => set({ localSaveDir: e.target.value })}
                />
                <small>Works independently of GitHub or Discord.</small>
              </label>
            </fieldset>

            <fieldset className="llm-group">
              <legend>
                <span className="field-label-row">
                  Recordings &amp; hotkeys
                  <HelpPopover title="Recordings & Hotkeys">
                    <p>
                      Choose where raw meeting audio and screenshots are stored, and configure
                      global keyboard shortcuts for starting/stopping recording and taking region
                      screenshots.
                    </p>
                  </HelpPopover>
                </span>
              </legend>
              <label>
                Recordings folder
                <input
                  value={draft.recordingsDir}
                  onChange={(e) => set({ recordingsDir: e.target.value })}
                />
                <small>Leave empty to use the default folder in Documents.</small>
              </label>
              <div className="settings-grid-2col">
                <label>
                  Record / Stop hotkey
                  <input
                    value={draft.recordHotkey}
                    onChange={(e) => set({ recordHotkey: e.target.value })}
                  />
                </label>
                <label>
                  Region screenshot hotkey
                  <input
                    value={draft.screenshotHotkey}
                    onChange={(e) => set({ screenshotHotkey: e.target.value })}
                  />
                </label>
              </div>
            </fieldset>

            {bridge && (
              <fieldset className="llm-group">
                <legend>Chrome extension bridge</legend>
                <label className="check">
                  <input
                    type="checkbox"
                    checked={bridge.enabled}
                    onChange={(e) => setBridge({ enabled: e.target.checked })}
                  />
                  Accept the Chrome extension on this computer
                </label>
                <label>
                  Bridge port
                  <input
                    type="number"
                    min={1024}
                    max={65535}
                    value={bridge.port}
                    onChange={(e) => setBridge({ port: Number(e.target.value) })}
                  />
                  <small>
                    If taken, the next free port up to {bridge.port + 9} is used automatically.
                  </small>
                </label>
                <label className="check">
                  <input
                    type="checkbox"
                    checked={bridge.autoAddTabs}
                    onChange={(e) => setBridge({ autoAddTabs: e.target.checked })}
                  />
                  Record a tab as soon as the extension sends it during a recording
                </label>
              </fieldset>
            )}

            {message && (
              <p
                className={message.kind === "error" ? "error" : "secret-result"}
                role={message.kind === "error" ? "alert" : "status"}
              >
                {message.text}
              </p>
            )}
            <div className="actions">
              <button type="submit" className="btn btn-record" disabled={!dirty}>
                Save preferences
              </button>
            </div>
          </form>
        </div>
      </div>

      <section aria-labelledby="publish-defaults-heading">
        <div className="field-label-row">
          <h3 id="publish-defaults-heading" className="sub-head">
            When to publish
          </h3>
          <HelpPopover
            title="When to publish"
            recommended="On approval so you can review notes and tasks before sharing"
          >
            <p>
              <strong>Off:</strong> Never publishes to that target.
            </p>
            <p>
              <strong>On approval:</strong> Waits until you review the draft and click Publish.
            </p>
            <p>
              <strong>Automatic:</strong> Publishes right after processing finishes.
            </p>
          </HelpPopover>
        </div>
        <p className="section-hint">
          Choose per project whether publishing is Off, On approval, or Automatic.{" "}
          {STATUS_WRITE_BACK_NOTE}
        </p>
        {validCycles.length === 0 ? (
          <p className="empty">No usable projects yet. Create one on the Cycles page.</p>
        ) : (
          <div className="secret-list">
            {validCycles.map((c) => (
              <ProjectPublishRules key={c.id} cycle={c} version={cycles} />
            ))}
          </div>
        )}
      </section>

      <DetectionSettingsView />
      <BrowserExtension />
    </div>
  );
}
