export const ENGLISH_MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
] as const;

export interface EnglishDateTimePickerProps {
  /** Local datetime string `YYYY-MM-DDTHH:MM` or empty string. */
  value: string;
  onChange: (next: string) => void;
  ariaLabel: string;
}

interface ParsedParts {
  year: string;
  month: string;
  day: string;
  hour: string;
  minute: string;
}

const PAD2 = (n: number) => String(n).padStart(2, "0");

function daysInMonth(year: number, month1To12: number): number {
  return new Date(year, month1To12, 0).getDate();
}

function parseLocalValue(value: string): ParsedParts | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/.exec(value);
  if (!m) return null;
  return {
    year: m[1],
    month: m[2],
    day: m[3],
    hour: m[4],
    minute: m[5],
  };
}

function defaultParts(): ParsedParts {
  const now = new Date();
  now.setMinutes(0, 0, 0);
  now.setHours(now.getHours() + 1);
  return {
    year: String(now.getFullYear()),
    month: PAD2(now.getMonth() + 1),
    day: PAD2(now.getDate()),
    hour: PAD2(now.getHours()),
    minute: "00",
  };
}

/**
 * App-controlled local date and time picker with English month and field labels,
 * independent of the operating system locale in WebView2.
 */
export function EnglishDateTimePicker(props: EnglishDateTimePickerProps) {
  const { value, onChange, ariaLabel } = props;
  const parsed = parseLocalValue(value);

  const currentYear = new Date().getFullYear();
  const baseYears = [currentYear, currentYear + 1, currentYear + 2, currentYear + 3];
  const parsedYearNum = parsed ? Number(parsed.year) : NaN;
  const years =
    Number.isFinite(parsedYearNum) && !baseYears.includes(parsedYearNum)
      ? [...baseYears, parsedYearNum].sort((a, b) => a - b)
      : baseYears;

  const activeYear = parsed ? Number(parsed.year) : currentYear;
  const activeMonth = parsed ? Number(parsed.month) : 1;
  const maxDay = daysInMonth(activeYear, activeMonth);
  const days = Array.from({ length: maxDay }, (_, i) => PAD2(i + 1));
  const hours = Array.from({ length: 24 }, (_, i) => PAD2(i));
  const minutes = Array.from({ length: 60 }, (_, i) => PAD2(i));

  const updatePart = (field: keyof ParsedParts, nextVal: string) => {
    if (!nextVal) {
      onChange("");
      return;
    }
    const next: ParsedParts = { ...(parsed ?? defaultParts()), [field]: nextVal };
    const maxD = daysInMonth(Number(next.year), Number(next.month));
    if (Number(next.day) > maxD) {
      next.day = PAD2(maxD);
    }
    onChange(`${next.year}-${next.month}-${next.day}T${next.hour}:${next.minute}`);
  };

  return (
    <div className="english-dt-picker" role="group" aria-label={ariaLabel}>
      <select
        className="english-dt-select"
        aria-label="Year"
        value={parsed?.year ?? ""}
        onChange={(e) => updatePart("year", e.target.value)}
      >
        <option value="">Year</option>
        {years.map((y) => (
          <option key={y} value={String(y)}>
            {y}
          </option>
        ))}
      </select>
      <select
        className="english-dt-select"
        aria-label="Month"
        value={parsed?.month ?? ""}
        onChange={(e) => updatePart("month", e.target.value)}
      >
        <option value="">Month</option>
        {ENGLISH_MONTHS.map((name, idx) => (
          <option key={name} value={PAD2(idx + 1)}>
            {name}
          </option>
        ))}
      </select>
      <select
        className="english-dt-select"
        aria-label="Day"
        value={parsed?.day ?? ""}
        onChange={(e) => updatePart("day", e.target.value)}
      >
        <option value="">Day</option>
        {days.map((d) => (
          <option key={d} value={d}>
            {d}
          </option>
        ))}
      </select>
      <span className="english-dt-sep" aria-hidden="true">
        at
      </span>
      <select
        className="english-dt-select"
        aria-label="Hour"
        value={parsed?.hour ?? ""}
        onChange={(e) => updatePart("hour", e.target.value)}
      >
        <option value="">HH</option>
        {hours.map((h) => (
          <option key={h} value={h}>
            {h}
          </option>
        ))}
      </select>
      <span className="english-dt-sep" aria-hidden="true">
        :
      </span>
      <select
        className="english-dt-select"
        aria-label="Minute"
        value={parsed?.minute ?? ""}
        onChange={(e) => updatePart("minute", e.target.value)}
      >
        <option value="">MM</option>
        {minutes.map((m) => (
          <option key={m} value={m}>
            {m}
          </option>
        ))}
      </select>
    </div>
  );
}
