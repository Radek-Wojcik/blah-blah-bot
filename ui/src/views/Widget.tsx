import { RecorderPanel } from "../components/RecorderPanel";
import { DetectionPrompt } from "./DetectionPrompt";

/**
 * Compact tray recorder popup backed by `RecorderPanel` with live audio-reactive
 * waveform metering and persisted source selection across restarts.
 */
export function Widget() {
  const showDetection = new URLSearchParams(window.location.search).has("detectms");

  if (!showDetection) {
    return <RecorderPanel compact={true} />;
  }

  return (
    <div style={{ height: "100%", display: "flex", flexDirection: "column" }}>
      <RecorderPanel compact={true} className="grow" />
      <div style={{ padding: "0 clamp(8px, 3vmin, 16px) clamp(8px, 3vmin, 16px)" }}>
        <DetectionPrompt embedded />
      </div>
    </div>
  );
}
