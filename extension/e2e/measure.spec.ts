// Continuity measurement: a synthetic tone goes through the real offscreen document (worklet,
// session, WebSocket) to the real bridge and into the recorder's PushTrack, for as long as
// BBB_MEASURE_MINUTES says (default 30), and the result is written to
// benchmarks/results/stage6-tab-30min-synthetic.json.
//
//   BBB_MEASURE_MINUTES=2 npm run measure     reduced run (what the file then says)
//   npm run measure                           the 30 minute run
//
// What this measures: the extension, the socket and the bridge on this machine. It does not
// measure Chrome's tab capture of a real page (a user must invoke the extension on a tab, so
// a script cannot start it): the real tab run is a separate manual measurement. Use an idle
// machine for the full run, as in P7.
import fs from "node:fs";
import path from "node:path";
import { test } from "@playwright/test";
import { hook, launchChrome, TEST_PORTS, type Chrome } from "./support/chrome";
import { pairViaPopup } from "./support/flows";
import { Receiver } from "./support/receiver";
import { cleanupDirs, freshDir, REPO_ROOT } from "./support/tmp";

const PLANNED_MINUTES = 30;
const GAP_LIMIT_MS = 100;
const RESULT = path.join(REPO_ROOT, "benchmarks", "results", "stage6-tab-30min-synthetic.json");

test("synthetic tab stream continuity", async () => {
  const minutes = Number(process.env.BBB_MEASURE_MINUTES ?? PLANNED_MINUTES);
  if (!(minutes > 0)) throw new Error("BBB_MEASURE_MINUTES must be a positive number");
  const work = freshDir("measure");
  let bc: Chrome | undefined;
  let receiver: Receiver | undefined;
  try {
    bc = await launchChrome(path.join(work, "profile"));
    receiver = await Receiver.start({
      ports: TEST_PORTS,
      code: "7K3M9QXD",
      extensionId: bc.extensionId,
      recordDir: path.join(work, "record"),
    });
    await pairViaPopup(bc, "7K3M9QXD");

    const started = await hook(bc, { type: "synthetic-start", tabId: 1, hz: 440 });
    if (!started.ok) throw new Error(`synthetic start failed: ${started.error}`);
    const streamId = started.streamId as string;
    await receiver.waitFor((e) => e.event === "stream_started" && e.stream_id === streamId);

    const t0 = Date.now();
    const total = minutes * 60_000;
    while (Date.now() - t0 < total) {
      await new Promise((r) => setTimeout(r, Math.min(60_000, total - (Date.now() - t0))));
      console.log(
        `measure: ${Math.round((Date.now() - t0) / 1000)} s of ${Math.round(total / 1000)} s`,
      );
    }
    await hook(bc, { type: "synthetic-stop", tabId: 1 });
    const end = await receiver.waitForEnd(streamId, 30_000);
    const runSeconds = (Date.now() - t0) / 1000;
    const s = end.stats;

    const gapOk = s.max_gap_ms <= GAP_LIMIT_MS && s.max_stall_ms <= GAP_LIMIT_MS;
    const lossOk = s.lost_frames === 0 && (s.recorder?.missing_frames ?? 0) === 0;
    const full = runSeconds >= PLANNED_MINUTES * 60 * 0.99;
    const result = {
      id: "stage6-tab-30min-synthetic",
      description:
        "Synthetic 440 Hz tone through the extension's offscreen document (AudioWorklet, session, WebSocket), the bridge and the recorder's PushTrack. Chrome's tab capture of a real page is not part of it.",
      date: new Date().toISOString().slice(0, 10),
      chrome: bc.version,
      platform: process.platform,
      planned_minutes: PLANNED_MINUTES,
      run_seconds: Math.round(runSeconds),
      full_run: full,
      stream: {
        frames: s.frames,
        lost_frames: s.lost_frames,
        max_gap_wall_ms: round(s.max_gap_ms),
        max_stall_ms: round(s.max_stall_ms),
        max_gap_sender_ms: round(s.max_sender_gap_ms),
        wall_s: round(s.wall_ms / 1000),
        audio_s: round(s.audio_ms / 1000),
        drift_arrival_ms: round(s.drift_ms),
        drift_sender_ms: round(s.sender_drift_ms),
      },
      recorder: s.recorder,
      criteria: {
        gap_limit_ms: GAP_LIMIT_MS,
        max_gap_within_limit: gapOk,
        no_lost_frames: lossOk,
      },
      overall: !full ? "incomplete" : gapOk && lossOk ? "pass" : "fail",
      notes: full
        ? "Full-length run."
        : `Reduced run of ${Math.round(runSeconds)} s: it shows the path works and the numbers so far, not the 30 minute criterion. Run npm run measure for the full run.`,
      manual: { machine_idle: null, real_tab_run: null },
    };
    fs.writeFileSync(RESULT, `${JSON.stringify(result, null, 2)}\n`);
    console.log(JSON.stringify(result, null, 2));
  } finally {
    try {
      await bc?.close();
    } finally {
      try {
        await receiver?.kill();
      } finally {
        cleanupDirs();
      }
    }
  }
});

function round(n: number): number {
  return Math.round(n * 10) / 10;
}
