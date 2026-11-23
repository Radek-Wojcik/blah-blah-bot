import { useEffect, useState } from "react";
import { useBackend } from "../backend-context";
import { repoState } from "../cycles";
import { formatDateTimeUtc, formatDuration } from "../format";
import { errorMessage, useCycles, useMeetings, useRecordingState } from "../hooks";
import type { MeetingSummary, Settings } from "../ipc/types";
import { canProcess, jobStateLabel } from "../processing";
import { BrowserExtension } from "./BrowserExtension";
import { DetectionSettingsView } from "./DetectionSettings";
import { CycleEditor } from "./CycleEditor";
import { CyclesView } from "./Cycles";
import { DraftEditor } from "./DraftEditor";
import { ProcessPrompt } from "./ProcessPrompt";
import { ProcessingSettingsView } from "./ProcessingSettings";
import { QueueView } from "./Queue";
import { SpeakersView } from "./Speakers";
import { TranscriptView } from "./Transcript";

type Page =
  "meetings" | "queue" | "settings" | "transcript" | "speakers" | "draft" | "cycles" | "cycle";

interface Route {
  page: Page;
  meetingId: string | null;
  cycleId?: string | null;
  /** A time in the meeting the transcript is opened at. */
  atMs?: number | null;
}

/**
 * `?page=queue`, `?page=transcript&meeting=m5&at=19000`, `?page=speakers&meeting=m5`,
 * `?page=draft&meeting=m5`, `?page=cycles` and `?page=cycle&cycle=NOMAD` open a page directly
 * (used by the screenshot script and the tests).
 */
function initialRoute(): Route {
  const params = new URLSearchParams(window.location.search);
  const meetingId = params.get("meeting");
  const page = params.get("page");
  const at = params.get("at");
  if ((page === "transcript" || page === "speakers" || page === "draft") && meetingId)
    return { page, meetingId, atMs: at === null ? null : Number(at) };
  if (page === "cycle" && params.get("cycle"))
    return { page, meetingId: null, cycleId: params.get("cycle") };
  if (page === "queue" || page === "settings" || page === "cycles")
    return { page, meetingId: null };
  return { page: "meetings", meetingId: null };
}

export function Main() {
  const [route, setRoute] = useState(initialRoute);
  const page = route.page;
  const setPage = (p: Page) => setRoute({ page: p, meetingId: null });
  const inMeetings =
    page === "meetings" || page === "transcript" || page === "speakers" || page === "draft";
  const recording = useRecordingState();
  return (
    <div className="main">
      <nav className="panel sidebar" aria-label="Main navigation">
        <div className="panel-body">
          <h1 className="brand glitch" data-text="BlahBlahBot">
            Blah<span>Blah</span>Bot
          </h1>
          <button className={inMeetings ? "nav nav-on" : "nav"} onClick={() => setPage("meetings")}>
            Meetings
          </button>
          <button
            className={page === "cycles" || page === "cycle" ? "nav nav-on" : "nav"}
            onClick={() => setPage("cycles")}
          >
            Cycles
          </button>
          <button
            className={page === "queue" ? "nav nav-on" : "nav"}
            onClick={() => setPage("queue")}
          >
            Queue
          </button>
          <button
            className={page === "settings" ? "nav nav-on" : "nav"}
            onClick={() => setPage("settings")}
          >
            Settings
          </button>
          <div className="ticker" aria-hidden="true">
            <span>
              SYS::ONLINE // LOCAL ONLY // NO TELEMETRY // WASAPI LOOPBACK READY // SEGMENT 60S //
              JOURNAL FLUSHED // SYS::ONLINE // LOCAL ONLY // NO TELEMETRY // WASAPI LOOPBACK READY
              // SEGMENT 60S // JOURNAL FLUSHED //
            </span>
          </div>
          {recording.status === "recording" && (
            <p className="side-live">
              <span className="dot dot-live" aria-hidden="true" /> REC{" "}
              {formatDuration(recording.elapsedMs)}
            </p>
          )}
        </div>
      </nav>
      <section className="content">
        <div className="content-inner">
          {page === "meetings" && (
            <MeetingList
              recordingMeetingId={recording.meetingId}
              onOpenTranscript={(id) => setRoute({ page: "transcript", meetingId: id })}
              onOpenSpeakers={(id) => setRoute({ page: "speakers", meetingId: id })}
              onOpenDraft={(id) => setRoute({ page: "draft", meetingId: id })}
            />
          )}
          {page === "queue" && (
            <QueueView onOpenDraft={(id) => setRoute({ page: "draft", meetingId: id })} />
          )}
          {page === "cycles" && (
            <CyclesView
              onOpenCycle={(id) => setRoute({ page: "cycle", meetingId: null, cycleId: id })}
            />
          )}
          {page === "cycle" && route.cycleId && (
            <CycleEditor cycleId={route.cycleId} onBack={() => setPage("cycles")} />
          )}
          {page === "draft" && route.meetingId && (
            <DraftEditor
              meetingId={route.meetingId}
              onBack={() => setPage("meetings")}
              onOpenTranscript={(atMs) =>
                setRoute({ page: "transcript", meetingId: route.meetingId, atMs })
              }
              onOpenQueue={() => setPage("queue")}
            />
          )}
          {page === "transcript" && route.meetingId && (
            <TranscriptView
              meetingId={route.meetingId}
              highlightMs={route.atMs}
              onBack={() => setPage("meetings")}
              onOpenSpeakers={() => setRoute({ page: "speakers", meetingId: route.meetingId })}
            />
          )}
          {page === "speakers" && route.meetingId && (
            <SpeakersView
              meetingId={route.meetingId}
              onBack={() => setPage("meetings")}
              onOpenTranscript={() => setRoute({ page: "transcript", meetingId: route.meetingId })}
            />
          )}
          {page === "settings" && <SettingsView />}
        </div>
      </section>
      <ProcessPrompt />
    </div>
  );
}

