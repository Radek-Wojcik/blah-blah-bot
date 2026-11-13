// Popup: pairing form and the capture toggle for the active tab. Opening the popup is the
// user gesture that grants the extension access to the active tab (activeTab).

const $ = (id) => document.getElementById(id);

function send(type, extra = {}) {
  return chrome.runtime.sendMessage({ target: "service-worker", type, ...extra }).then((reply) => {
    if (!reply || !reply.ok) throw new Error((reply && reply.error) || "no reply");
    return reply;
  });
}

function show(text, cls = "") {
  const el = $("status");
  el.textContent = text;
  el.className = cls;
}

let activeTabId = null;
let capturing = false;

function render(status) {
  $("pair").hidden = status.connected;
  $("capture").hidden = !status.connected;
  $("count").textContent = String(status.streams.length);
  capturing = status.streams.some((s) => s.tabId === activeTabId);
  $("captureBtn").textContent = capturing ? "Stop this tab" : "Capture this tab";
}

async function refresh() {
  render(await send("status"));
}

async function startCapture() {
  await send("start", { tabId: activeTabId });
  show("Capturing this tab.", "ok");
}

async function main() {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  activeTabId = tab ? tab.id : null;
  const stored = await chrome.storage.session.get("port");
  if (stored.port) $("port").value = String(stored.port);
  const local = await chrome.storage.local.get("auto");
  if (local.auto === false) $("auto").checked = false;

  await refresh();

  $("pairBtn").onclick = async () => {
    try {
      await send("pair", { port: Number($("port").value), token: $("token").value.trim() });
      $("token").value = "";
      show("Paired.", "ok");
      await refresh();
    } catch (err) {
      show(err.message, "err");
    }
  };

  $("captureBtn").onclick = async () => {
    try {
      if (capturing) {
        await send("stop", { tabId: activeTabId });
        show("Stopped.", "ok");
      } else {
        await startCapture();
      }
      await refresh();
    } catch (err) {
      show(err.message, "err");
    }
  };

  $("disconnectBtn").onclick = async () => {
    try {
      await send("disconnect");
      show("Disconnected.", "ok");
      await refresh();
    } catch (err) {
      show(err.message, "err");
    }
  };

  $("auto").onchange = () => chrome.storage.local.set({ auto: $("auto").checked });

  if ($("auto").checked && !$("capture").hidden && !capturing && activeTabId !== null) {
    try {
      await startCapture();
      await refresh();
    } catch (err) {
      show(err.message, "err");
    }
  }
}

main().catch((err) => show(err.message, "err"));
