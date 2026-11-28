import type { PublishMode, PublishTargetName } from "../ipc/types";
import { RULE_HELP, RULE_ORDER, RULE_SHORT, autoWarning } from "../publish";
import { RuleSwitch } from "./RuleSwitch";

/**
 * The rule of a target, chosen explicitly: the switch of three positions, and under it what each
 * position does in plain words, with the one in force marked. `On approval` is what a target has
 * until a rule is picked, so nothing is ever published without the user having chosen it.
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
}) {
  const { name, target, value, disabledReason } = props;
  const isDisabled = Boolean(props.disabled || disabledReason);
  const warning = disabledReason ? null : autoWarning(target, value);
  return (
    <div className="rule-choice">
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
