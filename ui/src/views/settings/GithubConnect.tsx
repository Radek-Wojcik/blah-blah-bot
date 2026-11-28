import { useCallback, useEffect, useRef, useState } from "react";
import { useBackend } from "../../backend-context";
import { errorMessage } from "../../hooks";
import type { GithubDeviceStart } from "../../ipc/types";
import { safeHttpsUrl } from "../../publish";

/** Slack added to GitHub's interval so a timer that fires a hair early never costs a whole round. */
const POLL_MARGIN_MS = 250;

/** Attempt ids, shared by every instance: a newer attempt anywhere outranks an older one. */
let lastAttempt = 0;
/** The id of the most recent Connect press, in any instance. */
let lastConnect = 0;

/** The verification page: exactly https://github.com/login/device (no port, login, query). */
function githubPage(uri: string): string | null {
  const safe = safeHttpsUrl(uri);
  if (!safe) return null;
  try {
    const u = new URL(safe);
    const ok =
      u.protocol === "https:" &&
      u.hostname === "github.com" &&
      u.port === "" &&
      u.username === "" &&
      u.password === "" &&
      u.search === "" &&
      u.hash === "" &&
      (u.pathname === "/login/device" || u.pathname === "/login/device/");
    return ok ? safe : null;
  } catch {
    return null;
  }
}

type Phase =
  | { kind: "idle" }
  | { kind: "starting" }
  | { kind: "waiting"; start: GithubDeviceStart }
  | { kind: "connected"; login: string | null };

/**
 * "Connect account" for github.com: the OAuth device flow. The person gets a code, approves it on
 * GitHub in the system browser and this component polls until the backend has stored the token.
 * Without a configured client id it says so and leaves the token field below as the way in.
 */
