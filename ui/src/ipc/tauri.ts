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
    overlayImage: async () => {
      const bytes = await invoke<ArrayBuffer>("overlay_image");
      return URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
    },
    overlayFinish: (region) => invoke("overlay_finish", { region }),
    processMeeting: (id, options) => invoke("process_meeting", { id, options: options ?? null }),
    queueState: () => invoke("queue_state"),
    retryJob: (id) => invoke("retry_job", { id }),
    cancelJob: (id) => invoke("cancel_job", { id }),
    getTranscript: (id) => invoke("get_transcript", { id }),
    editSegment: (id, segmentId, edit) => invoke("edit_segment", { id, segmentId, edit }),
    meetingImage: async (id, file) => {
      const bytes = await invoke<ArrayBuffer | number[]>("meeting_image", { id, file });
      const data = bytes instanceof ArrayBuffer ? bytes : new Uint8Array(bytes);
      return URL.createObjectURL(new Blob([data], { type: "image/png" }));
    },
    getMeetingVoices: (id) => invoke("get_meeting_voices", { id }),
    assignVoice: (id, voiceId, participantId) =>
      invoke("assign_voice", { id, voiceId, participantId }),
    assignVoiceNew: (id, voiceId, name) => invoke("assign_voice_new", { id, voiceId, name }),
    mergeVoices: (id, voiceId, into) => invoke("merge_voices", { id, voiceId, into }),
    confirmMatches: (id) => invoke("confirm_matches", { id }),
    voiceClip: async (id, voiceId, idx) => {
      const bytes = await invoke<ArrayBuffer | number[]>("voice_clip", { id, voiceId, idx });
      const data = bytes instanceof ArrayBuffer ? bytes : new Uint8Array(bytes);
      return URL.createObjectURL(new Blob([data], { type: "audio/wav" }));
    },
    listParticipants: () => invoke("list_participants"),
    createParticipant: (name) => invoke("create_participant", { name }),
    renameParticipant: (participantId, name) =>
      invoke("rename_participant", { participantId, name }),
    deleteParticipant: (participantId) => invoke("delete_participant", { participantId }),
    resetProfile: (participantId) => invoke("reset_profile", { participantId }),
    deleteVoiceData: () => invoke("delete_voice_data"),
    getProcessingSettings: () => invoke("get_processing_settings"),
    saveProcessingSettings: (settings) => invoke("save_processing_settings", { settings }),
    setHfToken: (token) => invoke("set_hf_token", { token }),
    clearHfToken: () => invoke("clear_hf_token"),
    modelStatus: () => invoke("model_status"),
    downloadModels: (keys) => invoke("download_models", { keys }),
    getSettings: () => invoke("get_settings"),
    saveSettings: (settings) => invoke("save_settings", { settings }),
    onRecordingState: (cb) => listen("recording-state", (e) => cb(e.payload as never)),
    onMeetingsChanged: (cb) => listen("meetings-changed", () => cb()),
    onQueueChanged: (cb) => listen("queue-changed", (e) => cb(e.payload as never)),
    onModelsProgress: (cb) => listen("models-progress", (e) => cb(e.payload as never)),
    onModelsChanged: (cb) => listen("models-changed", () => cb()),
  };
}

export function isIpcError(e: unknown): e is IpcError {
  return typeof e === "object" && e !== null && "code" in e && "message" in e;
}
