import { useRef } from "react";
import type { PublishMode } from "../ipc/types";
import { RULE_ORDER, RULE_SHORT } from "../publish";

/**
 * When a target is published, as a switch of three positions: Off, On approval, Automatic. It is
 * a radio group: one position is chosen, the arrow keys move it, and the choice is the position
 * that has focus.
 */
export function RuleSwitch(props: {
  /** `GitHub` or `Discord`: names the group for assistive technology. */
  name: string;
  value: PublishMode;
  disabled?: boolean;
  onChange: (mode: PublishMode) => void;
}) {
  const { name, value, disabled } = props;
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const choose = (i: number) => {
    const next = RULE_ORDER[(i + RULE_ORDER.length) % RULE_ORDER.length];
    refs.current[RULE_ORDER.indexOf(next)]?.focus();
    if (next !== value) props.onChange(next);
  };
  const onKey = (e: React.KeyboardEvent, i: number) => {
    if (e.key === "ArrowRight" || e.key === "ArrowDown") choose(i + 1);
    else if (e.key === "ArrowLeft" || e.key === "ArrowUp") choose(i - 1);
    else if (e.key === "Home") choose(0);
    else if (e.key === "End") choose(RULE_ORDER.length - 1);
    else return;
    e.preventDefault();
  };
  return (
    <span
      className="pane-toggle rule-switch"
      role="radiogroup"
      aria-label={`${name} publishing rule`}
    >
      {RULE_ORDER.map((rule, i) => {
        const on = rule === value;
        return (
          <button
            key={rule}
            ref={(el) => {
              refs.current[i] = el;
            }}
            type="button"
            role="radio"
            aria-checked={on}
            tabIndex={on ? 0 : -1}
            disabled={disabled}
            className={on ? "btn btn-small btn-stop" : "btn btn-small btn-ghost"}
            onClick={() => !on && props.onChange(rule)}
            onKeyDown={(e) => onKey(e, i)}
          >
            {RULE_SHORT[rule]}
          </button>
        );
      })}
    </span>
  );
}
