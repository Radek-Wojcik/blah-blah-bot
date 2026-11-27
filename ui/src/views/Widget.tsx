import { RecorderPanel } from "../components/RecorderPanel";
import { DetectionPrompt } from "./DetectionPrompt";
import { ProcessPrompt } from "./ProcessPrompt";

/**
 * Compact tray recorder popup backed by `RecorderPanel` with live audio-reactive
 * waveform metering and persisted source selection across restarts.
 */
export function Widget() {
  const showDetection = new URLSearchParams(window.location.search).has("detectms");

  if (!showDetection) {
    return (
      <>
        <RecorderPanel compact={true} />
        <ProcessPrompt />
      </>
    );
  }

  return (
    <div className="widget-shell">
      <RecorderPanel compact={true} className="grow" />
      <div style={{ padding: "0 clamp(8px, 3vmin, 16px) clamp(8px, 3vmin, 16px)" }}>
        <DetectionPrompt embedded />
      </div>
      <ProcessPrompt />
    </div>
  );
}
