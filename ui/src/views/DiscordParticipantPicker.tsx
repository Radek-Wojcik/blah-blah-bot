import { useState } from "react";
import { useBackend } from "../backend-context";
import { errorMessage } from "../hooks";
import type { DiscordMember, DiscordRef, DiscordRole } from "../ipc/types";
import { discordMemberLabel } from "../publish";

/**
 * Inline Discord server member search (and role list) for a participant row when the cycle has a
 * Discord bot target configured. Selecting a result populates the participant's `discord.userId`
 * or `discord.roleId`; manual numeric ID entry remains available alongside it.
 */
export function DiscordParticipantPicker(props: {
  cycleId: string;
  participantId: string;
  participantIndex: number;
  participantName: string;
  kind: "" | "user" | "role";
  onPick: (discord: DiscordRef) => void;
}) {
  const { cycleId, participantId, participantIndex, participantName, kind, onPick } = props;
  const backend = useBackend();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [members, setMembers] = useState<DiscordMember[] | null>(null);
  const [roles, setRoles] = useState<DiscordRole[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const labelPrefix = `Participant ${participantIndex + 1}`;

  const pick = (discord: DiscordRef) => {
    onPick(discord);
    setOpen(false);
    if (participantId.trim() !== "") {
      void backend.setParticipantDiscord(cycleId, participantId, discord).catch(() => undefined);
    }
  };

  const searchMembers = () => {
    const q = query.trim();
    if (Array.from(q).length < 2) {
      setError("Type at least 2 characters to search Discord members.");
      return;
    }
    setBusy(true);
    setError(null);
    setRoles(null);
    backend
      .searchDiscordMembers(cycleId, q)
      .then(setMembers)
      .catch((e) => {
        setMembers(null);
        setError(errorMessage(e));
      })
      .finally(() => setBusy(false));
  };

  const loadRoles = () => {
    setBusy(true);
    setError(null);
    setMembers(null);
    backend
      .listDiscordRoles(cycleId)
      .then(setRoles)
      .catch((e) => {
        setRoles(null);
        setError(errorMessage(e));
      })
      .finally(() => setBusy(false));
  };

  if (!open) {
    return (
      <div className="picker-toggle">
        <button
          className="btn btn-small btn-ghost"
          type="button"
          aria-label={`Search Discord for ${participantName || labelPrefix}`}
          onClick={() => {
            setOpen(true);
            setError(null);
            if (kind === "role") loadRoles();
          }}
        >
          {kind === "role" ? "Pick Discord role" : "Search Discord member"}
        </button>
      </div>
    );
  }

  return (
    <div className="discord-picker" aria-label={`${labelPrefix} Discord search`}>
      <div className="picker-controls">
        <input
          aria-label={`${labelPrefix} Discord member search`}
          value={query}
          placeholder="Search username or nickname…"
          onChange={(e) => {
            setQuery(e.target.value);
            setError(null);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              searchMembers();
            }
          }}
        />
        <button className="btn btn-small" type="button" disabled={busy} onClick={searchMembers}>
          Search
        </button>
        <button
          className="btn btn-small btn-ghost"
          type="button"
          disabled={busy}
          onClick={loadRoles}
        >
          Roles
        </button>
        <button
          className="btn btn-small btn-ghost"
          type="button"
          onClick={() => {
            setOpen(false);
            setError(null);
          }}
        >
          Close
        </button>
      </div>

      {error && (
        <small className="field-issue" role="alert">
          {error}
        </small>
      )}

      {members !== null &&
        (members.length === 0 ? (
          <small className="person-meta">
            No server members matched &ldquo;{query.trim()}&rdquo;.
          </small>
        ) : (
          <ul className="picker-results" aria-label={`${labelPrefix} Discord member results`}>
            {members.map((m) => (
              <li key={m.userId}>
                <button
                  type="button"
                  className="btn btn-small btn-ghost picker-item"
                  onClick={() => pick({ userId: m.userId })}
                >
                  <span>{discordMemberLabel(m)}</span>
                  <span className="person-meta">{m.userId}</span>
                </button>
              </li>
            ))}
          </ul>
        ))}

      {roles !== null &&
        (roles.length === 0 ? (
          <small className="person-meta">No roles found in this server.</small>
        ) : (
          <ul className="picker-results" aria-label={`${labelPrefix} Discord role results`}>
            {roles.map((r) => (
              <li key={r.roleId}>
                <button
                  type="button"
                  className="btn btn-small btn-ghost picker-item"
                  onClick={() => pick({ roleId: r.roleId })}
                >
                  <span>
                    @{r.name}
                    {!r.mentionable && <span className="person-meta"> (not mentionable)</span>}
                  </span>
                  <span className="person-meta">{r.roleId}</span>
                </button>
              </li>
            ))}
          </ul>
        ))}
    </div>
  );
}
