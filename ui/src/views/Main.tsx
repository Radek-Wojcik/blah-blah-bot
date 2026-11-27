import { useCallback, useEffect, useRef, useState } from "react";
import { useBackend } from "../backend-context";
import { type StepId } from "../first-run";
import { formatDuration } from "../format";
import { useRecordingState } from "../hooks";
import { onSettingsChanged } from "../hotkeys";
import type { MouseButtonsMode } from "../ipc/types";
import {
  canGoBack,
  canGoForward,
  createHistory,
  type HistoryState,
  type MeetingTab,
  type Page,
  pushRoute,
  type Route,
  type SettingsTab,
  stepBack,
  stepForward,
} from "../navigation";
import { FirstRun } from "./FirstRun";
import { MeetingDetailView } from "./MeetingDetail";
import { ProcessPrompt } from "./ProcessPrompt";
import { ProjectDetailView } from "./ProjectDetail";
import { ProjectsView } from "./Projects";
import { QueueView } from "./Queue";
import { RecordView } from "./RecordView";
import { SettingsView } from "./Settings";

const SETTINGS_TABS = new Set<SettingsTab>([
  "models",
  "connectors",
  "preferences",
  "processing",
  "voices",
]);

function parseMeetingTab(value: string | null): MeetingTab | undefined {
  if (value === "draft" || value === "transcript" || value === "speakers") return value;
  return undefined;
}

function parseSettingsTab(value: string | null): SettingsTab | undefined {
  if (value && SETTINGS_TABS.has(value as SettingsTab)) return value as SettingsTab;
  return undefined;
}

/**
 * `?page=projects`, `?page=project&cycle=NOMAD`, `?page=record`, `?page=queue`,
 * `?page=settings`, `?page=meeting&meeting=m5`, plus backward-compatible aliases
 * (`meetings`, `cycles`, `cycle`, `briefing`, `draft`, `transcript`, `speakers`).
 */
function initialRoute(): Route {
  const params = new URLSearchParams(window.location.search);
  const page = params.get("page");
  const meetingId = params.get("meeting");
  const cycleId = params.get("cycle");
  const at = params.get("at");
  const atNumber = at === null || at === "" ? null : Number(at);
  const highlightMs = atNumber !== null && Number.isFinite(atNumber) ? atNumber : null;

  if (
    (page === "meeting" || page === "draft" || page === "transcript" || page === "speakers") &&
    meetingId
  ) {
    const meetingTab = page === "meeting" ? (parseMeetingTab(params.get("tab")) ?? "draft") : page;
    return { page: "meeting", meetingId, meetingTab, highlightMs };
  }
  if ((page === "project" || page === "cycle" || page === "briefing") && cycleId) {
    return { page: "project", meetingId: null, cycleId };
  }
  if (page === "record") return { page: "record", meetingId: null };
  if (page === "queue") return { page: "queue", meetingId: null };
  if (page === "settings") {
    return { page: "settings", meetingId: null, settingsTab: parseSettingsTab(params.get("tab")) };
  }
  return { page: "projects", meetingId: null };
}

/** The first-run wizard when it is open: at which step, and why it was opened there. */
interface Wizard {
  step?: StepId;
  reason?: string | null;
}

type PendingNav = { kind: "push"; route: Route } | { kind: "back" } | { kind: "forward" };

