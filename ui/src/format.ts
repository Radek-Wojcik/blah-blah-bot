/** Format a duration as H:MM:SS (hours only when needed: M:SS below one hour). */
export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const ss = String(s).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/** Format a timestamp (milliseconds since the Unix epoch) as `YYYY-MM-DD HH:MM` in UTC. */
export function formatDateTimeUtc(unixMs: number): string {
  const d = new Date(unixMs);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getUTCFullYear()}-${p(d.getUTCMonth() + 1)}-${p(d.getUTCDate())} ${p(d.getUTCHours())}:${p(d.getUTCMinutes())}`;
}

/** Screenshot file name: sequence number and offset, e.g. `0001-00-12-41.png`. */
export function screenshotName(seq: number, offsetMs: number): string {
  const total = Math.floor(offsetMs / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const p = (n: number) => String(n).padStart(2, "0");
  return `${String(seq).padStart(4, "0")}-${p(h)}-${p(m)}-${p(s)}.png`;
}