export function GithubConnect(props: {
  tokenSet: boolean;
  /** Changes whenever a token is stored or removed in the field below (same value = nothing). */
  tokenEpoch?: number;
  onChanged: () => void;
}) {
  const backend = useBackend();
  const { tokenSet, tokenEpoch, onChanged } = props;
  const [configured, setConfigured] = useState<boolean | null>(null);
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });
  const [message, setMessage] = useState<{ text: string; kind: "status" | "error" } | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const alive = useRef(true);
  /** A connection is starting or waiting: the backend holds (or is about to hold) a session. */
  const running = useRef(false);
  /** Bumped by every start, cancel and disconnect: an answer of an older attempt is dropped. */
  const attempt = useRef(0);
  const prevTokenSet = useRef(tokenSet);
  const prevEpoch = useRef(tokenEpoch);
  const [disconnecting, setDisconnecting] = useState(false);

  useEffect(() => {
    alive.current = true;
    void backend
      .githubConnectStatus()
      .then((s) => alive.current && setConfigured(s.configured))
      .catch((e) => alive.current && setMessage({ text: errorMessage(e), kind: "error" }));
    return () => {
      alive.current = false;
      if (timer.current) clearTimeout(timer.current);
      if (running.current) {
        attempt.current = ++lastAttempt;
        void backend.githubDeviceCancel().catch(() => undefined);
      }
    };
  }, [backend]);

  const stop = useCallback(() => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    running.current = false;
  }, []);

  /** Ends the attempt in progress, if any, and tells the backend to forget its device code. */
  const abandon = useCallback(() => {
    attempt.current = ++lastAttempt;
    stop();
    void backend.githubDeviceCancel().catch(() => undefined);
  }, [backend, stop]);

  // The token was removed or replaced in the field below: "Connected as ..." would be a lie.
  useEffect(() => {
    const removed = prevTokenSet.current && !tokenSet;
    const replaced = prevEpoch.current !== tokenEpoch;
    prevTokenSet.current = tokenSet;
    prevEpoch.current = tokenEpoch;
    if (removed || replaced) {
      setPhase((p) => (p.kind === "connected" ? { kind: "idle" } : p));
    }
  }, [tokenSet, tokenEpoch]);

  const pollRef = useRef<(seconds: number) => void>(() => undefined);
  const poll = useCallback(
    (seconds: number) => {
      const mine = attempt.current;
      timer.current = setTimeout(
        () => {
          if (!alive.current || attempt.current !== mine) return;
          void backend
            .githubDevicePoll()
            .then((r) => {
              if (!alive.current || attempt.current !== mine) return;
              switch (r.state) {
                case "pending":
                  pollRef.current(seconds);
                  break;
                case "slowDown":
                  pollRef.current(r.interval);
                  break;
                case "connected":
                  stop();
                  setPhase({ kind: "connected", login: r.login });
                  setMessage(null);
                  onChanged();
                  break;
                case "denied":
                  stop();
                  setPhase({ kind: "idle" });
                  setMessage({
                    text: "GitHub access was denied. Try again, or use a token below.",
                    kind: "error",
                  });
                  break;
                case "expired":
                  stop();
                  setPhase({ kind: "idle" });
                  setMessage({
                    text: "The code expired before it was approved. Press Connect account to get a new one.",
                    kind: "error",
                  });
                  break;
              }
            })
            .catch((e) => {
              if (!alive.current || attempt.current !== mine) return;
              // The backend dropped the session with the error; make sure of it.
              abandon();
              setPhase({ kind: "idle" });
              setMessage({ text: errorMessage(e), kind: "error" });
            });
        },
        Math.max(1, seconds) * 1000 + POLL_MARGIN_MS,
      );
    },
    [abandon, backend, onChanged, stop],
  );

  useEffect(() => {
    pollRef.current = poll;
  }, [poll]);

  const connect = () => {
    // Synchronous guard: a second click before the state has caught up does nothing.
    if (running.current) return;
    running.current = true;
    const mine = ++lastAttempt;
    attempt.current = mine;
    lastConnect = mine;
    setMessage(null);
    setPhase({ kind: "starting" });
    backend
      .githubDeviceStart()
      .then((start) => {
        if (!alive.current || attempt.current !== mine) {
          // Cancelled, disconnected or unmounted while starting: drop the session just made,
          // unless a newer Connect (maybe in a fresh instance) has already replaced it.
          if (lastConnect === mine) void backend.githubDeviceCancel().catch(() => undefined);
          return;
        }
        setPhase({ kind: "waiting", start });
        poll(start.interval);
      })
      .catch((e) => {
        if (!alive.current || attempt.current !== mine) return;
        running.current = false;
        setPhase({ kind: "idle" });
        setMessage({ text: errorMessage(e), kind: "error" });
      });
  };

  const cancel = () => {
    abandon();
    setPhase({ kind: "idle" });
  };

  const disconnect = () => {
    if (disconnecting) return;
    setMessage(null);
    setDisconnecting(true);
    backend
      .clearGithubToken()
      .then(() => {
        if (!alive.current) return;
        setDisconnecting(false);
        if (running.current) abandon();
        setPhase({ kind: "idle" });
        onChanged();
        setMessage({ text: "Disconnected: the stored token was removed.", kind: "status" });
      })
      .catch((e) => {
        if (!alive.current) return;
        setDisconnecting(false);
        setMessage({ text: errorMessage(e), kind: "error" });
      });
  };

  const page = phase.kind === "waiting" ? githubPage(phase.start.verificationUri) : null;

  return (
    <section className="settings-subcard" aria-label="GitHub account sign-in">
      <p className="token-state" style={{ marginTop: 0 }}>
        Connect your GitHub account
      </p>
      {configured === false && (
        <p className="person-meta" role="status" data-testid="github-connect-unconfigured">
          Account sign-in is not configured in this build. Use a token below instead.
        </p>
      )}
      {phase.kind === "connected" && (
        <p className="secret-result" role="status" data-testid="github-connected">
          {phase.login ? `Connected as ${phase.login}` : "Connected"}
        </p>
      )}
      {phase.kind === "waiting" && (
        <div role="group" aria-label="Approve on GitHub">
          <p className="person-meta">Enter this code on GitHub to approve the connection:</p>
          <p className="device-code" data-testid="github-device-code" style={{ fontSize: "1.5em" }}>
            <strong>{phase.start.userCode}</strong>
          </p>
          {page ? (
            <a className="btn btn-small" href={page} target="_blank" rel="noreferrer noopener">
              Open github.com/login/device
            </a>
          ) : (
            <p className="error" role="alert">
              GitHub did not name a safe page to open.
            </p>
          )}
          <p className="person-meta" role="status">
            Waiting for approval…
          </p>
        </div>
      )}
      {message && (
        <p
          className={message.kind === "error" ? "error" : "secret-result"}
          role={message.kind === "error" ? "alert" : "status"}
        >
          {message.text}
        </p>
      )}
      <div className="actions">
        {phase.kind === "waiting" ? (
          <button type="button" className="btn btn-ghost" onClick={cancel}>
            Cancel
          </button>
        ) : (
          <>
            <button
              type="button"
              className="btn"
              disabled={configured !== true || phase.kind === "starting" || disconnecting}
              title={configured === false ? "Not configured: use a token below" : undefined}
              onClick={connect}
            >
              {phase.kind === "starting" ? "Starting…" : "Connect account"}
            </button>
            {phase.kind === "starting" && (
              <button type="button" className="btn btn-ghost" onClick={cancel}>
                Cancel
              </button>
            )}
          </>
        )}
        {tokenSet && (phase.kind === "idle" || phase.kind === "connected") && (
          <button
            type="button"
            className="btn btn-ghost"
            disabled={disconnecting}
            onClick={disconnect}
          >
            Disconnect
          </button>
        )}
      </div>
    </section>
  );
}
