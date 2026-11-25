import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Backend } from "./backend";
import { type IpcBytes, bytesToBlob } from "./bytes";
import { filtersFor, pickedPath } from "./pick";
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
      const bytes = await invoke<IpcBytes>("overlay_image");
      return URL.createObjectURL(bytesToBlob(bytes, "image/png"));
    },
    overlayReady: () => invoke("overlay_ready"),
    overlayFinish: (region) => invoke("overlay_finish", { region }),
    processMeeting: (id, options) => invoke("process_meeting", { id, options: options ?? null }),
    queueState: () => invoke("queue_state"),
    retryJob: (id) => invoke("retry_job", { id }),
    cancelJob: (id) => invoke("cancel_job", { id }),
    getTranscript: (id) => invoke("get_transcript", { id }),
    editSegment: (id, segmentId, edit) => invoke("edit_segment", { id, segmentId, edit }),
    meetingImage: async (id, file) => {
      const bytes = await invoke<IpcBytes>("meeting_image", { id, file });
      return URL.createObjectURL(bytesToBlob(bytes, "image/png"));
    },
    getMeetingVoices: (id) => invoke("get_meeting_voices", { id }),
    assignVoice: (id, voiceId, participantId) =>
      invoke("assign_voice", { id, voiceId, participantId }),
    assignVoiceNew: (id, voiceId, name) => invoke("assign_voice_new", { id, voiceId, name }),
    mergeVoices: (id, voiceId, into) => invoke("merge_voices", { id, voiceId, into }),
    confirmMatches: (id) => invoke("confirm_matches", { id }),
    voiceClip: async (id, voiceId, idx) => {
      const bytes = await invoke<IpcBytes>("voice_clip", { id, voiceId, idx });
      return URL.createObjectURL(bytesToBlob(bytes, "audio/wav"));
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
    setupStatus: () => invoke("setup_status"),
    detectLlamaServer: () => invoke("detect_llama_server"),
    findGgufModels: (dir) => invoke("find_gguf_models", { dir: dir ?? null }),
    checkSetupPath: (kind, path) => invoke("check_setup_path", { kind, path }),
    prepareWorkerEnv: () => invoke("prepare_worker_env"),
    setFirstRunDone: (done) => invoke("set_first_run_done", { done: done ?? true }),
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
    getBriefing: (cycleId, refresh) => invoke("get_briefing", { cycleId, refresh }),
    openBriefing: (cycleId) => invoke("open_briefing", { cycleId }),
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
    listCorrections: (meetingId) => invoke("list_corrections", { meetingId }),
    addCorrection: (meetingId, text, item, issue, date) =>
      invoke("add_correction", { meetingId, text, item, issue, date }),
    publishCorrections: (meetingId, ids) => invoke("publish_corrections", { meetingId, ids }),
    deleteCorrection: (meetingId, id) => invoke("delete_correction", { meetingId, id }),
    saveLastSources: (keys) => invoke("save_last_sources", { keys }),
    setQueueGpuOverride: (overrideCfg) => invoke("set_queue_gpu_override", { overrideCfg }),
    forceProcess: (meetingId) => invoke("force_process", { meetingId }),
    scheduleJob: (meetingId, scheduledUnixMs) =>
      invoke("schedule_job", { meetingId, scheduledUnixMs }),
    getProjectPrefs: (cycleId) => invoke("get_project_prefs", { cycleId }),
    saveProjectPrefs: (prefs) => invoke("save_project_prefs", { prefs }),
    wizardCreateProject: (req) => invoke("wizard_create_project", { req }),
    githubRepoStatus: () => invoke("github_repo_status"),
    wizardCreateProjectWithGithub: (req, github) =>
      invoke("wizard_create_project_with_github", { req, github }),
    importMeeting: (req) => invoke("import_meeting", { req }),
    exportMeetingMarkdown: (meetingId, destPath) =>
      invoke("export_meeting_markdown", { meetingId, destPath: destPath ?? null }),
    saveMeetingNoteAs: (meetingId, destPath) =>
      invoke("save_meeting_note_as", { meetingId, destPath }),
    // The dialog plugin is called directly: the page never reads or writes the files itself, it
    // only receives the chosen path.
    pickFile: async (req) =>
      pickedPath(
        await invoke("plugin:dialog|open", {
          options: {
            title: req.title,
            multiple: false,
            directory: false,
            filters: filtersFor(req.kind),
          },
        }),
      ),
    pickFolder: async (req) =>
      pickedPath(
        await invoke("plugin:dialog|open", {
          options: { title: req?.title, multiple: false, directory: true },
        }),
      ),
    saveFileAs: async (req) =>
      pickedPath(
        await invoke("plugin:dialog|save", {
          options: {
            title: req.title,
            defaultPath: req.defaultName,
            filters: [{ name: req.extension.toUpperCase(), extensions: [req.extension] }],
          },
        }),
      ),
    autoDetectModels: () => invoke("auto_detect_models"),
    listAllVoiceProfiles: () => invoke("list_all_voice_profiles"),
    voiceProfileSampleClip: (participantId) =>
      invoke("voice_profile_sample_clip", { participantId }),
    enrollVoiceProfile: (name, cycleId, audioPath) =>
      invoke("enroll_voice_profile", { name, cycleId, audioPath }),
    getSettings: () => invoke("get_settings"),
    saveSettings: (settings) => invoke("save_settings", { settings }),
    bridgeStatus: () => invoke("bridge_status"),
    startPairing: () => invoke("start_pairing"),
    cancelPairing: () => invoke("cancel_pairing"),
    unpairClient: (clientId) => invoke("unpair_client", { clientId }),
    getDetectionSettings: () => invoke("get_detection_settings"),
    saveDetectionSettings: (settings) => invoke("save_detection_settings", { settings }),
    detectionPrompt: () => invoke("detection_prompt"),
    answerDetection: (key, answer) => invoke("answer_detection", { key, answer }),
    onRecordingState: (cb) => listen("recording-state", (e) => cb(e.payload as never)),
    onRecordingLevels: (cb) => listen("recording-levels", (e) => cb(e.payload as never)),
    onMeetingsChanged: (cb) => listen("meetings-changed", () => cb()),
    onQueueChanged: (cb) => listen("queue-changed", (e) => cb(e.payload as never)),
    onModelsProgress: (cb) => listen("models-progress", (e) => cb(e.payload as never)),
    onSetupProgress: (cb) => listen("setup-progress", (e) => cb(e.payload as never)),
    onModelsChanged: (cb) => listen("models-changed", () => cb()),
    onDraftChanged: (cb) => listen("draft-changed", (e) => cb(e.payload as string)),
    onCyclesChanged: (cb) => listen("cycles-changed", () => cb()),
    onOutboxChanged: (cb) => listen("outbox-changed", (e) => cb(e.payload as never)),
    onOpenBriefing: (cb) => listen("open-briefing", (e) => cb(e.payload as string)),
    onBridgeChanged: (cb) => listen("bridge-changed", (e) => cb(e.payload as never)),
    onDetectionPrompt: (cb) => listen("detection-prompt", (e) => cb(e.payload as never)),
    onDetectionCleared: (cb) => listen("detection-cleared", (e) => cb(e.payload as string)),
  };
}

export function isIpcError(e: unknown): e is IpcError {
  return typeof e === "object" && e !== null && "code" in e && "message" in e;
}
