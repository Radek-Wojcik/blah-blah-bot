// WebSocket client to the local receiver (127.0.0.1 only). One connection is shared
// by every captured tab; frames are tagged with the tab id.

import { helloMessage, packFrame, startMessage, stopMessage } from "./protocol.js";

const MAX_BUFFERED_BYTES = 4 * 1024 * 1024; // beyond this, frames are dropped and counted

export class Bridge {
  constructor() {
    this.ws = null;
    this.session = null; // resume secret returned by the receiver after pairing
  }

  get connected() {
    return this.ws !== null && this.ws.readyState === WebSocket.OPEN && this.session !== null;
  }

  /** Opens the socket and authenticates with the one-time token (or the session secret). */
  connect(port, token) {
    if (!Number.isInteger(port) || port < 1024 || port > 65535) {
      return Promise.reject(new Error("invalid port"));
    }
    this.close();
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(`ws://127.0.0.1:${port}/`);
      ws.binaryType = "arraybuffer";
      let settled = false;
      const fail = (err) => {
        if (!settled) {
          settled = true;
          reject(err);
        }
      };
      ws.onopen = () => ws.send(helloMessage({ token, session: token ? null : this.session }));
      ws.onerror = () => fail(new Error("connection failed (is the receiver running?)"));
      ws.onclose = (ev) => {
        if (this.ws === ws) this.ws = null;
        fail(new Error(`closed before pairing (code ${ev.code})`));
        if (this.onclose) this.onclose(ev);
      };
      ws.onmessage = (ev) => {
        if (typeof ev.data !== "string") return;
        const msg = JSON.parse(ev.data);
        if (msg.type === "hello_ok" && !settled) {
          settled = true;
          this.ws = ws;
          this.session = msg.session;
          resolve();
        } else if (msg.type === "error") {
          fail(new Error(msg.reason || "receiver error"));
        }
      };
    });
  }

  close() {
    if (this.ws) {
      const ws = this.ws;
      this.ws = null;
      ws.onclose = null;
      ws.close(1000);
    }
  }

  sendStart(tabId, title) {
    this.ws.send(startMessage(tabId, title));
  }

  sendStop(tabId, reason, stats) {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(stopMessage(tabId, reason, stats));
    }
  }

  /** Returns true when the frame was queued, false when it was dropped. */
  sendFrame(tabId, seq, pcm) {
    const ws = this.ws;
    if (!ws || ws.readyState !== WebSocket.OPEN || ws.bufferedAmount > MAX_BUFFERED_BYTES) {
      return false;
    }
    ws.send(packFrame(tabId, seq, performance.timeOrigin + performance.now(), pcm));
    return true;
  }
}
