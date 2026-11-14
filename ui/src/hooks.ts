import { useCallback, useEffect, useState } from "react";
import { useBackend } from "./backend-context";
import { isIpcError } from "./ipc/tauri";
import type { MeetingSummary, RecordingState, SourceInfo } from "./ipc/types";

const idle: RecordingState = {
  status: "idle",
  meetingId: null,
  elapsedMs: 0,
  sources: [],
  screenshotCount: 0,
};

export function errorMessage(e: unknown): string {
  if (isIpcError(e)) return e.message;
  if (e instanceof Error) return e.message;
  return String(e);
}

/** Live recording state: loaded once, then replaced by every `recording-state` event. */
export function useRecordingState(): RecordingState {
  const backend = useBackend();
  const [state, setState] = useState<RecordingState>(idle);
  useEffect(() => {
    let off: (() => void) | undefined;
    let cancelled = false;
    void backend.recordingState().then((s) => !cancelled && setState(s));
    void backend.onRecordingState(setState).then((u) => {
      if (cancelled) u();
      else off = u;
    });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [backend]);
  return state;
}

export function useMeetings(): { meetings: MeetingSummary[]; reload: () => void } {
  const backend = useBackend();
  const [meetings, setMeetings] = useState<MeetingSummary[]>([]);
  const reload = useCallback(() => {
    void backend.listMeetings().then(setMeetings);
  }, [backend]);
  useEffect(() => {
    reload();
    let off: (() => void) | undefined;
    let cancelled = false;
    void backend.onMeetingsChanged(reload).then((u) => {
      if (cancelled) u();
      else off = u;
    });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [backend, reload]);
  return { meetings, reload };
}

export function useSources(refreshKey: unknown): SourceInfo[] {
  const backend = useBackend();
  const [sources, setSources] = useState<SourceInfo[]>([]);
  useEffect(() => {
    let cancelled = false;
    void backend.listSources().then((s) => !cancelled && setSources(s));
    return () => {
      cancelled = true;
    };
  }, [backend, refreshKey]);
  return sources;
}
