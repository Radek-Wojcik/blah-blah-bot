import { useEffect, useState } from "react";
import { useBackend } from "../backend-context";
import { BackButton } from "../components/BackButton";
import { formatDateTimeUtc, formatDuration } from "../format";
import { isoDateUtc, noteFileName, useSaveNote } from "../note-export";
import type { MeetingDetails } from "../ipc/types";
import { jobStateLabel } from "../processing";
import { DraftEditor } from "./DraftEditor";
import { isSafeHttpUrl } from "./ProjectDetail";
import { SpeakersView } from "./Speakers";
import { TranscriptView } from "./Transcript";

export type MeetingDetailTab = "draft" | "transcript" | "speakers";

export interface MeetingDetailViewProps {
  meetingId: string;
  initialTab?: MeetingDetailTab;
  highlightMs?: number | null;
  onTabChange?: (tab: MeetingDetailTab) => void;
  onBack: () => void;
  onOpenQueue: () => void;
}

/**
 * Tabbed meeting workspace combining Draft & Report, Transcript, and Speakers views, plus a
 * one-click local Markdown (.md) export button in the header.
 */
export function MeetingDetailView({
  meetingId,
  initialTab = "draft",
  highlightMs = null,
  onTabChange,
  onBack,
  onOpenQueue,
}: MeetingDetailViewProps) {
  const backend = useBackend();
  const [tab, setTab] = useState<MeetingDetailTab>(initialTab);
  const [prevInitialTab, setPrevInitialTab] = useState<MeetingDetailTab>(initialTab);
  const [jumpMs, setJumpMs] = useState<number | null>(highlightMs);
  const [details, setDetails] = useState<MeetingDetails | null>(null);
  const note = useSaveNote(meetingId);

  if (prevInitialTab !== initialTab) {
    setPrevInitialTab(initialTab);
    setTab(initialTab);
  }

  const selectTab = (next: MeetingDetailTab) => {
    setTab(next);
    onTabChange?.(next);
  };

  useEffect(() => {
    let cancelled = false;
    void backend
      .getMeeting(meetingId)
      .then((d) => {
        if (!cancelled) setDetails(d);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [backend, meetingId]);

  const summary = details?.summary;
  const heading = summary?.shortTitle?.trim() || summary?.title || meetingId;
  const defaultNoteName = noteFileName(
    summary?.shortTitle?.trim() || summary?.title,
    summary ? isoDateUtc(summary.startedUnixMs) : "",
  );
  const safeReportUrl = isSafeHttpUrl(summary?.reportUrl) ? summary.reportUrl : null;

  return (
    <>
      <div className="page-head">
        <div className="page-head-start">
          <BackButton label="Back" onClick={onBack} />
          <div>
            <h2>
              <span className="glitch" data-text={heading}>
                {heading}
              </span>
            </h2>
            {summary && (
              <p className="card-when" style={{ margin: "4px 0 0" }}>
                {formatDateTimeUtc(summary.startedUnixMs)} UTC
                {summary.durationMs !== null && ` · ${formatDuration(summary.durationMs)}`}
                {summary.shortTitle &&
                  summary.shortTitle !== summary.title &&
                  ` · ${summary.title}`}
                {summary.job && (
                  <>
                    {" "}
                    <span className={`badge badge-job-${summary.job}`}>
                      {jobStateLabel(summary.job)}
                    </span>
                  </>
                )}
              </p>
            )}
          </div>
        </div>

        <span className="head-actions">
          {safeReportUrl && (
            <a
              className="btn btn-small"
              href={safeReportUrl}
              target="_blank"
              rel="noopener noreferrer"
            >
              View on GitHub
            </a>
          )}
          <button
            type="button"
            className="btn btn-small btn-record"
            disabled={note.saving}
            onClick={() => void note.save(defaultNoteName)}
          >
            {note.saving ? "Saving…" : "Download note (.md)"}
          </button>
        </span>
      </div>

      {note.savedPath && (
        <p className="valid-note" role="status">
          Saved note to {note.savedPath}
        </p>
      )}
      {note.error && (
        <p className="error" role="alert">
          {note.error}
        </p>
      )}

      <div className="pane-toggle" role="tablist" aria-label="Meeting sections">
        <button
          type="button"
          role="tab"
          aria-selected={tab === "draft"}
          className={tab === "draft" ? "btn btn-small btn-stop" : "btn btn-small btn-ghost"}
          onClick={() => selectTab("draft")}
        >
          Draft &amp; Report
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === "transcript"}
          className={tab === "transcript" ? "btn btn-small btn-stop" : "btn btn-small btn-ghost"}
          onClick={() => selectTab("transcript")}
        >
          Transcript
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === "speakers"}
          className={tab === "speakers" ? "btn btn-small btn-stop" : "btn btn-small btn-ghost"}
          onClick={() => selectTab("speakers")}
        >
          Speakers
        </button>
      </div>

      {tab === "draft" && (
        <DraftEditor
          meetingId={meetingId}
          onBack={onBack}
          onOpenTranscript={(atMs) => {
            setJumpMs(atMs);
            selectTab("transcript");
          }}
          onOpenQueue={onOpenQueue}
        />
      )}
      {tab === "transcript" && (
        <TranscriptView
          meetingId={meetingId}
          highlightMs={jumpMs}
          onBack={onBack}
          onOpenSpeakers={() => selectTab("speakers")}
        />
      )}
      {tab === "speakers" && (
        <SpeakersView
          meetingId={meetingId}
          onBack={onBack}
          onOpenTranscript={() => selectTab("transcript")}
        />
      )}
    </>
  );
}
