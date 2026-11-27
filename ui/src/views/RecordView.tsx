import { useCallback, useEffect, useState } from "react";
import { useBackend } from "../backend-context";
import { HelpPopover } from "../components/HelpPopover";
import { HotkeyCapture } from "../components/HotkeyCapture";
import { RecorderPanel } from "../components/RecorderPanel";
import { errorMessage } from "../hooks";
import {
  DEFAULT_RECORD_HOTKEY,
  DEFAULT_SCREENSHOT_HOTKEY,
  notifySettingsChanged,
  onSettingsChanged,
} from "../hotkeys";
import type { HotkeyStatus, Settings } from "../ipc/types";

/**
 * Dedicated Main UI recording screen with source selection, live audio levels,
 * project/report configuration, and quick hotkey/source tips.
 */
export function RecordView() {
  const backend = useBackend();
  const [settings, setSettings] = useState<Settings | null>(null);
  const [savingHotkey, setSavingHotkey] = useState(false);
  const [hotkeyError, setHotkeyError] = useState<string | null>(null);
  const [hotkeyStatus, setHotkeyStatus] = useState<HotkeyStatus | null>(null);

  const loadHotkeyStatus = useCallback(() => {
    void backend
      .hotkeyStatus()
      .then((st) => setHotkeyStatus(st))
      .catch(() => {});
  }, [backend]);

  const loadSettings = useCallback(() => {
    let cancelled = false;
    void backend
      .getSettings()
      .then((s) => {
        if (!cancelled) setSettings(s);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [backend]);

  useEffect(() => {
    const cancel = loadSettings();
    loadHotkeyStatus();
    const unsub = onSettingsChanged(() => {
      loadHotkeyStatus();
      void backend
        .getSettings()
        .then((s) => setSettings(s))
        .catch(() => {});
    });
    return () => {
      cancel();
      unsub();
    };
  }, [backend, loadSettings, loadHotkeyStatus]);

  const recordHotkey = settings?.recordHotkey ?? DEFAULT_RECORD_HOTKEY;
  const screenshotHotkey = settings?.screenshotHotkey ?? DEFAULT_SCREENSHOT_HOTKEY;

  const updateHotkey = async (field: "recordHotkey" | "screenshotHotkey", next: string) => {
    setHotkeyError(null);
    setSavingHotkey(true);
    try {
      const latest = await backend.getSettings();
      const updated: Settings = { ...latest, [field]: next };
      await backend.saveSettings(updated);
      setSettings(updated);
      loadHotkeyStatus();
      notifySettingsChanged();
    } catch (err) {
      setHotkeyError(errorMessage(err));
    } finally {
      setSavingHotkey(false);
    }
  };

  return (
    <>
      <div className="page-head">
        <h2>
          <span className="glitch" data-text="Record">
            Record
          </span>
        </h2>
        <HelpPopover
          title="Main window recorder"
          text="Start and monitor a meeting recording directly from the main window, or use the tray widget anytime."
          recommended="Select your microphone plus either the call app or specific captured browser tabs."
        />
      </div>

      <div className="queue-layout-grid">
        <RecorderPanel compact={false} />

        <aside className="panel quiet" aria-label="Recording tips">
          <div className="panel-body">
            <h3 className="card-title">
              Recording tips
              <HelpPopover
                title="Source selection & hotkeys"
                text="Your last picked microphone, apps, and browser tabs are remembered automatically for the next session."
                recommended="Avoid picking both individual Chrome tabs and 'Chrome (whole browser)' at the same time to prevent double audio."
              />
            </h3>
            <p className="section-hint">
              Pick your microphone and the application or browser tab playing the call. Selections
              are saved automatically between restarts.
            </p>
            <div className="record-hotkeys">
              <HotkeyCapture
                id="record-view-record-hotkey"
                label="Record / Stop"
                value={recordHotkey}
                otherHotkey={screenshotHotkey}
                registrationError={hotkeyStatus?.recordError ?? null}
                disabled={savingHotkey}
                onChange={(next) => void updateHotkey("recordHotkey", next)}
              />
              <HotkeyCapture
                id="record-view-screenshot-hotkey"
                label="Region screenshot"
                value={screenshotHotkey}
                otherHotkey={recordHotkey}
                registrationError={hotkeyStatus?.screenshotError ?? null}
                disabled={savingHotkey}
                onChange={(next) => void updateHotkey("screenshotHotkey", next)}
              />
              {hotkeyError && (
                <p className="error" role="alert">
                  {hotkeyError}
                </p>
              )}
            </div>
            <p className="section-hint">
              You can add or remove audio sources and capture region screenshots at any point while
              recording is active.
            </p>
          </div>
        </aside>
      </div>
    </>
  );
}
