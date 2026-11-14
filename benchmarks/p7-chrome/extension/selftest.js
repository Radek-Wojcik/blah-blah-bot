// Development self-test. Runs the real bridge and multiplexer (everything except
// tabCapture itself) against synthetic tone streams:
//   tab 101 starts, tab 102 joins 3 s later, tab 101 stops cleanly and restarts,
//   then everything stops. The receiver result file holds the verdict.

import { Bridge } from "./bridge.js";
import { StreamMux } from "./mux.js";

const params = new URLSearchParams(location.search);
const port = Number(params.get("port"));
const token = params.get("token");
const logEl = document.getElementById("log");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const log = (msg) => {
  logEl.textContent += `${msg}\n`;
};

function tone(ctx, freq) {
  const osc = ctx.createOscillator();
  osc.frequency.value = freq;
  const dest = ctx.createMediaStreamDestination();
  osc.connect(dest);
  osc.start();
  return { stream: dest.stream, osc };
}

async function run() {
  const bridge = new Bridge();
  const mux = new StreamMux(bridge, { workletUrl: chrome.runtime.getURL("pcm-worklet.js") });
  await bridge.connect(port, token);
  log("paired");

  const src = new AudioContext();
  await src.resume();
  const a1 = tone(src, 440);
  await mux.add(101, a1.stream, { title: "tone A", audible: false });
  log(`audio contexts: playback=${mux.playbackCtx.state} capture=${mux.captureCtx.state}`);
  await sleep(3000);
  const b = tone(src, 660);
  await mux.add(102, b.stream, { title: "tone B", audible: false });
  log("tab 102 added");
  await sleep(3000);
  await mux.stop(101, "user_stop");
  a1.osc.stop();
  log("tab 101 stopped");
  await sleep(1000);
  const a2 = tone(src, 880);
  await mux.add(101, a2.stream, { title: "tone A again", audible: false });
  log("tab 101 restarted");
  await sleep(3000);
  await mux.stopAll("user_stop");
  bridge.close();
  log("done");
  document.title = "P7 self-test done";
}

run().catch((err) => {
  log(`error: ${err.message}`);
  document.title = "P7 self-test failed";
});
