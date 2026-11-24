import { describe, expect, it } from "vitest";
import { createMockBackend } from "./ipc/mock";
import type { SourceInfo, SourceKind } from "./ipc/types";
import {
  DOUBLE_RECORDING_WARNING,
  defaultSelection,
  doubleRecording,
  groupSources,
  kindLabel,
  restorePersistedSelection,
  selectionStableKeys,
  sourceStableKey,
} from "./picker";

function src(id: string, kind: SourceKind, label = id, extra: Partial<SourceInfo> = {}) {
  return { id, kind, label, active: true, ...extra } satisfies SourceInfo;
}

const mic = src("mic:default", "microphone", "Microphone");
const discord = src("app:1", "application", "Discord");
const whole = src("app:100", "browser", "Chrome (whole browser)", {
  hint: "The browser extension is not paired.",
});
const loose = src("tab:s1", "browser_tab", "Tab: Notes (docs.example.org)");
const g1a = src("tab:s2", "browser_tab", "Tab: Sync (meet.example.org)", { group: "Group 1" });
const g1b = src("tab:s3", "browser_tab", "Tab: Board (board.example.org)", { group: "Group 1" });
const g2 = src("tab:s4", "browser_tab", "Tab: Plan (plan.example.org)", { group: "Group 2" });

describe("groupSources", () => {
  it("orders the sections Microphone, Applications, Browser and leaves empty ones out", () => {
    const sections = groupSources([whole, discord, mic]);
    expect(sections.map((s) => s.title)).toEqual(["Microphone", "Applications", "Browser"]);
    expect(groupSources([discord]).map((s) => s.id)).toEqual(["applications"]);
    expect(groupSources([])).toEqual([]);
  });

  it("lists tabs by tab group with the whole browser last", () => {
    const browser = groupSources([whole, loose, g2, g1a, mic, g1b]).find(
      (s) => s.id === "browser",
    )!;
    const flat = browser.rows.map((r) => (r.type === "heading" ? `# ${r.text}` : r.source.id));
    expect(flat).toEqual([
      "# Group 1",
      "tab:s2",
      "tab:s3",
      "# Group 2",
      "tab:s4",
      "# Other tabs",
      "tab:s1",
      "app:100",
    ]);
  });

  it("shows loose tabs without a heading when there is no tab group", () => {
    const browser = groupSources([loose, whole]).find((s) => s.id === "browser")!;
    expect(browser.rows.map((r) => r.type)).toEqual(["source", "source"]);
  });

  it("keeps the whole browser alone when the extension offers no tab", () => {
    const browser = groupSources([mic, whole]).find((s) => s.id === "browser")!;
    expect(browser.rows).toEqual([{ type: "source", source: whole }]);
  });
});

describe("defaultSelection", () => {
  it("ticks the microphone and every captured tab, not the whole browser", () => {
    expect([...defaultSelection([discord, mic, loose, whole, g1a])].sort()).toEqual([
      "mic:default",
      "tab:s1",
      "tab:s2",
    ]);
  });

  it("falls back to the first source without a microphone and to nothing without sources", () => {
    expect([...defaultSelection([discord, whole])]).toEqual(["app:1"]);
    expect(defaultSelection([]).size).toBe(0);
  });
});

describe("doubleRecording", () => {
  const all = [mic, discord, whole, loose, g1a];

  it("warns when a tab and the whole browser are both chosen", () => {
    expect(doubleRecording(all, ["mic:default", "tab:s1", "app:100"])).toBe(
      DOUBLE_RECORDING_WARNING,
    );
  });

  it("is quiet for a tab alone, the whole browser alone, or other sources", () => {
    expect(doubleRecording(all, ["tab:s1", "tab:s2"])).toBeNull();
    expect(doubleRecording(all, ["app:100", "app:1"])).toBeNull();
    expect(doubleRecording(all, [])).toBeNull();
  });

  it("ignores ids that are not in the list", () => {
    expect(doubleRecording(all, ["tab:gone", "app:100"])).toBeNull();
  });
});

describe("kindLabel", () => {
  it("names each kind", () => {
    expect(kindLabel(mic)).toBe("mic");
    expect(kindLabel(loose)).toBe("tab");
    expect(kindLabel(whole)).toBe("all Chrome");
    expect(kindLabel(discord)).toBe("playing");
    expect(kindLabel({ ...discord, active: false })).toBe("idle");
  });
});

