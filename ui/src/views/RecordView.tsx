import { useEffect, useState } from "react";
import { useBackend } from "../backend-context";
import { HelpPopover } from "../components/HelpPopover";
import { RecorderPanel } from "../components/RecorderPanel";
import type { Settings } from "../ipc/types";

export interface RecordViewProps {
  onProcessNow?: (meetingId: string) => void;
  onAddToQueue?: (meetingId: string) => void;
}

/**
 * Dedicated Main UI recording screen with source selection, live audio levels,
 * project/report configuration, and quick hotkey/source tips.
 */
export function RecordView(props: RecordViewProps = {}) {
  const backend = useBackend();
  const [settings, setSettings] = useState<Settings | null>(null);

  useEffect(() => {
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

  const recordHotkey = settings?.recordHotkey || "Ctrl+Alt+R";
  const screenshotHotkey = settings?.screenshotHotkey || "Ctrl+Alt+S";

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
        <RecorderPanel
          compact={false}
          onProcessNow={props.onProcessNow}
          onAddToQueue={props.onAddToQueue}
        />

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
            <dl className="facts">
              <div>
                <dt>Record / Stop</dt>
                <dd>{recordHotkey}</dd>
              </div>
              <div>
                <dt>Region screenshot</dt>
                <dd>{screenshotHotkey}</dd>
              </div>
            </dl>
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
