import { useEffect, useState } from "react";
import { useBackend } from "../backend-context";
import { HelpPopover } from "../components/HelpPopover";
import { errorMessage, useQueue } from "../hooks";
import type { JobView, ProcessingSettings, QueueGpuOverride, StageView } from "../ipc/types";
import { jobStateLabel, percent, stageLabel, stageSkipText, stageStatusLabel } from "../processing";

const DEFAULT_MAX_UTIL = 40;
const DEFAULT_IDLE_SECONDS = 30;

/** Format remaining ETA seconds into a compact human-readable string like `~4m 20s left`. */
export function formatEta(etaSeconds: number | null | undefined): string | null {
  if (etaSeconds === null || etaSeconds === undefined || !Number.isFinite(etaSeconds)) {
    return null;
  }
  if (etaSeconds < 0) return null;
  const total = Math.round(etaSeconds);
  if (total === 0) return "< 1s left";
  if (total < 60) return `~${total}s left`;
  if (total < 3600) {
    const minutes = Math.floor(total / 60);
    const seconds = total % 60;
    return seconds > 0 ? `~${minutes}m ${seconds}s left` : `~${minutes}m left`;
  }
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  return minutes > 0 ? `~${hours}h ${minutes}m left` : `~${hours}h left`;
}

function toDateTimeLocalValue(unixMs: number): string {
  const d = new Date(unixMs);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
}