export function Main() {
  const backend = useBackend();
  const [history, setHistory] = useState<HistoryState>(() => createHistory(initialRoute()));
  const route = history.entries[history.index];
  // `undefined` until it is known whether the first-run setup was completed.
  const [wizard, setWizard] = useState<Wizard | null | undefined>(undefined);
  const [settingsDirty, setSettingsDirty] = useState(false);
  const [pendingNav, setPendingNav] = useState<PendingNav | null>(null);
  const [mouseButtons, setMouseButtons] = useState<MouseButtonsMode>("normal");
  const lastSideClickRef = useRef<{ button: number; time: number } | null>(null);

  useEffect(() => {
    let cancelled = false;
    void backend.getProcessingSettings().then((s) => {
      if (!cancelled) setWizard((cur) => (cur === undefined ? (s.firstRunDone ? null : {}) : cur));
    });
    return () => {
      cancelled = true;
    };
  }, [backend]);

  useEffect(() => {
    let cancelled = false;
    const load = () => {
      void backend
        .getSettings()
        .then((s) => {
          if (!cancelled) setMouseButtons(s.mouseButtons ?? "normal");
        })
        .catch(() => {});
    };
    load();
    const unsub = onSettingsChanged(load);
    return () => {
      cancelled = true;
      unsub();
    };
  }, [backend]);

  const openSetup = useCallback((step?: StepId, reason?: string | null) => {
    setWizard({ step, reason });
  }, []);

  const page = route.page;

  const navigate = useCallback(
    (next: Route) => {
      if (page === "settings" && settingsDirty && next.page !== "settings") {
        setPendingNav({ kind: "push", route: next });
        return;
      }
      setPendingNav(null);
      if (next.page !== "settings") setSettingsDirty(false);
      setHistory((h) => pushRoute(h, next));
    },
    [page, settingsDirty],
  );

  const goBack = useCallback(() => {
    if (!canGoBack(history)) return;
    if (page === "settings" && settingsDirty) {
      setPendingNav({ kind: "back" });
      return;
    }
    setPendingNav(null);
    setSettingsDirty(false);
    setHistory((h) => stepBack(h));
  }, [history, page, settingsDirty]);

  const goForward = useCallback(() => {
    if (!canGoForward(history)) return;
    if (page === "settings" && settingsDirty) {
      setPendingNav({ kind: "forward" });
      return;
    }
    setPendingNav(null);
    setSettingsDirty(false);
    setHistory((h) => stepForward(h));
  }, [history, page, settingsDirty]);

  useEffect(() => {
    const navigationOff = mouseButtons === "off" || wizard !== null;

    const isBlocked = () =>
      document.querySelector('[role="dialog"], [role="alertdialog"]') !== null ||
      document.querySelector('[data-hotkey-capturing="true"]') !== null;

    const handleSideDown = (e: MouseEvent) => {
      if (e.button === 3 || e.button === 4) {
        e.preventDefault();
      }
    };

    const handleSideActivate = (e: MouseEvent) => {
      if (e.button !== 3 && e.button !== 4) return;
      // Always swallow the side buttons so the web view never navigates by itself.
      e.preventDefault();
      if (navigationOff || isBlocked()) return;

      const now = Date.now();
      const prev = lastSideClickRef.current;
      if (prev && prev.button === e.button && now - prev.time < 80) {
        return;
      }
      lastSideClickRef.current = { button: e.button, time: now };

      const wantBack = mouseButtons === "swapped" ? e.button === 4 : e.button === 3;
      if (wantBack) {
        goBack();
      } else {
        goForward();
      }
    };

    window.addEventListener("mousedown", handleSideDown);
    window.addEventListener("mouseup", handleSideActivate);
    window.addEventListener("auxclick", handleSideActivate);
    return () => {
      window.removeEventListener("mousedown", handleSideDown);
      window.removeEventListener("mouseup", handleSideActivate);
      window.removeEventListener("auxclick", handleSideActivate);
    };
  }, [mouseButtons, wizard, goBack, goForward]);

  const setPage = (p: Page) => navigate({ page: p, meetingId: null });
  const inProjects = page === "projects" || page === "project" || page === "meeting";
  const recording = useRecordingState();

  useEffect(() => {
    let off: (() => void) | undefined;
    let stopped = false;
    void backend
      .onOpenBriefing((cycleId) => navigate({ page: "project", meetingId: null, cycleId }))
      .then((u) => (stopped ? u() : (off = u)));
    return () => {
      stopped = true;
      off?.();
    };
  }, [backend, navigate]);

  if (wizard === undefined) return null;
  if (wizard !== null)
    return (
      <FirstRun
        initialStep={wizard.step}
        reason={wizard.reason}
        onClose={() => setWizard(null)}
        onOpenSettings={() => {
          setWizard(null);
          setSettingsDirty(false);
          setHistory((h) =>
            pushRoute(h, { page: "settings", meetingId: null, settingsTab: "models" }),
          );
        }}
      />
    );
  return (
    <div className="main">
      <nav className="panel sidebar" aria-label="Main navigation">
        <div className="panel-body">
          <h1 className="brand glitch" data-text="BlahBlahBot">
            Blah<span>Blah</span>Bot
          </h1>
          <button className={inProjects ? "nav nav-on" : "nav"} onClick={() => setPage("projects")}>
            Projects
          </button>
          <button
            className={page === "record" ? "nav nav-on" : "nav"}
            onClick={() => setPage("record")}
          >
            Record
          </button>
          <button
            className={page === "queue" ? "nav nav-on" : "nav"}
            onClick={() => setPage("queue")}
          >
            Queue
          </button>
          <button
            className={page === "settings" ? "nav nav-on" : "nav"}
            onClick={() => setPage("settings")}
          >
            Settings
          </button>
          {recording.status === "recording" && (
            <p className="side-live">
              <span className="dot dot-live" aria-hidden="true" /> REC{" "}
              {formatDuration(recording.elapsedMs)}
            </p>
          )}
        </div>
      </nav>
      <section className="content">
        <div className="content-inner">
          {page === "projects" && (
            <ProjectsView
              onOpenProject={(cycleId: string) =>
                navigate({ page: "project", meetingId: null, cycleId })
              }
              onOpenMeeting={(meetingId: string) =>
                navigate({
                  page: "meeting",
                  meetingId,
                  meetingTab: "draft",
                  returnCycleId: null,
                })
              }
              onOpenSetup={openSetup}
            />
          )}
          {page === "project" && route.cycleId && (
            <ProjectDetailView
              key={route.cycleId}
              cycleId={route.cycleId}
              onBack={() => setPage("projects")}
              onOpenMeeting={(meetingId: string) =>
                navigate({
                  page: "meeting",
                  meetingId,
                  meetingTab: "draft",
                  returnCycleId: route.cycleId,
                })
              }
              onOpenSetup={openSetup}
            />
          )}
          {page === "record" && <RecordView />}
          {page === "queue" && (
            <QueueView
              onOpenDraft={(id) =>
                navigate({ page: "meeting", meetingId: id, meetingTab: "draft" })
              }
            />
          )}
          {page === "meeting" && route.meetingId && (
            <MeetingDetailView
              key={route.meetingId}
              meetingId={route.meetingId}
              initialTab={route.meetingTab}
              highlightMs={route.highlightMs}
              onTabChange={(meetingTab) =>
                setHistory((h) => pushRoute(h, { ...route, meetingTab }))
              }
              onBack={() => {
                if (route.returnCycleId) {
                  navigate({ page: "project", meetingId: null, cycleId: route.returnCycleId });
                } else {
                  setPage("projects");
                }
              }}
              onOpenQueue={() => setPage("queue")}
            />
          )}
          {page === "settings" && (
            <SettingsView
              initialTab={route.settingsTab}
              onTabChange={(settingsTab) =>
                setHistory((h) => pushRoute(h, { ...route, settingsTab }))
              }
              onBack={() => (canGoBack(history) ? goBack() : setPage("projects"))}
              onDirtyChange={setSettingsDirty}
              pendingExit={pendingNav !== null}
              onConfirmExit={() => {
                const next = pendingNav;
                setPendingNav(null);
                setSettingsDirty(false);
                if (!next) return;
                if (next.kind === "push") {
                  setHistory((h) => pushRoute(h, next.route));
                } else if (next.kind === "back") {
                  setHistory((h) => stepBack(h));
                } else if (next.kind === "forward") {
                  setHistory((h) => stepForward(h));
                }
              }}
              onCancelExit={() => setPendingNav(null)}
              onRunSetup={() => openSetup("welcome")}
            />
          )}
        </div>
      </section>
      <ProcessPrompt onOpenSetup={openSetup} />
    </div>
  );
}
