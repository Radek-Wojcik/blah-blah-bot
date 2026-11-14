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
      <nav className="sidebar" aria-label="Main navigation">
        <h1 className="brand">Blah Blah Bot</h1>
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
        {recording.status === "recording" && (
          <p className="side-live">
            <span className="dot dot-live" aria-hidden="true" /> Recording{" "}
            {formatDuration(recording.elapsedMs)}
          </p>
        )}
      </nav>
      <section className="content">
        {page === "meetings" ? <MeetingList /> : <SettingsView />}
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
      <h2>Meetings</h2>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {meetings.length === 0 ? (
        <p className="empty">No recordings yet. Press Record in the tray widget.</p>
      ) : (
        <table className="meetings">
          <thead>
            <tr>
              <th>Meeting</th>
              <th>Started (UTC)</th>
              <th>Length</th>
              <th>Tracks</th>
              <th>Shots</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {meetings.map((m) => (
              <tr key={m.id}>
                <td>
                  {m.title} <StateBadge meeting={m} />
                </td>
                <td>{formatDateTimeUtc(m.startedUnixMs)}</td>
                <td>{m.durationMs === null ? "–" : formatDuration(m.durationMs)}</td>
                <td>{m.trackCount}</td>
                <td>{m.screenshotCount}</td>
                <td className="row-actions">
                  {m.state === "recoverable" && (
                    <button
                      className="btn btn-small"
                      onClick={() => act(backend.recoverMeeting(m.id))}
                    >
                      Recover
                    </button>
                  )}
                  <button
                    className="btn btn-small"
                    onClick={() => act(backend.deleteMeeting(m.id))}
                  >
                    Delete
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
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
      <h2>Settings</h2>
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
          <button className="btn btn-record" type="submit">
            Save
          </button>
          {message && <span role="status">{message}</span>}
        </div>
      </form>
    </>
  );
}
