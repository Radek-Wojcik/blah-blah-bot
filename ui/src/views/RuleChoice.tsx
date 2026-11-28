import type { PublishMode, PublishTargetName, RuleSource } from "../ipc/types";
import { RULE_HELP, RULE_ORDER, RULE_SHORT, autoWarning } from "../publish";
import { RuleSwitch } from "./RuleSwitch";

/**
 * The rule of a target, chosen explicitly: the switch of three positions, and under it what each
 * position does in plain words, with the one in force marked. `On approval` is what a target has
 * until a rule is picked, so nothing is ever published without the user having chosen it.
 *
 * With `inherit` the project may also follow the global rule of Settings > Preferences: a
 * "Use global setting (currently: X)" choice above the switch. The switch then shows the rule in
 * force (`value`); pressing a position makes it the project's own rule.
 */
export function RuleChoice(props: {
  /** `GitHub` or `Discord`, with the cycle when several are on one page. */
  name: string;
  target: PublishTargetName;
  value: PublishMode;
  disabled?: boolean;
  /** When set, disables the switch and displays this message below it. */
  disabledReason?: string | null;
  onChange: (mode: PublishMode) => void;
  /**
   * Offers "Use global setting": `global` is the global rule, `source` where `value` comes from,
   * `onInherit` removes the project's own rule.
   */
  inherit?: { global: PublishMode; source: RuleSource; onInherit: () => void };
}) {
  const { name, target, value, disabledReason, inherit } = props;
  const isDisabled = Boolean(props.disabled || disabledReason);
  const warning = disabledReason ? null : autoWarning(target, value);
  return (
    <div className="rule-choice">
      {inherit && (
        <label className="check rule-inherit">
          <input
            type="checkbox"
            checked={inherit.source === "global"}
            disabled={isDisabled}
            aria-label={`${name}: use global setting`}
            onChange={(e) => (e.target.checked ? inherit.onInherit() : props.onChange(value))}
          />
          <span>
            Use global setting (currently: {RULE_SHORT[inherit.global]})
            <small className="target-what">
              {inherit.source === "global"
                ? ` In use: ${RULE_SHORT[value]}, from the global setting.`
                : ` In use: ${RULE_SHORT[value]}, set for this project only.`}
            </small>
          </span>
        </label>
      )}
      <RuleSwitch name={name} value={value} disabled={isDisabled} onChange={props.onChange} />
      {disabledReason && (
        <p className="target-rule-help" role="note">
          {disabledReason}
        </p>
      )}
      <ul className="rule-options" aria-label={`What each ${name} rule does`}>
        {RULE_ORDER.map((rule) => (
          <li key={rule} className={rule === value ? "rule-option rule-option-on" : "rule-option"}>
            <strong>{RULE_SHORT[rule]}</strong>
            {rule === value && <span className="rule-chosen"> (in use)</span>}
            {": "}
            {RULE_HELP[rule]}
          </li>
        ))}
      </ul>
      {warning && <small className="target-rule-warn">{warning}</small>}
    </div>
  );
}
