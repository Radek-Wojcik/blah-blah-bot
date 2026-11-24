import { useEffect, useMemo, useState } from "react";
import { useBackend } from "../backend-context";
import { plural, summaryLine } from "../briefing";
import { type StepId, stepForError } from "../first-run";
import { errorMessage, useCycles, useMeetings, useRecordingState } from "../hooks";
import type { Briefing } from "../ipc/types";
import { MODE_NAMES } from "../report-gate";
import { MeetingRowCard, useMeetingCycles } from "./ProjectDetail";
import { ProjectWizard } from "./ProjectWizard";
import { ReadinessBanner } from "./ReadinessBanner";

export interface ProjectsViewProps {
  onOpenProject: (cycleId: string) => void;
  onOpenMeeting: (meetingId: string) => void;
  onOpenSetup?: (step?: StepId, reason?: string | null) => void;
}

/**
 * Unified Projects home view: shows a grid of project cards with live task/topic summaries and
 * quick creation wizard, plus an unassigned meetings section when standalone recordings exist.
 */
export function ProjectsView({ onOpenProject, onOpenMeeting, onOpenSetup }: ProjectsViewProps) {
  const backend = useBackend();
  const { cycles, reload: reloadCycles } = useCycles();
  const { meetings, reload: reloadMeetings } = useMeetings();
  const recording = useRecordingState();

  const [briefings, setBriefings] = useState<Record<string, Briefing>>({});
  const [wizardOpen, setWizardOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const { cycleMap, resolved } = useMeetingCycles(meetings);

  useEffect(() => {
    if (!cycles || cycles.length === 0) return;
    let cancelled = false;
    void Promise.all(
      cycles.map(async (c): Promise<[string, Briefing] | null> => {
        try {
          const b = await backend.getBriefing(c.id, false);
          return [c.id, b];
        } catch {
          return null;
        }
      }),
    ).then((entries) => {
      if (cancelled) return;
      const next: Record<string, Briefing> = {};
      for (const entry of entries) {
        if (entry) next[entry[0]] = entry[1];
      }
      setBriefings(next);
    });
    return () => {
      cancelled = true;
    };
  }, [backend, cycles]);

  const standaloneMeetings = useMemo(
    () => (resolved ? meetings.filter((m) => cycleMap[m.id] === null) : []),
    [meetings, cycleMap, resolved],
  );

  const actOnMeeting = (p: Promise<unknown>) => {
    setError(null);
    void p.then(reloadMeetings).catch((e) => setError(errorMessage(e)));
  };

  const processMeeting = (id: string) => {
    setError(null);
    void backend
      .processMeeting(id)
      .then(reloadMeetings)
      .catch((e) => {
        const step = stepForError(e);
        if (step && onOpenSetup) onOpenSetup(step, errorMessage(e));
        else setError(errorMessage(e));
      });
  };

  return (
    <>
      <div className="page-head">
        <h2>
          <span className="glitch" data-text="Projects">
            Projects
          </span>
        </h2>
        <span className="head-actions">
          <span className="chip">{plural(cycles?.length ?? 0, "project", "projects")}</span>
          <button type="button" className="btn btn-record" onClick={() => setWizardOpen(true)}>
            New Project
          </button>
        </span>
      </div>

      {onOpenSetup && <ReadinessBanner onOpenSetup={(step) => onOpenSetup(step)} />}

      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}

      {cycles === null ? (
        <p className="empty">Loading projects…</p>
      ) : cycles.length === 0 ? (
        <p className="empty">
          No projects yet. Click &ldquo;New Project&rdquo; to set up your first meeting workspace.
        </p>
      ) : (
        <ul className="cards" aria-label="Projects">
          {cycles.map((c) => {
            const b = briefings[c.id];
            const summaryText = b
              ? summaryLine(b)
              : c.defaultMode
                ? `Default layout: ${MODE_NAMES[c.defaultMode]}`
                : "Project configuration needs attention";
            const updatedText = c.stateAsOf ? `Updated ${c.stateAsOf}` : "Local project";
            return (
              <li key={c.id}>
                <article className="panel" aria-label={`Project ${c.name}`}>
                  <div className="panel-body">
                    <h3 className="card-title">
                      {c.name} <span className="cycle-id">{c.id}</span>
                      {c.offline && <span className="badge badge-job-failed">Offline</span>}
                      {!c.valid && <span className="badge badge-job-failed">Needs fixing</span>}
                    </h3>
                    <p className="card-when">
                      {plural(c.participantsCount, "participant", "participants")} · {updatedText}
                    </p>
                    <p className="summary-text">{summaryText}</p>
                    {!c.valid && c.issues[0] && (
                      <p className="error">
                        {c.issues[0].path}: {c.issues[0].message}
                      </p>
                    )}
                    <div className="card-actions">
                      <button
                        type="button"
                        className="btn btn-small btn-record"
                        onClick={() => onOpenProject(c.id)}
                      >
                        Open project
                      </button>
                    </div>
                  </div>
                </article>
              </li>
            );
          })}
        </ul>
      )}

      {standaloneMeetings.length > 0 && (
        <section aria-label="Unassigned / standalone meetings" style={{ marginTop: "var(--gap)" }}>
          <div className="page-head">
            <h3 className="sub-head">Unassigned / standalone meetings</h3>
            <span className="chip">
              {plural(standaloneMeetings.length, "recording", "recordings")}
            </span>
          </div>
          <ul className="cycle-rows" aria-label="Standalone meetings">
            {standaloneMeetings.map((m) => (
              <MeetingRowCard
                key={m.id}
                meeting={m}
                recordingMeetingId={recording.meetingId}
                onOpen={onOpenMeeting}
                onProcess={processMeeting}
                onRecover={(id) => actOnMeeting(backend.recoverMeeting(id))}
                onDelete={(id) => actOnMeeting(backend.deleteMeeting(id))}
              />
            ))}
          </ul>
        </section>
      )}

      {wizardOpen && (
        <ProjectWizard
          onClose={() => setWizardOpen(false)}
          onCreated={(cycleId) => {
            setWizardOpen(false);
            reloadCycles();
            onOpenProject(cycleId);
          }}
        />
      )}
    </>
  );
}
