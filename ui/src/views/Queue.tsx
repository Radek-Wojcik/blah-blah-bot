import { useState } from "react";
import { useBackend } from "../backend-context";
import { errorMessage, useQueue } from "../hooks";
import type { JobView, StageView } from "../ipc/types";
import { jobStateLabel, percent, stageLabel, stageSkipText, stageStatusLabel } from "../processing";

/** The processing queue: one card per job with its stages, live progress and actions. */
export function QueueView(props: { onOpenDraft?: (meetingId: string) => void }) {
  const backend = useBackend();
  const queue = useQueue();
  const [error, setError] = useState<string | null>(null);

  const act = (p: Promise<unknown>) => {
    setError(null);
    p.catch((e) => setError(errorMessage(e)));
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
      {queue && queue.jobs.length === 0 ? (
        <p className="empty">Nothing is queued. Process a meeting from the Meetings page.</p>
      ) : (
        <ul className="jobs" aria-label="Processing jobs">
          {(queue?.jobs ?? []).map((job) => (
            <li key={job.meetingId}>
              <JobCard
                job={job}
                onOpenDraft={props.onOpenDraft && (() => props.onOpenDraft?.(job.meetingId))}
                onRetry={() => act(backend.retryJob(job.meetingId))}
                onCancel={() => act(backend.cancelJob(job.meetingId))}
              />
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

function JobCard(props: {
  job: JobView;
  onRetry: () => void;
  onCancel: () => void;
  onOpenDraft?: () => void;
}) {
  const { job } = props;
  const pct = percent(job.progress);
  const failed = job.stages.some((s) => s.status === "failed");
  // `reduce` ran (it was not skipped): there is a draft to open.
  const hasDraft = job.stages.some((s) => s.stage === "reduce" && s.status === "done" && !s.note);
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
        </div>
        {job.state === "waiting" && job.waitingReason && (
          <p className="waiting">Waiting: {job.waitingReason}</p>
        )}
        <ol className="stepper" aria-label={`Stages of ${job.title}`}>
          {job.stages.map((s) => (
            <Step key={s.stage} stage={s} />
          ))}
        </ol>
        <div className="card-actions">
          {hasDraft && props.onOpenDraft && (
            <button
              className="btn btn-small"
              aria-label={`Open the draft of ${job.title}`}
              onClick={props.onOpenDraft}
            >
              Open draft
            </button>
          )}
          {failed && (
            <button className="btn btn-small btn-stop" onClick={props.onRetry}>
              Retry
            </button>
          )}
          {job.state !== "running" && (
            <button className="btn btn-small btn-ghost" onClick={props.onCancel}>
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
