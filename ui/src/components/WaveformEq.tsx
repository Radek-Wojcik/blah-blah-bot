import type { RecordingLevels } from "../ipc/types";

export const EQ_BAR_COUNT = 12;

const ZERO_BARS: number[] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

export interface WaveformEqProps {
  /** Real-time recording levels payload (`peak`, `rms`, `bars`, `peakHold`). */
  levels?: RecordingLevels | null;
  /** Optional direct 12-bar array (`0..1`) override. */
  bars?: number[];
  /** Optional direct peak override (`0..1`). */
  peak?: number;
  /** Optional direct RMS override (`0..1`). */
  rms?: number;
  /** Optional direct peak-hold override (`0..1`). */
  peakHold?: number;
  /** Whether recording is currently active. */
  active?: boolean;
  /** Alias for `active`. */
  recording?: boolean;
  className?: string;
}

/**
 * 12-bar audio waveform equalizer driven by dB-normalized `RecordingLevels` (`bars`, `peak`, `rms`,
 * `peakHold`), scaling bar heights dynamically from actual audio energy with a peak-hold marker.
 */
export function WaveformEq(props: WaveformEqProps) {
  const { levels, bars, peak, rms, peakHold, active, recording, className } = props;

  const effectiveLevels: RecordingLevels = levels ?? {
    peak: peak ?? 0,
    rms: rms ?? 0,
    bars: bars ?? ZERO_BARS,
    peakHold: peakHold ?? peak ?? 0,
  };

  const rawPeakHold = peakHold ?? effectiveLevels.peakHold ?? effectiveLevels.peak;
  const clampedPeakHold = Number.isFinite(rawPeakHold) ? Math.min(1, Math.max(0, rawPeakHold)) : 0;

  const isActive =
    active ??
    recording ??
    (effectiveLevels.peak > 0 ||
      effectiveLevels.rms > 0 ||
      clampedPeakHold > 0 ||
      effectiveLevels.bars.some((b) => b > 0));

  const normalizedBars = Array.from({ length: EQ_BAR_COUNT }, (_, i) => {
    if (!isActive) return 0.12;
    const raw = effectiveLevels.bars[i] ?? 0;
    const energy = Number.isFinite(raw) ? raw : 0;
    return Math.min(1, Math.max(0.08, energy));
  });

  const classes = ["waveform-eq", "eq", isActive ? "waveform-eq-live" : "", className ?? ""]
    .filter(Boolean)
    .join(" ");

  return (
    <span
      className={classes}
      role="img"
      aria-label={
        isActive
          ? `Audio waveform level ${Math.round(effectiveLevels.peak * 100)} percent`
          : "Audio waveform idle"
      }
      data-active={isActive ? "true" : "false"}
      data-peak={effectiveLevels.peak.toFixed(2)}
      data-rms={effectiveLevels.rms.toFixed(2)}
      data-peak-hold={clampedPeakHold.toFixed(2)}
    >
      {normalizedBars.map((val, i) => (
        <i
          key={i}
          className="waveform-eq-bar"
          data-bar-index={i}
          data-level={val.toFixed(2)}
          style={{
            transform: `scaleY(${val.toFixed(3)})`,
          }}
        />
      ))}
      {isActive && clampedPeakHold > 0.02 && (
        <span
          className="waveform-eq-peak"
          aria-hidden="true"
          data-peak-hold={clampedPeakHold.toFixed(2)}
          style={{
            bottom: `${Math.round(clampedPeakHold * 100)}%`,
          }}
        />
      )}
    </span>
  );
}
