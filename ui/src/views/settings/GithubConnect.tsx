import { useCallback, useEffect, useRef, useState } from "react";
import { useBackend } from "../../backend-context";
import { errorMessage } from "../../hooks";
import type { GithubDeviceStart } from "../../ipc/types";
import { safeHttpsUrl } from "../../publish";

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
export function GithubConnect(props: { tokenSet: boolean; onChanged: () => void }) {
  const backend = useBackend();
  const { tokenSet, onChanged } = props;
  const [configured, setConfigured] = useState<boolean | null>(null);
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });
  const [message, setMessage] = useState<{ text: string; kind: "status" | "error" } | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const alive = useRef(true);
  const running = useRef(false);

  useEffect(() => {
    alive.current = true;
    void backend
      .githubConnectStatus()
      .then((s) => alive.current && setConfigured(s.configured))
      .catch((e) => alive.current && setMessage({ text: errorMessage(e), kind: "error" }));
    return () => {
      alive.current = false;
      if (timer.current) clearTimeout(timer.current);
      if (running.current) void backend.githubDeviceCancel().catch(() => undefined);
    };
  }, [backend]);

  const stop = useCallback(() => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    running.current = false;
  }, []);

  const pollRef = useRef<(seconds: number) => void>(() => undefined);
  const poll = useCallback(
    (seconds: number) => {
      timer.current = setTimeout(
        () => {
          void backend
            .githubDevicePoll()
            .then((r) => {
              if (!alive.current || !running.current) return;
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
              if (!alive.current) return;
              stop();
              setPhase({ kind: "idle" });
              setMessage({ text: errorMessage(e), kind: "error" });
            });
        },
        Math.max(1, seconds) * 1000,
      );
    },
    [backend, onChanged, stop],
  );

  useEffect(() => {
    pollRef.current = poll;
  }, [poll]);

  const connect = () => {
    setMessage(null);
    setPhase({ kind: "starting" });
    backend
      .githubDeviceStart()
      .then((start) => {
        if (!alive.current) return;
        running.current = true;
        setPhase({ kind: "waiting", start });
        poll(start.interval);
      })
      .catch((e) => {
        if (!alive.current) return;
        setPhase({ kind: "idle" });
        setMessage({ text: errorMessage(e), kind: "error" });
      });
  };

  const cancel = () => {
    stop();
    void backend.githubDeviceCancel().catch(() => undefined);
    setPhase({ kind: "idle" });
  };

  const disconnect = () => {
    setMessage(null);
    backend
      .clearGithubToken()
      .then(() => {
        setPhase({ kind: "idle" });
        onChanged();
        setMessage({ text: "Disconnected: the stored token was removed.", kind: "status" });
      })
      .catch((e) => setMessage({ text: errorMessage(e), kind: "error" }));
  };

  const page = phase.kind === "waiting" ? safeHttpsUrl(phase.start.verificationUri) : null;

  return (
    <div className="settings-subcard" aria-label="GitHub account sign-in">
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
          <button
            type="button"
            className="btn"
            disabled={configured !== true || phase.kind === "starting"}
            title={configured === false ? "Not configured: use a token below" : undefined}
            onClick={connect}
          >
            {phase.kind === "starting" ? "Starting…" : "Connect account"}
          </button>
        )}
        {tokenSet && phase.kind !== "waiting" && (
          <button type="button" className="btn btn-ghost" onClick={disconnect}>
            Disconnect
          </button>
        )}
      </div>
    </div>
  );
}
