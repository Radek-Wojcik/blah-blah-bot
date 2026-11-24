import { useEffect, useState } from "react";
import { useBackend } from "../backend-context";
import { HelpPopover } from "../components/HelpPopover";
import { errorMessage } from "../hooks";
import type { LanguageSetting, MeetingDetails, ReportDefaults, ReportMode } from "../ipc/types";
import { MODE_NAMES, REPORT_MODES } from "../report-gate";
import { Modal } from "./Modal";

const VIDEO_EXT_RE = /\.(mkv|mp4|webm|mov|avi)$/i;

/** Return true when the file path points to a supported video container. */
export function isVideoFilePath(filePath: string): boolean {
  return VIDEO_EXT_RE.test(filePath.trim());
}

export interface ImportMeetingModalProps {
  cycleId: string;
  onClose: () => void;
  onImported: (details: MeetingDetails) => void;
}

/** Modal dialog for importing an external audio or video recording into a project. */
export function ImportMeetingModal({ cycleId, onClose, onImported }: ImportMeetingModalProps) {
  const backend = useBackend();
  const [filePath, setFilePath] = useState("");
  const [mediaMode, setMediaMode] = useState<"audio_only" | "audio_and_slides">("audio_and_slides");
  const [reportMode, setReportMode] = useState<ReportMode>("auto");
  const [meetingType, setMeetingType] = useState("");
  const [outputLanguage, setOutputLanguage] = useState<LanguageSetting | "">("");
  const [processNow, setProcessNow] = useState(true);
  const [defaults, setDefaults] = useState<ReportDefaults | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void backend
      .reportDefaults(cycleId)
      .then((d) => {
        if (cancelled) return;
        setDefaults(d);
        setReportMode(d.defaultMode);
        if (d.defaultMeetingType) setMeetingType(d.defaultMeetingType);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [backend, cycleId]);

  const isVideo = isVideoFilePath(filePath);

  const handleFilePick = (files: FileList | null) => {
    const file = files?.[0] as (File & { path?: string }) | undefined;
    if (!file) return;
    setFilePath(file.path && file.path.trim() !== "" ? file.path : file.name);
    setError(null);
  };

  const submit = async () => {
    const trimmed = filePath.trim();
    if (trimmed === "") {
      setError("Choose or paste the path to an audio or video file.");
      return;
    }
    setError(null);
    setSubmitting(true);
    try {
      const details = await backend.importMeeting({
        cycleId,
        filePath: trimmed,
        mediaMode: isVideo ? mediaMode : "audio_only",
        reportMode,
        meetingType: reportMode === "by-type" && meetingType !== "" ? meetingType : null,
        outputLanguage: outputLanguage === "" ? null : outputLanguage,
        processNow,
      });
      onImported(details);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Modal title="Add Meeting from Recording" onClose={onClose} wide>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}

      <form
        className="form"
        aria-label="Import meeting"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <label>
          Audio or video file path
          <input
            aria-label="Audio or video file path"
            value={filePath}
            placeholder="C:\Recordings\sprint-review.mp4"
            autoFocus
            onChange={(e) => setFilePath(e.target.value)}
          />
          <small>
            Supports audio (mp3, wav, m4a, ogg, flac, aac) and video (mkv, mp4, webm, mov, avi).
          </small>
        </label>

        <label>
          Browse file
          <input
            type="file"
            aria-label="Browse audio or video file"
            accept=".mp3,.wav,.m4a,.ogg,.flac,.aac,.mkv,.mp4,.webm,.mov,.avi,audio/*,video/*"
            onChange={(e) => handleFilePick(e.target.files)}
          />
        </label>

        {isVideo && (
          <div className="field-group">
            <span className="field-label-row">
              <span className="field-label">Video handling</span>
              <HelpPopover
                title="Video slide detection"
                text="When a video includes screen sharing or slides, Audio + Visual Slides extracts key frame changes without capturing cursor jitter."
                recommended="Audio + Visual Slides for screen shares"
              />
            </span>
            <div className="import-mode-cards" role="radiogroup" aria-label="Video handling mode">
              <button
                type="button"
                role="radio"
                aria-checked={mediaMode === "audio_only"}
                className={
                  mediaMode === "audio_only"
                    ? "import-mode-card import-mode-card-active"
                    : "import-mode-card"
                }
                onClick={() => setMediaMode("audio_only")}
              >
                <strong>Audio only</strong>
                <span className="person-meta">
                  Extracts the audio track for faster transcription when screen visuals don&apos;t
                  matter.
                </span>
              </button>

              <button
                type="button"
                role="radio"
                aria-checked={mediaMode === "audio_and_slides"}
                className={
                  mediaMode === "audio_and_slides"
                    ? "import-mode-card import-mode-card-active"
                    : "import-mode-card"
                }
                onClick={() => setMediaMode("audio_and_slides")}
              >
                <strong>Audio + Visual Slides</strong>
                <span className="person-meta">
                  Automatically captures key slide/screen changes (ignoring cursor jitter) and
                  summarizes them into the note.
                </span>
              </button>
            </div>
          </div>
        )}

        <div className="form-row">
          <label>
            Report mode
            <select
              aria-label="Report mode"
              value={reportMode}
              onChange={(e) => setReportMode(e.target.value as ReportMode)}
            >
              {REPORT_MODES.map((m) => (
                <option key={m} value={m}>
                  {MODE_NAMES[m]}
                </option>
              ))}
            </select>
          </label>

          <label>
            Output language
            <select
              aria-label="Output language"
              value={outputLanguage}
              onChange={(e) => setOutputLanguage(e.target.value as LanguageSetting | "")}
            >
              <option value="">Project default</option>
              <option value="auto">Auto</option>
              <option value="en">English</option>
              <option value="pl">Polish</option>
            </select>
          </label>
        </div>

        {reportMode === "by-type" && defaults && defaults.meetingTypes.length > 0 && (
          <label>
            Meeting type
            <select
              aria-label="Meeting type"
              value={meetingType}
              onChange={(e) => setMeetingType(e.target.value)}
            >
              <option value="">Choose…</option>
              {defaults.meetingTypes.map((t) => (
                <option key={t.name} value={t.name}>
                  {t.name}
                </option>
              ))}
            </select>
          </label>
        )}

        <label className="check">
          <input
            type="checkbox"
            checked={processNow}
            onChange={(e) => setProcessNow(e.target.checked)}
          />
          Process immediately
        </label>

        <div className="actions">
          <button className="btn btn-ghost" type="button" disabled={submitting} onClick={onClose}>
            Cancel
          </button>
          <button
            className="btn btn-record"
            type="submit"
            disabled={submitting || filePath.trim() === ""}
          >
            {submitting ? "Importing…" : "Import meeting"}
          </button>
        </div>
      </form>
    </Modal>
  );
}
