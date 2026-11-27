import type { ReactNode } from "react";

export interface RangeNumberFieldProps {
  id: string;
  label: string;
  unit?: string;
  min: number;
  max: number;
  step?: number;
  value: string;
  onChange: (next: string) => void;
  help?: ReactNode;
  sliderAriaLabel?: string;
}

/**
 * Synchronized range slider and compact number input sharing the same validation bounds.
 */
export function RangeNumberField(props: RangeNumberFieldProps) {
  const { id, label, unit, min, max, step = 1, value, onChange, help, sliderAriaLabel } = props;
  const parsed = Number(value);
  const sliderValue = Number.isFinite(parsed) ? Math.min(max, Math.max(min, parsed)) : min;

  return (
    <div className="field-block">
      <div className="field-label-row">
        <label htmlFor={id}>{label}</label>
        {help}
      </div>
      <div className="range-number-row">
        <input
          type="range"
          className="range-slider"
          aria-label={sliderAriaLabel ?? `${label} slider`}
          min={min}
          max={max}
          step={step}
          value={sliderValue}
          onChange={(e) => onChange(e.target.value)}
        />
        <input
          id={id}
          type="number"
          className="range-number-input"
          min={min}
          max={max}
          step={step}
          value={value}
          onChange={(e) => onChange(e.target.value)}
        />
        {unit && <span className="range-number-unit">{unit}</span>}
      </div>
    </div>
  );
}
