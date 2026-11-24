// Google Meet detection by tab URL. Only the host is ever reported, never the meeting code.

export const MEET_HOST = "meet.google.com";

const CODE = /^\/[a-z]{3}-[a-z]{4}-[a-z]{3}\/?$/;

/** True for a call URL such as https://meet.google.com/abc-defg-hij (lobby or call). */
export function isMeetCallUrl(url: string | undefined | null): boolean {
  if (!url) return false;
  let u: URL;
  try {
    u = new URL(url);
  } catch {
    return false;
  }
  return u.protocol === "https:" && u.hostname === MEET_HOST && CODE.test(u.pathname);
}

export type MeetState = "joined" | "left";

/**
 * The next Meet state of a tab. A tab is `joined` once its URL is a call and it plays sound;
 * it stays joined (a quiet call is still a call) until the URL leaves the call or the tab
 * closes. Returns the state to report, or null when nothing changed.
 */
export function nextMeetState(
  joined: boolean,
  tab: { url?: string | null; audible?: boolean; closed?: boolean },
): MeetState | null {
  const inCall = !tab.closed && isMeetCallUrl(tab.url);
  if (!joined && inCall && tab.audible === true) return "joined";
  if (joined && !inCall) return "left";
  return null;
}