function formatScheduledLabel(unixMs: number): string {
  return new Date(unixMs).toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** The processing queue: live jobs on the left and temporary GPU overrides on the right. */
export function QueueView(props: { onOpenDraft?: (meetingId: string) => void }) {
  const backend = useBackend();
  const queue = useQueue();
  const [globalSettings, setGlobalSettings] = useState<ProcessingSettings | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void backend
      .getProcessingSettings()
      .then((s) => {
        if (!cancelled) setGlobalSettings(s);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [backend]);

  const act = async (p: Promise<unknown>): Promise<boolean> => {
    setError(null);
    try {
      await p;
      return true;
    } catch (e) {
      setError(errorMessage(e));
      return false;
    }
  };

  return (
    <>
      <div className="page-head">
        <h2>
          <span className="glitch" data-text="Queue">
            Queue
          </span>
        </h2>
        <span className="chip">{queue ? queue.jobs.length : 0} jobs</span>
      </div>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <div className="queue-layout-grid">
        <div>
          {queue && queue.jobs.length === 0 ? (
            <p className="empty">
              Nothing is queued. Process a meeting from a project or start a recording on the Record
              page.
            </p>
          ) : (
            <ul className="jobs" aria-label="Processing jobs">
              {(queue?.jobs ?? []).map((job) => (
                <li key={job.meetingId}>
                  <JobCard
                    job={job}
                    onOpenDraft={props.onOpenDraft && (() => props.onOpenDraft?.(job.meetingId))}
                    onRetry={() => void act(backend.retryJob(job.meetingId))}
                    onCancel={() => void act(backend.cancelJob(job.meetingId))}
                    onForceProcess={() => void act(backend.forceProcess(job.meetingId))}
                    onSchedule={(unixMs) => act(backend.scheduleJob(job.meetingId, unixMs))}
                  />
                </li>
              ))}
            </ul>
          )}
        </div>

        <GpuOverridePanel
          activeOverride={queue?.gpuOverride ?? null}
          defaultMaxUtil={globalSettings?.gpuMaxUtilPercent ?? DEFAULT_MAX_UTIL}
          defaultIdleSeconds={globalSettings?.gpuIdleSeconds ?? DEFAULT_IDLE_SECONDS}
          onApply={(cfg) => act(backend.setQueueGpuOverride(cfg))}
          onReset={() => act(backend.setQueueGpuOverride(null))}
        />
      </div>
    </>
  );
}

function GpuOverridePanel(props: {
  activeOverride: QueueGpuOverride | null;
  defaultMaxUtil: number;
  defaultIdleSeconds: number;
  onApply: (overrideCfg: QueueGpuOverride) => Promise<boolean>;
  onReset: () => Promise<boolean>;
}) {
  const { activeOverride, defaultMaxUtil, defaultIdleSeconds, onApply, onReset } = props;
  const [draft, setDraft] = useState<{ maxUtil: string; idleSec: string } | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [validationError, setValidationError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const maxUtilValue =
    draft?.maxUtil ?? String(activeOverride?.gpuMaxUtilPercent ?? defaultMaxUtil);
  const idleSecValue =
    draft?.idleSec ?? String(activeOverride?.gpuIdleSeconds ?? defaultIdleSeconds);

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    setStatus(null);
    setValidationError(null);
    const maxUtil = Number(maxUtilValue);
    const idleSec = Number(idleSecValue);
    if (!Number.isFinite(maxUtil) || maxUtil < 0 || maxUtil > 100) {
      setValidationError("Max GPU utilization must be between 0% and 100%.");
      return;
    }
    if (!Number.isFinite(idleSec) || idleSec < 0 || idleSec > 3600) {
      setValidationError("Wait for idle seconds must be between 0 and 3600.");
      return;
    }
    setBusy(true);
    void onApply({
      gpuMaxUtilPercent: Math.round(maxUtil),
      gpuIdleSeconds: Math.round(idleSec),
    })
      .then((ok) => {
        if (ok) {
          setDraft(null);
          setStatus("Temporary GPU override applied.");
        }
      })
      .finally(() => setBusy(false));
  };

  const handleReset = () => {
    setStatus(null);
    setValidationError(null);
    setBusy(true);
    void onReset()
      .then((ok) => {
        if (ok) {
          setDraft(null);
          setStatus("Reverted to global GPU settings.");
        }
      })
      .finally(() => setBusy(false));
  };

  return (
    <aside className="panel quiet" aria-label="Queue GPU overrides (temporary)">
      <div className="panel-body">
        <h3 className="card-title">
          Queue GPU overrides (temporary)
          <HelpPopover
            title="Temporary GPU overrides"
            text="Override your GPU guard thresholds just for the current queue without editing global Settings. Useful when you want jobs to start while other apps are using the GPU."
            recommended="Increase max GPU utilization to 75–90% if you only have light desktop apps open."
          />
        </h3>
        <p className="section-hint">
          Applies only to jobs currently in the queue. Automatically reverts to global settings once
          the queue finishes.
        </p>
        <p className="person-meta" style={{ marginBottom: 12 }}>
          {activeOverride
            ? `Override active: ${activeOverride.gpuMaxUtilPercent}% max util · ${activeOverride.gpuIdleSeconds}s idle`
            : `Using global settings: ${defaultMaxUtil}% max util · ${defaultIdleSeconds}s idle`}
        </p>

        <form className="form" onSubmit={handleSubmit}>
          <label>
            <span className="field-label-row">
              Max GPU utilization (%)
              <HelpPopover
                title="Max GPU utilization (%)"
                text="Queued jobs wait if GPU usage stays above this percentage."
                recommended="40% when gaming or doing 3D work; 80%+ to prioritize processing."
              />
            </span>
            <input
              type="number"
              min={0}
              max={100}
              step={1}
              value={maxUtilValue}
              onChange={(e) =>
                setDraft({
                  maxUtil: e.target.value,
                  idleSec: idleSecValue,
                })
              }
            />
          </label>
          <label>
            <span className="field-label-row">
              Wait for idle (seconds)
              <HelpPopover
                title="Wait for idle (seconds)"
                text="How long the GPU must remain below the utilization limit before a job starts."
                recommended="30s for normal use; 5s for fast turnaround."
              />
            </span>
            <input
              type="number"
              min={0}
              max={3600}
              step={1}
              value={idleSecValue}
              onChange={(e) =>
                setDraft({
                  maxUtil: maxUtilValue,
                  idleSec: e.target.value,
                })
              }
            />
          </label>

          {validationError && (
            <p className="error" role="alert">
              {validationError}
            </p>
          )}
          {status && (
            <p className="person-meta" role="status">
              {status}
            </p>
          )}

          <div className="actions">
            <button
              type="button"
              className="btn btn-small btn-ghost"
              disabled={busy}
              onClick={handleReset}
            >
              Reset
            </button>
            <button type="submit" className="btn btn-small" disabled={busy}>
              Apply override
            </button>
          </div>
        </form>

        <p className="section-hint" style={{ marginTop: 14, marginBottom: 0 }}>
          While scheduled or processing jobs are active, your PC stays awake (monitors may still
          turn off).
        </p>
      </div>
    </aside>
  );
}

function JobCard(props: {
  job: JobView;
  onRetry: () => void;
  onCancel: () => void;
  onForceProcess: () => void;
  onSchedule: (unixMs: number | null) => Promise<boolean>;
  onOpenDraft?: () => void;
}) {
  const { job } = props;
  const pct = percent(job.progress);
  const eta = formatEta(job.etaSeconds);
  const failed = job.stages.some((s) => s.status === "failed");
  const canForce = job.state === "queued" || job.state === "waiting";
  const canSchedule = job.state === "queued" || job.state === "waiting";
  // `reduce` ran (it was not skipped): there is a draft to open.
  const hasDraft = job.stages.some((s) => s.stage === "reduce" && s.status === "done" && !s.note);

  const [scheduleDraft, setScheduleDraft] = useState<string | null>(null);
  const [scheduleError, setScheduleError] = useState<string | null>(null);

  const scheduleInput =
    scheduleDraft ?? (job.scheduledUnixMs != null ? toDateTimeLocalValue(job.scheduledUnixMs) : "");

  const handleScheduleClick = () => {
    setScheduleError(null);
    if (!scheduleInput) {
      setScheduleError("Choose a date and time first.");
      return;
    }
    const unixMs = new Date(scheduleInput).getTime();
    if (!Number.isFinite(unixMs)) {
      setScheduleError("Invalid schedule date or time.");
      return;
    }
    void props.onSchedule(unixMs).then((ok) => {
      if (ok) setScheduleDraft(null);
    });
  };

  const handleClearSchedule = () => {
    setScheduleError(null);
    void props.onSchedule(null).then((ok) => {
      if (ok) setScheduleDraft(null);
    });
  };

  return (
    <article className={`panel job job-${job.state}`} aria-label={job.title}>
      <div className="panel-body">
        <h3 className="card-title">
          {job.title}{" "}
          <span className={`badge badge-job-${job.state}`}>{jobStateLabel(job.state)}</span>
        </h3>
        <div className="progress-row">
          <div
            className="progress"
            role="progressbar"
            aria-label={`Progress of ${job.title}`}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={pct}
          >
            <span className="progress-fill" style={{ width: `${pct}%` }} />
          </div>
          <span className="progress-pct">{pct}%</span>
          {eta && (
            <span
              className="person-meta"
              aria-label={`ETA for ${job.title}`}
              style={{ whiteSpace: "nowrap" }}
            >
              {eta}
            </span>
          )}
        </div>
        {job.state === "waiting" && job.waitingReason && (
          <p className="waiting">Waiting: {job.waitingReason}</p>
        )}
        {job.scheduledUnixMs != null && (
          <p className="person-meta" style={{ marginBottom: 10 }}>
            Scheduled for {formatScheduledLabel(job.scheduledUnixMs)}
          </p>
        )}
        <ol className="stepper" aria-label={`Stages of ${job.title}`}>
          {job.stages.map((s) => (
            <Step key={s.stage} stage={s} />
          ))}
        </ol>

        {canSchedule && (
          <fieldset className="gate-new" style={{ marginBottom: 12 }}>
            <legend>Schedule start</legend>
            <label>
              Run at
              <input
                type="datetime-local"
                aria-label={`Schedule date and time for ${job.title}`}
                value={scheduleInput}
                onChange={(e) => setScheduleDraft(e.target.value)}
              />
            </label>
            <small>
              The PC stays awake (monitors may turn off) while scheduled or processing jobs run.
            </small>
            {scheduleError && (
              <p className="error" role="alert">
                {scheduleError}
              </p>
            )}
            <div className="actions">
              {job.scheduledUnixMs != null && (
                <button
                  type="button"
                  className="btn btn-small btn-ghost"
                  onClick={handleClearSchedule}
                >
                  Clear schedule
                </button>
              )}
              <button
                type="button"
                className="btn btn-small"
                disabled={!scheduleInput}
                onClick={handleScheduleClick}
              >
                Schedule
              </button>
            </div>
          </fieldset>
        )}

        <div className="card-actions">
          {hasDraft && props.onOpenDraft && (
            <button
              type="button"
              className="btn btn-small"
              aria-label={`Open the draft of ${job.title}`}
              onClick={props.onOpenDraft}
            >
              Open draft
            </button>
          )}
          {canForce && (
            <span className="field-label-row">
              <button
                type="button"
                className="btn btn-small btn-stop"
                onClick={props.onForceProcess}
              >
                Force process
              </button>
              <HelpPopover
                title="Force process"
                text="Ignores the GPU utilization limit and any scheduled delay once, starting this job immediately."
              />
            </span>
          )}
          {failed && (
            <button type="button" className="btn btn-small btn-stop" onClick={props.onRetry}>
              Retry
            </button>
          )}
          {job.state !== "running" && (
            <button type="button" className="btn btn-small btn-ghost" onClick={props.onCancel}>
              Cancel
            </button>
          )}
        </div>
      </div>
    </article>
  );
}

function Step({ stage }: { stage: StageView }) {
  const live = stage.status === "running";
  const skip = stageSkipText(stage);
  return (
    <li className={skip ? "step step-skipped" : `step step-${stage.status}`}>
      <span className="step-mark" aria-hidden="true" />
      <span className="step-name">{stageLabel(stage.stage)}</span>
      <span className="step-status">
        {skip ? "Skipped" : stageStatusLabel(stage.status)}
        {live && stage.fraction !== null && ` ${percent(stage.fraction)}%`}
        {stage.attempts > 1 && ` (attempt ${stage.attempts})`}
      </span>
      {live && stage.message && <span className="step-message">{stage.message}</span>}
      {skip && <span className="step-message">{skip}</span>}
      {stage.status === "failed" && stage.error && (
        <span className="step-error">{stage.error}</span>
      )}
    </li>
  );
}
