import { useEffect, useState } from "react";
import { useBackend } from "../backend-context";
import { formatDateTimeUtc, formatDuration } from "../format";
import { errorMessage, useMeetings, useRecordingState } from "../hooks";
import type { MeetingSummary, Settings } from "../ipc/types";

type Page = "meetings" | "settings";

export function Main() {
  const [page, setPage] = useState<Page>("meetings");
  const recording = useRecordingState();
  return (
    <div className="main">
      <nav className="panel sidebar" aria-label="Main navigation">
        <div className="panel-body">
          <h1 className="brand glitch" data-text="BlahBlahBot">
            Blah<span>Blah</span>Bot
          </h1>
          <button
            className={page === "meetings" ? "nav nav-on" : "nav"}
            onClick={() => setPage("meetings")}
          >
            Meetings
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
          {page === "meetings" ? <MeetingList /> : <SettingsView />}
        </div>
      </section>
    </div>
  );
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

function MeetingList() {
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
                    {m.title} <StateBadge meeting={m} />
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
            <div className="actions">
              {message && <span role="status">{message}</span>}
              <button className="btn btn-record" type="submit">
                Save
              </button>
            </div>
          </form>
        </div>
      </div>
    </>
  );
}
