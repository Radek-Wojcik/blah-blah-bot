import { useId, useState } from "react";
import {
  BOT_INTENTS_NOTE,
  BOT_PERMISSIONS_VALUE,
  REQUIRED_BOT_PERMISSIONS,
  discordInviteUrl,
} from "../publish";

/**
 * Generates the Discord OAuth2 bot invite link from an Application ID, displays the exact
 * permission integer (`309237746688`) and required permissions, and states that zero privileged
 * intents are required.
 */
export function DiscordInviteBox(props: { compact?: boolean }) {
  const [appId, setAppId] = useState("");
  const helpId = useId();
  const trimmed = appId.trim();
  const url = trimmed === "" ? null : discordInviteUrl(trimmed);
  const invalid = trimmed !== "" && url === null;

  return (
    <div className="discord-invite-box" aria-label="Discord bot invite link">
      <p className="notice">{BOT_INTENTS_NOTE} (0 privileged intents required.)</p>
      <label>
        Discord Application ID (for invite link)
        <input
          value={appId}
          inputMode="numeric"
          placeholder="123456789012345678"
          aria-describedby={helpId}
          onChange={(e) => setAppId(e.target.value)}
        />
        <small id={helpId}>
          Copy Application ID from the Discord Developer Portal (General Information). Scope:{" "}
          <code>bot</code>, permissions integer: <code>{BOT_PERMISSIONS_VALUE}</code>.
        </small>
      </label>
      {invalid && (
        <p className="field-issue" role="alert">
          Enter a 17 to 20 digit Discord Application ID to build the invite link.
        </p>
      )}
      {url && (
        <p className="secret-result" role="status">
          Invite URL:{" "}
          <a href={url} target="_blank" rel="noreferrer noopener">
            {url}
          </a>
        </p>
      )}
      {!props.compact && (
        <ul className="perm-list" aria-label="Required bot permissions">
          {REQUIRED_BOT_PERMISSIONS.map((p) => (
            <li key={p.name}>
              <code>{p.name}</code> — {p.why}
              {p.threadOnly && <span className="person-meta"> (threads only)</span>}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
