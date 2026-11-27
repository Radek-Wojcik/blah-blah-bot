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

/** Parse a form number; an empty or non-numeric field is NaN, never 0. */
export function parseFormNumber(text: string): number {
  const trimmed = text.trim();
  return trimmed === "" ? Number.NaN : Number(trimmed);
}

/**
 * Synchronized range slider and compact number input sharing the same validation bounds.
 * While the number field is empty or not a number the slider keeps its last position.
 */
export function RangeNumberField(props: RangeNumberFieldProps) {
  const { id, label, unit, min, max, step = 1, value, onChange, help, sliderAriaLabel } = props;
  const parsed = parseFormNumber(value);
  const numeric = Number.isFinite(parsed);
  const sliderValue = numeric ? Math.min(max, Math.max(min, parsed)) : min;

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
          aria-valuetext={numeric ? `${parsed}${unit ? ` ${unit}` : ""}` : "no value"}
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
        {unit && (
          <span className="range-number-unit" aria-hidden="true">
            {unit}
          </span>
        )}
      </div>
    </div>
  );
}
