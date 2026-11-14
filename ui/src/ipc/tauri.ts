import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Backend } from "./backend";
import type { IpcError } from "./types";

/** Command names are snake_case and mirror the `#[tauri::command]` functions in `crates/app`. */
export function createTauriBackend(): Backend {
  return {
    listMeetings: () => invoke("list_meetings"),
    getMeeting: (id) => invoke("get_meeting", { id }),
    deleteMeeting: (id) => invoke("delete_meeting", { id }),
    recoverMeeting: (id) => invoke("recover_meeting", { id }),
    listSources: () => invoke("list_sources"),
    recordingState: () => invoke("recording_state"),
    startRecording: (req) => invoke("start_recording", { req }),
    stopRecording: () => invoke("stop_recording"),
    addSource: (sourceId) => invoke("add_source", { sourceId }),
    removeSource: (sourceId) => invoke("remove_source", { sourceId }),
    takeScreenshot: () => invoke("take_screenshot"),
    getSettings: () => invoke("get_settings"),
    saveSettings: (settings) => invoke("save_settings", { settings }),
    onRecordingState: (cb) => listen("recording-state", (e) => cb(e.payload as never)),
    onMeetingsChanged: (cb) => listen("meetings-changed", () => cb()),
  };
}

export function isIpcError(e: unknown): e is IpcError {
  return typeof e === "object" && e !== null && "code" in e && "message" in e;
}
