import { useEffect, useState } from "react";
import { useBackend } from "../backend-context";
import { bannerText, firstOpenStep, type StepId } from "../first-run";
import type { SetupStatus } from "../ipc/types";

/**
 * A short notice above the meetings while something that processing needs is missing, with the
 * button that opens the setup at the first thing to fix. It shows nothing when the setup is
 * complete, and it never stops the user from doing anything else.
 */
export function ReadinessBanner(props: { onOpenSetup: (step: StepId) => void }) {
  const backend = useBackend();
  const [status, setStatus] = useState<SetupStatus | null>(null);
  useEffect(() => {
    let cancelled = false;
    let off: (() => void) | undefined;
    const load = () => void backend.setupStatus().then((s) => !cancelled && setStatus(s));
    load();
    void backend.onModelsChanged(load).then((u) => {
      if (cancelled) u();
      else off = u;
    });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [backend]);

  const text = bannerText(status);
  if (!text) return null;
  return (
    <div className="notice readiness-banner" role="status">
      <span>{text}</span>
      <button
        type="button"
        className="btn btn-small"
        onClick={() => props.onOpenSetup(firstOpenStep(status))}
      >
        Open setup
      </button>
    </div>
  );
}
