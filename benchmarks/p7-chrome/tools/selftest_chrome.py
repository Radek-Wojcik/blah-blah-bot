"""Automated partial check of the P7 extension (everything except tabCapture itself).

Launches Chrome with a temporary profile and the unpacked extension, then runs two stages.

Stage 1 (data path): the extension's own `selftest.html` feeds synthetic tone streams through the
real bridge, multiplexer and AudioWorklet: tab 101 starts, tab 102 joins, tab 101 stops cleanly and
restarts. The receiver verdict is written to `benchmarks/results/p7-chrome-selftest.json`.

Stage 2 (control path): messages are sent to the service worker like the popup does: pair (creates
the offscreen document with reason USER_MEDIA and opens the WebSocket), start (expected to be
refused by Chrome because the extension was not invoked on the tab by a user gesture), status and
disconnect. The refusal is the correct behaviour and is not worked around.

    uv run --group p7 python p7-chrome/tools/selftest_chrome.py
"""

from __future__ import annotations

import asyncio
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.parse
import urllib.request
from pathlib import Path

from websockets.asyncio.client import connect

from bbb_bench.proto.p7_protocol import DEFAULT_EXTENSION_ID, new_token

HERE = Path(__file__).resolve().parent
EXTENSION_DIR = HERE.parent / "extension"
CHROME = os.environ.get("BBB_CHROME", r"C:\Program Files\Google\Chrome\Application\chrome.exe")
DEBUG_PORT = 9333
STAGE1_PORT = 17556
STAGE2_PORT = 17557
STAGE1_S = 12  # selftest.js runs about 10 s

CONTROL_SCRIPT = """
(async () => {
  const send = (m) => Promise.race([
    chrome.runtime.sendMessage({target: 'service-worker', ...m}),
    new Promise((resolve) => setTimeout(() => resolve({timeout: true}), 8000)),
  ]).catch((e) => ({thrown: String(e)}));
  const out = {};
  out.pair = await send({type: 'pair', port: PORT, token: 'TOKEN'});
  const tabs = await chrome.tabs.query({});
  out.start = await send({type: 'start', tabId: tabs[0].id});
  out.status = await send({type: 'status'});
  out.disconnect = await send({type: 'disconnect'});
  return JSON.stringify(out);
})()
"""


def devtools_json(path: str):
    url = f"http://127.0.0.1:{DEBUG_PORT}{path}"
    with urllib.request.urlopen(url, timeout=3) as resp:
        return json.load(resp)


def wait_for_extension(timeout_s: float = 15.0) -> bool:
    """True once the unpacked extension's service worker shows up as a DevTools target."""
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        try:
            targets = devtools_json("/json")
        except OSError:
            targets = []
        if any(DEFAULT_EXTENSION_ID in t.get("url", "") for t in targets):
            return True
        time.sleep(0.5)
    return False


def open_page(url: str) -> None:
    req = urllib.request.Request(
        f"http://127.0.0.1:{DEBUG_PORT}/json/new?{urllib.parse.quote(url, safe='')}",
        method="PUT",
    )
    urllib.request.urlopen(req, timeout=5).read()


def selftest_target() -> dict | None:
    pages = [t for t in devtools_json("/json") if t.get("type") == "page"]
    return next((t for t in pages if "selftest.html" in t.get("url", "")), None)


async def evaluate(ws_url: str, expression: str) -> str:
    async with connect(ws_url, max_size=None) as ws:
        params = {"expression": expression, "returnByValue": True, "awaitPromise": True}
        await ws.send(json.dumps({"id": 1, "method": "Runtime.evaluate", "params": params}))
        reply = json.loads(await asyncio.wait_for(ws.recv(), 60))
        return str(reply["result"]["result"].get("value", reply))


def start_receiver(
    port: int, name: str, token: str, duration: float, *extra: str
) -> subprocess.Popen:
    return subprocess.Popen(
        [
            sys.executable,
            "-m",
            "bbb_bench.proto.p7_receiver",
            "--port",
            str(port),
            "--name",
            name,
            "--token",
            token,
            "--duration",
            str(duration),
            "--quiet",
            *extra,
        ]
    )


def run_stages() -> None:
    token1, token2 = new_token(), new_token()
    receiver1 = start_receiver(STAGE1_PORT, "selftest", token1, STAGE1_S + 6)
    receiver2 = start_receiver(
        STAGE2_PORT, "selftest-control", token2, STAGE1_S + 20, "--no-result"
    )
    time.sleep(1.5)
    with tempfile.TemporaryDirectory(prefix="bbb-p7-profile-", ignore_cleanup_errors=True) as prof:
        chrome = subprocess.Popen(
            [
                CHROME,
                "--headless=new",
                f"--user-data-dir={prof}",
                f"--load-extension={EXTENSION_DIR}",
                f"--remote-debugging-port={DEBUG_PORT}",
                "--autoplay-policy=no-user-gesture-required",
                "--disable-features=DisableLoadExtensionCommandLineSwitch",
                "--no-first-run",
                "--no-default-browser-check",
                "about:blank",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        try:
            if not wait_for_extension():
                print("extension did not load (is --load-extension honoured by this Chrome?)")
                return
            base = f"chrome-extension://{DEFAULT_EXTENSION_ID}/selftest.html"
            open_page(f"{base}?port={STAGE1_PORT}&token={token1}")
            time.sleep(STAGE1_S)
            target = selftest_target()
            if target is None:
                print("selftest page not found")
                return
            ws_url = target["webSocketDebuggerUrl"]
            print("--- stage 1 page ---")
            print(asyncio.run(evaluate(ws_url, "document.title + ' | ' + document.body.innerText")))
            print("--- stage 2 control path (service worker + offscreen document) ---")
            script = CONTROL_SCRIPT.replace("PORT", str(STAGE2_PORT)).replace("TOKEN", token2)
            print(asyncio.run(evaluate(ws_url, script)))
        finally:
            # Only the browser started here is stopped, by process id.
            subprocess.run(
                ["taskkill", "/PID", str(chrome.pid), "/T", "/F"], capture_output=True, check=False
            )
            try:
                chrome.wait(timeout=15)  # let go of the profile files before they are removed
            except subprocess.TimeoutExpired:
                pass
            # The context manager ignores a failed removal; do not leave the profile behind.
            for _ in range(10):
                if not Path(prof).exists():
                    break
                shutil.rmtree(prof, ignore_errors=True)
                time.sleep(1)
    for proc in (receiver1, receiver2):
        proc.wait(timeout=90)


if __name__ == "__main__":
    run_stages()
