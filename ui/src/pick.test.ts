import { afterEach, describe, expect, it } from "vitest";
import { createMockBackend } from "./ipc/mock";
import { MOCK_PICK_DEFAULTS } from "./ipc/mock-pick";
import { filtersFor, pickedPath } from "./ipc/pick";
import { isoDateUtc, noteFileName } from "./note-export";

describe("pickedPath", () => {
  it("is null for a cancelled dialog", () => {
    expect(pickedPath(null)).toBeNull();
    expect(pickedPath(undefined)).toBeNull();
  });

  it("returns the path, also from a one-element list", () => {
    expect(pickedPath("C:\\Audio\\a.wav")).toBe("C:\\Audio\\a.wav");
    expect(pickedPath(["C:\\Audio\\a.wav"])).toBe("C:\\Audio\\a.wav");
  });

  it("throws instead of guessing when the dialog gives no usable path", () => {
    expect(() => pickedPath("")).toThrow(/did not return a path/);
    expect(() => pickedPath("  ")).toThrow(/did not return a path/);
    expect(() => pickedPath({ name: "a.wav" })).toThrow(/did not return a path/);
    expect(() => pickedPath(["a", "b"])).toThrow(/did not return a path/);
  });
});

describe("filtersFor", () => {
  it("offers audio and video for a recording and audio for a voice sample", () => {
    const media = filtersFor("media")[0]!;
    expect(media.extensions).toEqual(expect.arrayContaining(["mp3", "wav", "mp4", "mkv"]));
    const audio = filtersFor("audio")[0]!;
    expect(audio.extensions).toEqual(expect.arrayContaining(["wav", "mp3"]));
    expect(audio.extensions).not.toContain("mkv");
  });
});

describe("noteFileName", () => {
  it("is the title as a slug, the date and .md", () => {
    expect(noteFileName("Thruster test 7: pressure flat!", "2026-11-13")).toBe(
      "thruster-test-7-pressure-flat-2026-11-13.md",
    );
  });

  it("keeps letters of other alphabets and removes characters Windows does not allow", () => {
    expect(noteFileName('Zażółć "gęślą" <jaźń>/x', "2026-11-13")).toBe(
      "zażółć-gęślą-jaźń-x-2026-11-13.md",
    );
  });

  it("falls back to meeting-note and cuts a long title", () => {
    expect(noteFileName(null, "2026-11-13")).toBe("meeting-note-2026-11-13.md");
    expect(noteFileName("???", "2026-11-13")).toBe("meeting-note-2026-11-13.md");
    expect(noteFileName("x".repeat(200), "2026-11-13").length).toBe(60 + "-2026-11-13.md".length);
    expect(noteFileName("Title", "")).toBe("title.md");
  });

  it("formats the UTC date of a timestamp", () => {
    expect(isoDateUtc(Date.UTC(2026, 10, 13, 23, 59))).toBe("2026-11-13");
  });
});

describe("mock dialogs", () => {
  afterEach(() => {
    delete window.__bbbPick;
  });

  it("answer with deterministic fake paths by default", async () => {
    const b = createMockBackend({ clockRate: 0 });
    expect(await b.pickFile({ kind: "media" })).toBe(MOCK_PICK_DEFAULTS.media);
    expect(await b.pickFile({ kind: "audio" })).toBe(MOCK_PICK_DEFAULTS.audio);
    expect(await b.pickFolder()).toBe(MOCK_PICK_DEFAULTS.folder);
    expect(await b.saveFileAs({ defaultName: "n.md", extension: "md" })).toBe(
      `${MOCK_PICK_DEFAULTS.saveDir}\\n.md`,
    );
  });

  it("can be preset by an option, and a null is a cancelled dialog", async () => {
    const b = createMockBackend({
      clockRate: 0,
      pick: { file: "D:\\x\\a.mp3", folder: null, save: null },
    });
    expect(await b.pickFile({ kind: "media" })).toBe("D:\\x\\a.mp3");
    expect(await b.pickFolder()).toBeNull();
    expect(await b.saveFileAs({ defaultName: "n.md", extension: "md" })).toBeNull();
  });

  it("can be preset by the window hook, which wins over the option", async () => {
    const b = createMockBackend({ clockRate: 0, pick: { file: "D:\\opt.mp3" } });
    window.__bbbPick = { file: "D:\\hook.mp3" };
    expect(await b.pickFile({ kind: "media" })).toBe("D:\\hook.mp3");
  });

  it("lets the hook cancel a dialog that an option answered, and falls back per dialog", async () => {
    const b = createMockBackend({
      clockRate: 0,
      pick: { file: "D:\\opt.mp3", folder: "D:\\opt-folder" },
    });
    window.__bbbPick = { file: null };
    expect(await b.pickFile({ kind: "media" })).toBeNull();
    // A dialog the hook does not mention keeps the option's answer.
    expect(await b.pickFolder()).toBe("D:\\opt-folder");
  });

  it("offers the audio default for a voice sample and a Save as path with the proposed name", async () => {
    const b = createMockBackend({ clockRate: 0 });
    expect(await b.pickFile({ kind: "audio", title: "Choose a voice sample" })).toBe(
      MOCK_PICK_DEFAULTS.audio,
    );
    expect(await b.saveFileAs({ defaultName: "kickoff-2026-11-13.md", extension: "md" })).toBe(
      "Documents\\BlahBlahBot\\notes\\kickoff-2026-11-13.md",
    );
  });

  it("refuses a note path that is only an extension", async () => {
    const b = createMockBackend({ clockRate: 0 });
    await expect(b.saveMeetingNoteAs("m5", "C:\\Notes\\.md")).rejects.toMatchObject({
      code: "invalid",
    });
  });

  it("saves a note only to a .md path", async () => {
    const b = createMockBackend({ clockRate: 0 });
    expect(await b.saveMeetingNoteAs("m5", "C:\\Notes\\a.md")).toBe("C:\\Notes\\a.md");
    await expect(b.saveMeetingNoteAs("m5", "C:\\Notes\\a.txt")).rejects.toMatchObject({
      code: "invalid",
    });
  });
});
