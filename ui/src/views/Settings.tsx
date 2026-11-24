import { useCallback, useEffect, useRef, useState } from "react";
import { UnsavedDialog } from "../components/UnsavedDialog";
import { AiModelsTab } from "./settings/AiModelsTab";
import { ConnectorsTab } from "./settings/ConnectorsTab";
import { PreferencesTab } from "./settings/PreferencesTab";
import { ProcessingTab } from "./settings/ProcessingTab";
import { VoiceProfiles } from "./VoiceProfiles";

export type SettingsTab = "models" | "connectors" | "preferences" | "processing" | "voices";

export interface SettingsTabHandle {
  save: () => Promise<boolean>;
  discard: () => void;
}

export interface EditableTabProps {
  onDirtyChange: (dirty: boolean) => void;
  registerHandle: (handle: SettingsTabHandle | null) => void;
}

const TABS: { id: SettingsTab; label: string }[] = [
  { id: "models", label: "AI Models" },
  { id: "connectors", label: "Connectors" },
  { id: "preferences", label: "Preferences" },
  { id: "processing", label: "Processing" },
  { id: "voices", label: "Voice Profiles" },
];

export interface SettingsViewProps {
  initialTab?: SettingsTab;
  onDirtyChange?: (dirty: boolean) => void;
  pendingExit?: boolean;
  onConfirmExit?: () => void;
  onCancelExit?: () => void;
}

/**
 * Tabbed application settings with left section navigation, unsaved-change tracking,
 * and confirmation before switching away from a modified tab.
 */
export function SettingsView(props: SettingsViewProps = {}) {
  const { initialTab, onDirtyChange, pendingExit, onConfirmExit, onCancelExit } = props;
  const [tab, setTab] = useState<SettingsTab>(initialTab ?? "models");
  const [dirty, setDirty] = useState(false);
  const [pendingTab, setPendingTab] = useState<SettingsTab | null>(null);
  const [saving, setSaving] = useState(false);
  const handleRef = useRef<SettingsTabHandle | null>(null);

  const registerHandle = useCallback((handle: SettingsTabHandle | null) => {
    handleRef.current = handle;
  }, []);

  useEffect(() => {
    onDirtyChange?.(dirty);
  }, [dirty, onDirtyChange]);

  useEffect(() => {
    if (pendingExit && !dirty) {
      onConfirmExit?.();
    }
  }, [pendingExit, dirty, onConfirmExit]);

  useEffect(() => {
    if (!dirty) return;
    const onBeforeUnload = (e: BeforeUnloadEvent) => {
      e.preventDefault();
      e.returnValue = "";
    };
    window.addEventListener("beforeunload", onBeforeUnload);
    return () => window.removeEventListener("beforeunload", onBeforeUnload);
  }, [dirty]);

  const selectTab = (next: SettingsTab) => {
    if (next === tab) return;
    if (dirty) {
      setPendingTab(next);
      return;
    }
    setTab(next);
  };

  const handleSave = async () => {
    const h = handleRef.current;
    if (!h) return;
    setSaving(true);
    try {
      const ok = await h.save();
      if (ok) {
        setDirty(false);
        if (pendingTab) {
          setTab(pendingTab);
          setPendingTab(null);
        } else if (pendingExit) {
          onConfirmExit?.();
        }
      } else {
        setPendingTab(null);
        onCancelExit?.();
      }
    } finally {
      setSaving(false);
    }
  };

  const handleDiscard = () => {
    handleRef.current?.discard();
    setDirty(false);
    if (pendingTab) {
      setTab(pendingTab);
      setPendingTab(null);
    } else if (pendingExit) {
      onConfirmExit?.();
    }
  };

  return (
    <section className="settings-view" aria-labelledby="settings-heading">
      <div className="page-head">
        <h2 id="settings-heading">
          <span className="glitch" data-text="Settings">
            Settings
          </span>
        </h2>
      </div>

      <div className="settings-shell">
        <nav className="settings-nav" role="tablist" aria-label="Settings sections">
          {TABS.map((t) => {
            const active = tab === t.id;
            return (
              <button
                key={t.id}
                type="button"
                role="tab"
                id={`settings-tab-${t.id}`}
                aria-selected={active}
                aria-controls={`settings-panel-${t.id}`}
                className={active ? "nav nav-on" : "nav"}
                onClick={() => selectTab(t.id)}
              >
                {t.label}
              </button>
            );
          })}
        </nav>

        <div
          className="settings-pane"
          role="tabpanel"
          id={`settings-panel-${tab}`}
          aria-labelledby={`settings-tab-${tab}`}
        >
          <UnsavedDialog
            open={pendingTab !== null || Boolean(pendingExit)}
            dirty={dirty}
            saving={saving}
            title="You have unsaved changes"
            message={
              pendingExit
                ? "Save your changes before leaving Settings?"
                : "Save your changes before switching to another settings section?"
            }
            onSave={handleSave}
            onDiscard={handleDiscard}
            onCancel={() => {
              setPendingTab(null);
              onCancelExit?.();
            }}
          />

          {tab === "models" && (
            <AiModelsTab onDirtyChange={setDirty} registerHandle={registerHandle} />
          )}
          {tab === "connectors" && <ConnectorsTab />}
          {tab === "preferences" && (
            <PreferencesTab onDirtyChange={setDirty} registerHandle={registerHandle} />
          )}
          {tab === "processing" && (
            <ProcessingTab onDirtyChange={setDirty} registerHandle={registerHandle} />
          )}
          {tab === "voices" && <VoiceProfiles />}
        </div>
      </div>
    </section>
  );
}
