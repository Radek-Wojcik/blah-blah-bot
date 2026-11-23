import { useEffect, useState } from "react";
import { useBackend } from "../backend-context";
import { bridgeSummary, clientDetails, groupSummary, pairingTimeLeft } from "../bridge";
import { errorMessage } from "../hooks";
import type { BridgeStatus } from "../ipc/types";

/**
 * Settings > Browser extension: the state of the local bridge, pairing with a one-time code
 * and the list of paired extensions. The on/off switch and the options are part of the
 * settings form above.
 */
export function BrowserExtension() {
  const backend = useBackend();
  const [status, setStatus] = useState<BridgeStatus | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    let alive = true;
    void backend.bridgeStatus().then((s) => alive && setStatus(s));
    const off = backend.onBridgeChanged((s) => setStatus(s));
    return () => {
      alive = false;
      void off.then((f) => f());
    };
  }, [backend]);

  const pairing = status?.pairing ?? null;
  useEffect(() => {
    if (!pairing) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [pairing]);

  if (!status) return null;
  const run = (p: Promise<BridgeStatus>) => {
    setMessage(null);
    p.then(setStatus).catch((e) => setMessage(errorMessage(e)));
  };
  const left = pairing ? pairingTimeLeft(pairing.expiresUnixMs, now) : null;

  return (
    <section aria-labelledby="extension-heading">
      <h3 id="extension-heading" className="sub-head">
        Browser extension
      </h3>
      <div className="panel quiet" style={{ maxWidth: 620 }}>
        <div className="panel-body">
          <p data-testid="bridge-state">{bridgeSummary(status)}</p>
          {status.groups.map((g, i) => (
            <p key={g.groupId} data-testid="bridge-group">
              {groupSummary(g, i)}
            </p>
          ))}
          {pairing && left ? (
            <div aria-label="Pairing code">
              <p>
                Open the extension in Chrome, choose Pair and enter this code. It works once and
                expires in <span aria-label="Time left">{left}</span>.
              </p>
              <p className="pair-code" data-testid="pairing-code">
                {pairing.code}
              </p>
              <div className="actions">
                <button className="btn" onClick={() => run(backend.cancelPairing())}>
                  Cancel pairing
                </button>
              </div>
            </div>
          ) : (
            <div className="actions">
              <button
                className="btn btn-record"
                disabled={!status.listening}
                onClick={() => run(backend.startPairing())}
              >
                Pair extension
              </button>
            </div>
          )}
          {message && <p role="alert">{message}</p>}
        </div>
      </div>
      {status.clients.length === 0 ? (
        <p className="empty">No extension is paired yet.</p>
      ) : (
        <ul className="people" aria-label="Paired extensions">
          {status.clients.map((c) => (
            <li key={c.id} className="person">
              <span className="person-main">
                <span>{c.name}</span>
                <span className="person-meta">{clientDetails(c)}</span>
              </span>
              <button
                className="btn"
                aria-label={`Unpair ${c.name}`}
                onClick={() => run(backend.unpairClient(c.id))}
              >
                Unpair
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
