import type { Backend } from "./backend";

/**
 * The Stage 4 methods (cycles, report modes, language-model backends, drafts) the mock backend
 * does not implement yet. Each one rejects, so a view that calls it before the mock is
 * extended fails loudly. Delete an entry here when the mock gets the real implementation.
 */
function notImplemented(): Promise<never> {
  return Promise.reject(new Error("not implemented in the mock"));
}

const noEvents = async () => () => {};

export const stage4Stubs: Pick<
  Backend,
  | "setCloudKey"
  | "clearCloudKey"
  | "cloudKeyPresent"
  | "setExternalKey"
  | "clearExternalKey"
  | "externalKeyPresent"
  | "cloudPreview"
  | "confirmCloud"
  | "listCycles"
  | "listCycleRepos"
  | "getCycle"
  | "createCycle"
  | "saveCycle"
  | "listTemplates"
  | "getTemplate"
  | "saveTemplate"
  | "validateTemplate"
  | "addCycleRepo"
  | "pullCycleRepo"
  | "reportDefaults"
  | "getDraft"
  | "updateDraftItem"
  | "moveDraftItem"
  | "deleteDraftItem"
  | "addDraftItem"
  | "setDraftSections"
  | "setReportMode"
  | "acceptNewDraft"
  | "discardNewDraft"
  | "renderReport"
  | "onDraftChanged"
  | "onCyclesChanged"
> = {
  setCloudKey: notImplemented,
  clearCloudKey: notImplemented,
  cloudKeyPresent: notImplemented,
  setExternalKey: notImplemented,
  clearExternalKey: notImplemented,
  externalKeyPresent: notImplemented,
  cloudPreview: notImplemented,
  confirmCloud: notImplemented,
  listCycles: notImplemented,
  listCycleRepos: notImplemented,
  getCycle: notImplemented,
  createCycle: notImplemented,
  saveCycle: notImplemented,
  listTemplates: notImplemented,
  getTemplate: notImplemented,
  saveTemplate: notImplemented,
  validateTemplate: notImplemented,
  addCycleRepo: notImplemented,
  pullCycleRepo: notImplemented,
  reportDefaults: notImplemented,
  getDraft: notImplemented,
  updateDraftItem: notImplemented,
  moveDraftItem: notImplemented,
  deleteDraftItem: notImplemented,
  addDraftItem: notImplemented,
  setDraftSections: notImplemented,
  setReportMode: notImplemented,
  acceptNewDraft: notImplemented,
  discardNewDraft: notImplemented,
  renderReport: notImplemented,
  // Subscriptions do nothing rather than reject, so a view can mount.
  onDraftChanged: noEvents,
  onCyclesChanged: noEvents,
};