function JobChip({ meeting }: { meeting: MeetingSummary }) {
  if (!meeting.job) return null;
  return <span className={`badge badge-job-${meeting.job}`}>{jobStateLabel(meeting.job)}</span>;
}

function StateBadge({ meeting }: { meeting: MeetingSummary }) {
  if (meeting.state === "stopped") return null;
  const text = {
    recording: "Recording",
    recoverable: "Needs recovery",
    recovered: "Recovered",
    stopped: "",
  }[meeting.state];
  return <span className={`badge badge-${meeting.state}`}>{text}</span>;
}

function MeetingList(props: {
  recordingMeetingId: string | null;
  onOpenTranscript: (id: string) => void;
  onOpenSpeakers: (id: string) => void;
  onOpenDraft: (id: string) => void;
}) {
  const backend = useBackend();
  const { meetings, reload } = useMeetings();
  const [error, setError] = useState<string | null>(null);

  const act = (p: Promise<unknown>) => {
    setError(null);
    p.then(reload).catch((e) => setError(errorMessage(e)));
  };

  return (
    <>
      <div className="page-head">
        <h2>
          <span className="glitch" data-text="Meetings">
            Meetings
          </span>
        </h2>
        <span className="chip">{meetings.length} stored</span>
      </div>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {meetings.length === 0 ? (
        <p className="empty">No recordings yet. Press Record in the tray widget.</p>
      ) : (
        <ul className="cards">
          {meetings.map((m) => (
            <li key={m.id}>
              <article className="panel">
                <div className="panel-body">
                  <h3 className="card-title">
                    {m.title} <StateBadge meeting={m} /> <JobChip meeting={m} />
                  </h3>
                  <p className="card-when">{formatDateTimeUtc(m.startedUnixMs)} UTC</p>
                  <dl className="stats">
                    <div>
                      <dt>Length</dt>
                      <dd>{m.durationMs === null ? "–" : formatDuration(m.durationMs)}</dd>
                    </div>
                    <div>
                      <dt>Tracks</dt>
                      <dd>{m.trackCount}</dd>
                    </div>
                    <div>
                      <dt>Shots</dt>
                      <dd>{m.screenshotCount}</dd>
                    </div>
                  </dl>
                  <div className="card-actions">
                    {m.state === "recoverable" && (
                      <button
                        className="btn btn-small btn-stop"
                        onClick={() => act(backend.recoverMeeting(m.id))}
                      >
                        Recover
                      </button>
                    )}
                    {m.job === "done" && (
                      <>
                        <button
                          className="btn btn-small"
                          aria-label={`Name speakers of ${m.title}`}
                          onClick={() => props.onOpenSpeakers(m.id)}
                        >
                          Name speakers
                        </button>
                        <button
                          className="btn btn-small"
                          onClick={() => props.onOpenTranscript(m.id)}
                        >
                          Transcript
                        </button>
                        <button
                          className="btn btn-small"
                          aria-label={`Open the draft of ${m.title}`}
                          onClick={() => props.onOpenDraft(m.id)}
                        >
                          Draft
                        </button>
                      </>
                    )}
                    <button
                      className="btn btn-small btn-stop"
                      disabled={!canProcess(m, props.recordingMeetingId)}
                      onClick={() => act(backend.processMeeting(m.id))}
                    >
                      Process
                    </button>
                    <button
                      className="btn btn-small btn-ghost"
                      onClick={() => act(backend.deleteMeeting(m.id))}
                    >
                      Delete
                    </button>
                  </div>
                </div>
              </article>
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

function SettingsView() {
  const backend = useBackend();
  const [settings, setSettings] = useState<Settings | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  useEffect(() => {
    void backend.getSettings().then(setSettings);
  }, [backend]);

  if (!settings) return null;
  const set = (patch: Partial<Settings>) => setSettings({ ...settings, ...patch });
  const bridge = settings.bridge;
  const setBridge = (patch: Partial<NonNullable<Settings["bridge"]>>) =>
    bridge && set({ bridge: { ...bridge, ...patch } });

  return (
    <>
      <div className="page-head">
        <h2>
          <span className="glitch" data-text="Settings">
            Settings
          </span>
        </h2>
      </div>
      <div className="panel quiet" style={{ maxWidth: 620 }}>
        <div className="panel-body">
          <form
            className="form"
            onSubmit={(e) => {
              e.preventDefault();
              setMessage(null);
              backend
                .saveSettings(settings)
                .then(() => setMessage("Saved"))
                .catch((err) => setMessage(errorMessage(err)));
            }}
          >
            <label>
              Recordings folder
              <input
                value={settings.recordingsDir}
                onChange={(e) => set({ recordingsDir: e.target.value })}
              />
              <small>Leave empty to use the default folder in Documents.</small>
            </label>
            <label>
              Record / Stop hotkey
              <input
                value={settings.recordHotkey}
                onChange={(e) => set({ recordHotkey: e.target.value })}
              />
            </label>
            <label>
              Region screenshot hotkey
              <input
                value={settings.screenshotHotkey}
                onChange={(e) => set({ screenshotHotkey: e.target.value })}
              />
            </label>
            {bridge && (
              <fieldset>
                <legend>Browser extension</legend>
                <label className="check">
                  <input
                    type="checkbox"
                    checked={bridge.enabled}
                    onChange={(e) => setBridge({ enabled: e.target.checked })}
                  />
                  Accept the Chrome extension on this computer
                </label>
                <label>
                  Port
                  <input
                    type="number"
                    min={1024}
                    max={65535}
                    value={bridge.port}
                    onChange={(e) => setBridge({ port: Number(e.target.value) })}
                  />
                  <small>If it is taken, the next free port up to {bridge.port + 9} is used.</small>
                </label>
                <label className="check">
                  <input
                    type="checkbox"
                    checked={bridge.autoAddTabs}
                    onChange={(e) => setBridge({ autoAddTabs: e.target.checked })}
                  />
                  Record a tab as soon as the extension sends it during a recording
                </label>
              </fieldset>
            )}
            <div className="actions">
              {message && <span role="status">{message}</span>}
              <button className="btn btn-record" type="submit">
                Save
              </button>
            </div>
          </form>
        </div>
      </div>
      <DetectionSettingsView />
      <BrowserExtension />
      <CycleRepos />
      <ProcessingSettingsView />
    </>
  );
}

/** The registered cycle repositories (managed on the Cycles page). */
function CycleRepos() {
  const { repos } = useCycles();
  return (
    <section aria-labelledby="repos-heading">
      <h3 id="repos-heading" className="sub-head">
        Cycle repositories
      </h3>
      {repos.length === 0 ? (
        <p className="empty">No repository yet. Add or create one on the Cycles page.</p>
      ) : (
        <ul className="people" aria-label="Cycle repositories">
          {repos.map((r) => (
            <li key={r.id} className="person">
              <span className="person-main">
                <span>{r.id}</span>
                <span className="person-meta">{r.remote ?? r.path}</span>
              </span>
              <span className="person-meta">
                {repoState(r)}
                {r.offline && " · offline"}
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
