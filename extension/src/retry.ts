// Retry helper for capturing a tab again right after it was released (ADR 0021): Chrome needs
// a moment before the same tab can be captured once more.

export interface RetryOptions {
  intervalMs: number;
  maxMs: number;
  shouldRetry: (err: unknown) => boolean;
  sleep?: (ms: number) => Promise<void>;
}

export async function retry<T>(fn: () => Promise<T>, opts: RetryOptions): Promise<T> {
  const sleep = opts.sleep ?? ((ms) => new Promise<void>((r) => setTimeout(r, ms)));
  let waited = 0;
  for (;;) {
    try {
      return await fn();
    } catch (err) {
      if (!opts.shouldRetry(err) || waited >= opts.maxMs) throw err;
      await sleep(opts.intervalMs);
      waited += opts.intervalMs;
    }
  }
}

/** Errors that mean the tab is still being released. */
export function isBusyCaptureError(err: unknown): boolean {
  const text = String(err instanceof Error ? err.message : err);
  return /active stream|already.*captur|cannot capture|could not start/i.test(text);
}