describe("sourceStableKey and restorePersistedSelection", () => {
  it("computes stable keys across PID changes", () => {
    expect(sourceStableKey(mic)).toBe("mic:default");
    expect(sourceStableKey(whole)).toBe("browser:chrome");
    expect(sourceStableKey(loose)).toBe("tab:s1");
    expect(sourceStableKey(src("s9", "browser_tab", "Tab"))).toBe("tab:s9");
    expect(sourceStableKey(src("app:1024", "application", "Discord"))).toBe("app_name:discord");
    expect(sourceStableKey(src("app:9912", "application", "Discord"))).toBe("app_name:discord");
  });

  it("restores persisted selection even when application PIDs changed", () => {
    const firstRun = [
      mic,
      src("app:1024", "application", "Discord"),
      src("app:2048", "application", "Microsoft Teams"),
      whole,
    ];
    const savedKeys = selectionStableKeys(firstRun, ["mic:default", "app:1024"]);
    expect(savedKeys).toEqual(["mic:default", "app_name:discord"]);

    const secondRun = [
      mic,
      src("app:8844", "application", "Discord"),
      src("app:3310", "application", "Microsoft Teams"),
      whole,
    ];
    const restored = restorePersistedSelection(secondRun, savedKeys);
    expect([...restored].sort()).toEqual(["app:8844", "mic:default"]);
  });

  it("falls back to defaultSelection when lastKeys is empty or matches no running source", () => {
    const list = [mic, discord, loose];
    expect([...restorePersistedSelection(list, [])].sort()).toEqual(["mic:default", "tab:s1"]);
    expect([...restorePersistedSelection(list, ["app_name:zoom"])].sort()).toEqual([
      "mic:default",
      "tab:s1",
    ]);
    expect(restorePersistedSelection([], ["mic:default"]).size).toBe(0);
  });
});

describe("mock backend UX Wave 2B IPC commands", () => {
  it("persists lastSourceKeys and localSave settings", async () => {
    const b = createMockBackend({ clockRate: 0 });
    expect((await b.getSettings()).lastSourceKeys).toEqual(["mic:default"]);
    const keys = await b.saveLastSources(["mic:default", "app_name:discord"]);
    expect(keys).toEqual(["mic:default", "app_name:discord"]);
    expect((await b.getSettings()).lastSourceKeys).toEqual(["mic:default", "app_name:discord"]);
  });

  it("manages queue GPU override, scheduleJob, and forceProcess", async () => {
    const b = createMockBackend({ clockRate: 0, jobStepMs: 40 });
    const q1 = await b.setQueueGpuOverride({ gpuMaxUtilPercent: 85, gpuIdleSeconds: 5 });
    expect(q1.gpuOverride).toEqual({ gpuMaxUtilPercent: 85, gpuIdleSeconds: 5 });

    const scheduled = await b.scheduleJob("m3", Date.now() + 60_000);
    expect(scheduled.scheduledUnixMs).toBeGreaterThan(0);
    expect(scheduled.state).toBe("waiting");

    const forced = await b.forceProcess("m3");
    expect(forced.scheduledUnixMs).toBeNull();
    expect(forced.state).toBe("running");
  });

  it("supports project prefs, wizardCreateProject, importMeeting, and exportMeetingMarkdown", async () => {
    const b = createMockBackend({ clockRate: 0 });
    const created = await b.wizardCreateProject({
      mode: "auto",
      id: "ORBIT2",
      name: "Orbit Two",
      outputLanguage: "pl",
      defaultReportMode: "auto",
      localSaveEnabled: true,
      localSaveDir: "C:\\Notes",
    });
    expect(created.id).toBe("ORBIT2");
    expect(created.languages.output).toBe("pl");
    expect(created.report.defaultMode).toBe("auto");

    const prefs = await b.getProjectPrefs("ORBIT2");
    expect(prefs.localSaveEnabled).toBe(true);
    expect(prefs.localSaveDir).toBe("C:\\Notes");

    const imported = await b.importMeeting({
      cycleId: "ORBIT2",
      filePath: "C:\\Videos\\sprint-demo.mp4",
      mediaMode: "audio_and_slides",
      reportMode: "auto",
      processNow: false,
    });
    expect(imported.summary.title).toBe("sprint-demo");
    expect(imported.screenshots.length).toBe(4);

    const exportedPath = await b.exportMeetingMarkdown(imported.summary.id);
    expect(exportedPath).toContain("C:\\Notes\\ORBIT2");
  });

  it("lists, plays sample clip, and enrolls global voice profiles", async () => {
    const b = createMockBackend({ clockRate: 0 });
    const initial = await b.listAllVoiceProfiles();
    expect(initial.length).toBeGreaterThanOrEqual(3);
    expect(initial[0].projects).toContain("NOMAD");

    const clip = await b.voiceProfileSampleClip(initial[0].id);
    expect(clip.length).toBeGreaterThan(44);
    expect(String.fromCharCode(...clip.slice(0, 4))).toBe("RIFF");

    const enrolled = await b.enrollVoiceProfile("Marta Test", "NOMAD", "C:\\Audio\\marta.wav");
    expect(enrolled.name).toBe("Marta Test");
    expect(enrolled.sampleCount).toBe(1);
    expect(enrolled.hasSampleClip).toBe(true);

    const detected = await b.autoDetectModels();
    expect(detected.llmModels.length).toBeGreaterThan(0);
    expect(detected.defaultLocalSaveDir).toBeTruthy();
  });
});
