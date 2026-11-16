import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Backend } from "./backend";
import type { IpcError } from "./types";

/**
 * Object URL for PNG bytes returned by a command. The bytes arrive as an `ArrayBuffer` over the
 * custom IPC protocol, but as a plain array of numbers when the web view falls back to
 * `postMessage` (for example when the CSP blocks the protocol). A `Blob` built from such an
 * array would hold its text ("137,80,78,..."), which is not an image, so convert it first.
 */
export function pngObjectUrl(bytes: ArrayBuffer | number[]): string {
  const data = bytes instanceof ArrayBuffer ? bytes : new Uint8Array(bytes);
  return URL.createObjectURL(new Blob([data], { type: "image/png" }));
}

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
    overlayImage: async () => pngObjectUrl(await invoke<ArrayBuffer | number[]>("overlay_image")),
    overlayFinish: (region) => invoke("overlay_finish", { region }),
    processMeeting: (id, options) => invoke("process_meeting", { id, options: options ?? null }),
    queueState: () => invoke("queue_state"),
    retryJob: (id) => invoke("retry_job", { id }),
    cancelJob: (id) => invoke("cancel_job", { id }),
    getTranscript: (id) => invoke("get_transcript", { id }),
    editSegment: (id, segmentId, edit) => invoke("edit_segment", { id, segmentId, edit }),
    meetingImage: async (id, file) => {
      return pngObjectUrl(await invoke<ArrayBuffer | number[]>("meeting_image", { id, file }));
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
    listParticipants: (cycleId, meetingId) => invoke("list_participants", { cycleId, meetingId }),
    createParticipant: (name, cycleId) => invoke("create_participant", { name, cycleId }),
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
    setCloudKey: (key) => invoke("set_cloud_key", { key }),
    clearCloudKey: () => invoke("clear_cloud_key"),
    cloudKeyPresent: () => invoke("cloud_key_present"),
    setExternalKey: (key) => invoke("set_external_key", { key }),
    clearExternalKey: () => invoke("clear_external_key"),
    externalKeyPresent: () => invoke("external_key_present"),
    cloudPreview: (meetingId) => invoke("cloud_preview", { meetingId }),
    confirmCloud: (meetingId) => invoke("confirm_cloud", { meetingId }),
    listCycles: () => invoke("list_cycles"),
    listCycleRepos: () => invoke("list_cycle_repos"),
    getCycle: (cycleId) => invoke("get_cycle", { cycleId }),
    createCycle: (id, name, repoId) => invoke("create_cycle", { id, name, repoId }),
    saveCycle: (cycle) => invoke("save_cycle", { cycle }),
    listTemplates: (cycleId) => invoke("list_templates", { cycleId }),
    getTemplate: (cycleId, name) => invoke("get_template", { cycleId, name }),
    saveTemplate: (cycleId, template) => invoke("save_template", { cycleId, template }),
    validateTemplate: (template) => invoke("validate_template", { template }),
    addCycleRepo: (req) => invoke("add_cycle_repo", { req }),
    pullCycleRepo: (repoId) => invoke("pull_cycle_repo", { repoId }),
    reportDefaults: (cycleId) => invoke("report_defaults", { cycleId }),
    getDraft: (meetingId) => invoke("get_draft", { meetingId }),
    updateDraftItem: (meetingId, itemId, patch) =>
      invoke("update_draft_item", { meetingId, itemId, patch }),
    moveDraftItem: (meetingId, itemId, sectionId, beforeItemId) =>
      invoke("move_draft_item", { meetingId, itemId, sectionId, beforeItemId }),
    deleteDraftItem: (meetingId, itemId) => invoke("delete_draft_item", { meetingId, itemId }),
    addDraftItem: (meetingId, item) => invoke("add_draft_item", { meetingId, item }),
    setDraftSections: (meetingId, sections) =>
      invoke("set_draft_sections", { meetingId, sections }),
    setReportMode: (meetingId, mode, template, meetingType) =>
      invoke("set_report_mode", { meetingId, mode, template, meetingType }),
    acceptNewDraft: (meetingId) => invoke("accept_new_draft", { meetingId }),
    discardNewDraft: (meetingId) => invoke("discard_new_draft", { meetingId }),
    renderReport: (meetingId, variant) => invoke("render_report", { meetingId, variant }),
    setStatusDecision: (meetingId, proposalId, decision, comment) =>
      invoke("set_status_decision", { meetingId, proposalId, decision, comment }),
    addStatusProposal: (meetingId, target, status, comment) =>
      invoke("add_status_proposal", { meetingId, target, status, comment }),
    publishingSecrets: () => invoke("publishing_secrets"),
    setGithubToken: (token, api) => invoke("set_github_token", { token, api }),
    clearGithubToken: (api) => invoke("clear_github_token", { api }),
    testGithubAccess: (cycleId) => invoke("test_github_access", { cycleId }),
    setGithubRepo: (repoId, githubRepo, githubApi) =>
      invoke("set_github_repo", { repoId, githubRepo, githubApi }),
    setTargetSecret: (reference, value) => invoke("set_target_secret", { reference, value }),
    clearTargetSecret: (reference) => invoke("clear_target_secret", { reference }),
    testDiscordWebhook: (reference) => invoke("test_discord_webhook", { reference }),
    setTargetRule: (cycleId, target, mode) => invoke("set_target_rule", { cycleId, target, mode }),
    publishPlan: (meetingId, targets) => invoke("publish_plan", { meetingId, targets }),
    publishMeeting: (meetingId, targets) => invoke("publish_meeting", { meetingId, targets }),
    confirmPublicRepo: (cycleId) => invoke("confirm_public_repo", { cycleId }),
    outboxState: () => invoke("outbox_state"),
    retryOutboxOp: (id) => invoke("retry_outbox_op", { id }),
    cancelOutboxOp: (id) => invoke("cancel_outbox_op", { id }),
    cycleTasks: (cycleId) => invoke("cycle_tasks", { cycleId }),
    getSettings: () => invoke("get_settings"),
    saveSettings: (settings) => invoke("save_settings", { settings }),
    onRecordingState: (cb) => listen("recording-state", (e) => cb(e.payload as never)),
    onMeetingsChanged: (cb) => listen("meetings-changed", () => cb()),
    onQueueChanged: (cb) => listen("queue-changed", (e) => cb(e.payload as never)),
    onModelsProgress: (cb) => listen("models-progress", (e) => cb(e.payload as never)),
    onModelsChanged: (cb) => listen("models-changed", () => cb()),
    onDraftChanged: (cb) => listen("draft-changed", (e) => cb(e.payload as string)),
    onCyclesChanged: (cb) => listen("cycles-changed", () => cb()),
    onOutboxChanged: (cb) => listen("outbox-changed", (e) => cb(e.payload as never)),
  };
}

export function isIpcError(e: unknown): e is IpcError {
  return typeof e === "object" && e !== null && "code" in e && "message" in e;
}
