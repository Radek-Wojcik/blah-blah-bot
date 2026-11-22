import type { Unsubscribe } from "./backend";
import { fail } from "./mock-util";
import type { BridgeSettings, BridgeStatus, PairedClientSetting } from "./types";

/** The code the mock offers, so screenshots are reproducible. */
export const MOCK_PAIRING_CODE = "7K3M-9QXD";
const CODE_TTL_MS = 5 * 60_000;

export interface MockBridgeOptions {
  /** Clock of the pairing expiry and of new pairings. */
  now: () => number;
  /** When the demo extension was paired. */
  demoPairedUnixMs: number;
  /**
   * When set, an extension "enters" the code this many milliseconds after pairing starts:
   * a new client appears and the code disappears, as with a real extension.
   */
  pairAfterMs?: number;
}

/** The bridge to the Chrome extension, simulated: pairing, paired clients and their state. */
export function createMockBridge(opts: MockBridgeOptions) {
  const settings: BridgeSettings = {
    enabled: true,
    port: 17555,
    portFallbacks: [17556, 17557, 17558, 17559, 17560, 17561, 17562, 17563, 17564],
    allowedExtensionIds: [],
    clients: [
      {
        id: "c-5f0e2a9b4c1d7e38",
        name: "Chrome 136",
        extVersion: "0.6.0",
        secretSha256: "",
        pairedUnixMs: opts.demoPairedUnixMs,
      },
    ],
    autoAddTabs: true,
  };
  const connected = new Set<string>(["c-5f0e2a9b4c1d7e38"]);
  let pairing: { code: string; expiresUnixMs: number } | null = null;
  let pairTimer: ReturnType<typeof setTimeout> | null = null;
  let nextClient = 2;
  const listeners = new Set<{ cb: (s: BridgeStatus) => void }>();

  const status = (): BridgeStatus => ({
    enabled: settings.enabled,
    listening: settings.enabled,
    port: settings.enabled ? settings.port : null,
    error: null,
    pairing: pairing && pairing.expiresUnixMs > opts.now() ? { ...pairing } : null,
    clients: settings.clients.map((c) => ({
      id: c.id,
      name: c.name,
      extVersion: c.extVersion,
      pairedUnixMs: c.pairedUnixMs,
      connected: settings.enabled && connected.has(c.id),
    })),
    streams: [],
  });
  const emit = () => listeners.forEach((l) => l.cb(status()));
  const clearTimer = () => {
    if (pairTimer !== null) clearTimeout(pairTimer);
    pairTimer = null;
  };

  return {
    /** The bridge part of `Settings`, as the backend returns it. */
    settings: (): BridgeSettings => structuredClone(settings),
    /** Take the options of a saved settings screen; the client list is kept. */
    save(next: BridgeSettings | undefined) {
      if (!next) return;
      if (!Number.isInteger(next.port) || next.port < 1024 || next.port > 65535)
        fail("invalid", "Bridge ports must be between 1024 and 65535");
      settings.enabled = next.enabled;
      settings.port = next.port;
      settings.autoAddTabs = next.autoAddTabs;
      if (!settings.enabled) {
        pairing = null;
        clearTimer();
      }
      emit();
    },
    methods: {
      bridgeStatus: async () => status(),
      startPairing: async () => {
        if (!settings.enabled)
          fail(
            "bridge_off",
            "The browser extension bridge is not running. Turn it on in the settings.",
          );
        pairing = { code: MOCK_PAIRING_CODE, expiresUnixMs: opts.now() + CODE_TTL_MS };
        clearTimer();
        if (opts.pairAfterMs !== undefined) {
          pairTimer = setTimeout(() => {
            const client: PairedClientSetting = {
              id: `c-mock${nextClient++}`,
              name: "Chrome 137",
              extVersion: "0.6.0",
              secretSha256: "",
              pairedUnixMs: opts.now(),
            };
            settings.clients.push(client);
            connected.add(client.id);
            pairing = null;
            pairTimer = null;
            emit();
          }, opts.pairAfterMs);
        }
        emit();
        return status();
      },
      cancelPairing: async () => {
        pairing = null;
        clearTimer();
        emit();
        return status();
      },
      unpairClient: async (clientId: string) => {
        if (!settings.clients.some((c) => c.id === clientId))
          fail("not_found", "This extension is not paired");
        settings.clients = settings.clients.filter((c) => c.id !== clientId);
        connected.delete(clientId);
        emit();
        return status();
      },
      onBridgeChanged: (cb: (s: BridgeStatus) => void): Promise<Unsubscribe> => {
        const entry = { cb };
        listeners.add(entry);
        return Promise.resolve(() => void listeners.delete(entry));
      },
    },
  };
}
