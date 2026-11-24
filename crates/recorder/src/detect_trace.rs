//! Session traces for the meeting detector: the file format, a builder that records changes
//! only, anonymization, replay through [`crate::detect::Detector`] and scoring against labels.
//!
//! A trace holds enumeration only: which audio sessions existed, their state and a coarse peak
//! (none, low, high) over time. It never holds audio content, device names or display names.
//! The format (version 1):
//!
//! ```json
//! {
//!   "version": 1, "kind": "real", "description": "...", "poll_ms": 1000, "duration_ms": 600000,
//!   "apps": { "discord": { "kind": "conferencing" }, "app-1": { "kind": "other" } },
//!   "events": [
//!     { "t_ms": 0, "sessions": [ { "app": "discord", "pid": 1, "flow": "capture",
//!                                  "state": "active", "peak": "low" } ] },
//!     { "t_ms": 90000, "recording": ["discord"] },
//!     { "t_ms": 95000, "meet": [ { "tab": 1, "state": "joined" } ] }
//!   ],
//!   "labels": [ { "start_ms": 0, "end_ms": 600000, "app": "discord", "kind": "call", "note": "" } ],
//!   "expect": { "low": { "prompts": 1 }, "balanced": { "prompts": 1 }, "high": { "prompts": 1 } }
//! }
//! ```
//!
//! `events` hold changes only: an event with `sessions` replaces the list of sessions (an empty
//! list means none), an event without it leaves them as they were; the same holds for
//! `recording` (applications being recorded) and `meet` (Meet tabs joined or left at that
//! moment). Process ids are numbered by first appearance. Programs other than the known
//! conferencing applications are named `app-N` (browsers `browser-N`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::detect::{
    AppSession, DetectConfig, Detector, Event, Flow, Level, Sensitivity, SessionState,
};

/// Version of the trace format.
pub const TRACE_VERSION: u32 = 1;

/// Where a trace comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TraceKind {
    /// Recorded on a real machine (enumeration only, anonymized).
    Real,
    /// Made by hand or derived from a real trace to exercise a situation.
    Synthetic,
}

/// What kind of program an application is, for the anonymized names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppKind {
    Conferencing,
    Browser,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    pub kind: AppKind,
}

/// One session in a trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceSession {
    pub app: String,
    pub pid: u32,
    pub flow: Flow,
    pub state: SessionState,
    pub peak: Level,
    /// Name of the process that owns the session when it differs from the application (the
    /// webview child of the new Teams). Informational; replay ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

impl From<&TraceSession> for AppSession {
    fn from(s: &TraceSession) -> Self {
        AppSession {
            app: s.app.clone(),
            pid: s.pid,
            flow: s.flow,
            state: s.state,
            peak: s.peak,
        }
    }
}

impl From<&AppSession> for TraceSession {
    fn from(s: &AppSession) -> Self {
        TraceSession {
            app: s.app.clone(),
            pid: s.pid,
            flow: s.flow,
            state: s.state,
            peak: s.peak,
            via: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MeetState {
    Joined,
    Left,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetMessage {
    pub tab: u64,
    pub state: MeetState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceEvent {
    pub t_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sessions: Option<Vec<TraceSession>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub meet: Vec<MeetMessage>,
}

/// What a label says about a stretch of the trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelKind {
    /// A call: exactly one prompt is wanted.
    Call,
    /// Activity that must not raise a prompt (a device check, a recorded call, idle).
    NoPrompt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
    pub start_ms: u64,
    pub end_ms: u64,
    pub app: String,
    pub kind: LabelKind,
    #[serde(default)]
    pub note: String,
}

/// The prompts a trace must produce with one sensitivity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Expect {
    pub prompts: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trace {
    pub version: u32,
    pub kind: TraceKind,
    #[serde(default)]
    pub description: String,
    pub poll_ms: u64,
    pub duration_ms: u64,
    #[serde(default)]
    pub apps: BTreeMap<String, AppInfo>,
    pub events: Vec<TraceEvent>,
    #[serde(default)]
    pub labels: Vec<Label>,
    #[serde(default)]
    pub expect: BTreeMap<Sensitivity, Expect>,
}

impl Trace {
    /// Parse a trace and check its version.
    pub fn parse(json: &str) -> Result<Self, String> {
        let trace: Trace = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if trace.version != TRACE_VERSION {
            return Err(format!(
                "trace version {} is not supported (expected {TRACE_VERSION})",
                trace.version
            ));
        }
        Ok(trace)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Applications that are browsers (never detected through audio sessions).
    fn browsers(&self) -> Vec<String> {
        self.apps
            .iter()
            .filter(|(_, i)| i.kind == AppKind::Browser)
            .map(|(a, _)| a.clone())
            .collect()
    }
}

/// Builds a trace from polls, keeping the changes only.
#[derive(Debug, Clone)]
pub struct TraceBuilder {
    poll_ms: u64,
    events: Vec<TraceEvent>,
    last: Option<Vec<TraceSession>>,
}

impl TraceBuilder {
    pub fn new(poll_ms: u64) -> Self {
        Self {
            poll_ms,
            events: Vec::new(),
            last: None,
        }
    }

    /// Add one poll. An event is stored only when the sessions differ from the previous poll.
    pub fn push(&mut self, t_ms: u64, sessions: &[AppSession]) {
        let mut list: Vec<TraceSession> = sessions.iter().map(TraceSession::from).collect();
        list.sort_by(|a, b| (&a.app, a.pid, a.flow).cmp(&(&b.app, b.pid, b.flow)));
        if self.last.as_ref() == Some(&list) {
            return;
        }
        self.last = Some(list.clone());
        self.events.push(TraceEvent {
            t_ms,
            sessions: Some(list),
            recording: None,
            meet: Vec::new(),
        });
    }

    /// The finished trace. `apps` is derived from the names used.
    pub fn finish(self, kind: TraceKind, description: &str, duration_ms: u64) -> Trace {
        let mut trace = Trace {
            version: TRACE_VERSION,
            kind,
            description: description.to_string(),
            poll_ms: self.poll_ms,
            duration_ms,
            apps: BTreeMap::new(),
            events: self.events,
            labels: Vec::new(),
            expect: BTreeMap::new(),
        };
        trace.apps = apps_of(&trace, &[], &[]);
        trace
    }
}

fn session_apps(trace: &Trace) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for e in &trace.events {
        for s in e.sessions.iter().flatten() {
            if !seen.contains(&s.app) {
                seen.push(s.app.clone());
            }
        }
    }
    seen
}

fn apps_of(trace: &Trace, known: &[String], browsers: &[String]) -> BTreeMap<String, AppInfo> {
    session_apps(trace)
        .into_iter()
        .map(|a| {
            let kind = if known.contains(&a) {
                AppKind::Conferencing
            } else if browsers.contains(&a) {
                AppKind::Browser
            } else {
                AppKind::Other
            };
            (a, AppInfo { kind })
        })
        .collect()
}

/// Remove what identifies the machine or the user from a trace: process ids become the order
/// of their first appearance, programs that are not conferencing applications are named
/// `app-N` (browsers `browser-N`) in order of first appearance, and the `via` names are
/// dropped. Device and display names are never stored in the first place. Labels and
/// expectations are carried over with the new names.
pub fn anonymize(raw: &Trace, known: &[String], browsers: &[String]) -> Trace {
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut pids: BTreeMap<u32, u32> = BTreeMap::new();
    let (mut apps, mut browser_count) = (0u32, 0u32);
    for app in session_apps(raw) {
        let new = if known.contains(&app) {
            app.clone()
        } else if browsers.contains(&app) {
            browser_count += 1;
            format!("browser-{browser_count}")
        } else {
            apps += 1;
            format!("app-{apps}")
        };
        names.insert(app, new);
    }
    let rename = |a: &str| names.get(a).cloned().unwrap_or_else(|| a.to_string());
    let mut out = raw.clone();
    for e in &mut out.events {
        if let Some(list) = &mut e.sessions {
            for s in list.iter_mut() {
                let next = u32::try_from(pids.len()).map_or(u32::MAX, |n| n + 1);
                s.pid = *pids.entry(s.pid).or_insert(next);
                s.app = rename(&s.app);
                s.via = None;
            }
        }
        if let Some(list) = &mut e.recording {
            for a in list.iter_mut() {
                *a = rename(a);
            }
        }
    }
    for l in &mut out.labels {
        l.app = rename(&l.app);
    }
    out.apps = apps_of(&out, known, &[]);
    for (name, info) in &mut out.apps {
        if name.starts_with("browser-") {
            info.kind = AppKind::Browser;
        }
    }
    out
}

/// A prompt raised during a replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReplayPrompt {
    pub t_ms: u64,
    pub app: String,
    pub key: String,
}

/// Replay a trace through the detector with `cfg`, polling at the interval of the
/// configured sensitivity. Prompts are never answered, so a call is prompted once and
/// ends when its signal has been gone for the end-of-call time.
pub fn replay(trace: &Trace, cfg: &DetectConfig) -> Vec<ReplayPrompt> {
    let mut cfg = cfg.clone();
    for b in trace.browsers() {
        if !cfg.excluded_apps.contains(&b) {
            cfg.excluded_apps.push(b);
        }
    }
    let mut detector = Detector::new(cfg);
    let step = detector.poll_ms().max(1);
    let mut sessions: Vec<AppSession> = Vec::new();
    let mut next_event = 0;
    let mut out = Vec::new();
    let mut t = 0;
    while t <= trace.duration_ms {
        while let Some(e) = trace.events.get(next_event).filter(|e| e.t_ms <= t) {
            if let Some(list) = &e.sessions {
                sessions = list.iter().map(AppSession::from).collect();
            }
            if let Some(list) = &e.recording {
                detector.set_recorded(list);
            }
            for m in &e.meet {
                detector.meet(e.t_ms, m.tab, m.state == MeetState::Joined);
            }
            next_event += 1;
        }
        for event in detector.observe(t, &sessions) {
            if let Event::Prompt(p) = event {
                out.push(ReplayPrompt {
                    t_ms: t,
                    app: p.app,
                    key: p.key,
                });
            }
        }
        t += step;
    }
    out
}

/// What a replay scored against the labels of a trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TraceScore {
    /// Prompts raised.
    pub prompts: usize,
    /// Labelled calls.
    pub calls: usize,
    /// Prompts outside a call, or during a stretch labelled `no_prompt`.
    pub false_prompts: usize,
    /// Calls that got no prompt.
    pub missed_calls: usize,
    /// Prompts beyond the first of a call.
    pub duplicate_prompts: usize,
    /// Time from the start of each prompted call to its first prompt.
    pub delays_ms: Vec<u64>,
}

/// Score prompts against labels. A prompt belongs to a `call` label of the same application
/// when it falls between its start and end.
pub fn score(trace: &Trace, prompts: &[ReplayPrompt]) -> TraceScore {
    let calls: Vec<&Label> = trace
        .labels
        .iter()
        .filter(|l| l.kind == LabelKind::Call)
        .collect();
    let mut first: Vec<Option<u64>> = vec![None; calls.len()];
    let (mut false_prompts, mut duplicates) = (0, 0);
    for p in prompts {
        let hit = calls
            .iter()
            .position(|l| l.app == p.app && l.start_ms <= p.t_ms && p.t_ms <= l.end_ms);
        match hit {
            None => false_prompts += 1,
            Some(i) if first[i].is_some() => duplicates += 1,
            Some(i) => first[i] = Some(p.t_ms),
        }
    }
    TraceScore {
        prompts: prompts.len(),
        calls: calls.len(),
        false_prompts,
        missed_calls: first.iter().filter(|f| f.is_none()).count(),
        duplicate_prompts: duplicates,
        delays_ms: calls
            .iter()
            .zip(&first)
            .filter_map(|(l, f)| f.map(|t| t - l.start_ms))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sess(app: &str, pid: u32, flow: Flow, state: SessionState, peak: Level) -> AppSession {
        AppSession {
            app: app.into(),
            pid,
            flow,
            state,
            peak,
        }
    }

    #[test]
    fn the_builder_keeps_changes_only() {
        let mut b = TraceBuilder::new(1000);
        let idle = [sess(
            "zzz",
            9,
            Flow::Render,
            SessionState::Inactive,
            Level::None,
        )];
        for t in 0..5 {
            b.push(t * 1000, &idle);
        }
        let loud = [sess(
            "zzz",
            9,
            Flow::Render,
            SessionState::Active,
            Level::High,
        )];
        b.push(5_000, &loud);
        b.push(6_000, &loud);
        let trace = b.finish(TraceKind::Real, "test", 7_000);
        assert_eq!(trace.events.len(), 2);
        assert_eq!(trace.events[1].t_ms, 5_000);
        assert_eq!(trace.apps["zzz"].kind, AppKind::Other);
    }

    #[test]
    fn anonymizing_numbers_pids_and_names_and_keeps_conferencing_apps() {
        let mut b = TraceBuilder::new(1000);
        b.push(
            0,
            &[
                sess(
                    "mygame",
                    4242,
                    Flow::Render,
                    SessionState::Active,
                    Level::Low,
                ),
                sess(
                    "discord",
                    77,
                    Flow::Capture,
                    SessionState::Active,
                    Level::Low,
                ),
                sess("chrome", 5, Flow::Render, SessionState::Active, Level::High),
                sess(
                    "othertool",
                    9001,
                    Flow::Capture,
                    SessionState::Inactive,
                    Level::None,
                ),
            ],
        );
        let mut raw = b.finish(TraceKind::Real, "raw", 1000);
        raw.events[0].sessions.as_mut().unwrap()[0].via = Some("secret".into());
        raw.labels.push(Label {
            start_ms: 0,
            end_ms: 10,
            app: "mygame".into(),
            kind: LabelKind::NoPrompt,
            note: String::new(),
        });
        let known = vec!["discord".to_string()];
        let anon = anonymize(&raw, &known, &["chrome".to_string()]);
        let json = anon.to_json();
        for secret in ["mygame", "othertool", "chrome", "4242", "9001", "secret"] {
            assert!(!json.contains(secret), "{secret} left in the trace");
        }
        let names: Vec<_> = anon.events[0]
            .sessions
            .as_ref()
            .unwrap()
            .iter()
            .map(|s| (s.app.as_str(), s.pid))
            .collect();
        // Sorted by name before numbering: chrome, discord, mygame, othertool.
        assert_eq!(
            names,
            vec![("browser-1", 1), ("discord", 2), ("app-1", 3), ("app-2", 4)]
        );
        assert_eq!(anon.labels[0].app, "app-1");
        assert_eq!(anon.apps["discord"].kind, AppKind::Conferencing);
        assert_eq!(anon.apps["browser-1"].kind, AppKind::Browser);
        assert_eq!(anon.apps["app-2"].kind, AppKind::Other);
    }

    #[test]
    fn a_trace_survives_json_and_a_wrong_version_is_refused() {
        let mut b = TraceBuilder::new(1000);
        b.push(
            0,
            &[sess(
                "zoom",
                1,
                Flow::Capture,
                SessionState::Active,
                Level::Low,
            )],
        );
        let trace = b.finish(TraceKind::Synthetic, "x", 5_000);
        assert_eq!(Trace::parse(&trace.to_json()).unwrap(), trace);
        let bad = trace.to_json().replace("\"version\": 1", "\"version\": 2");
        assert!(Trace::parse(&bad).is_err());
    }

    fn call_trace() -> Trace {
        let mut b = TraceBuilder::new(1000);
        let call = [
            sess(
                "discord",
                1,
                Flow::Capture,
                SessionState::Active,
                Level::Low,
            ),
            sess(
                "discord",
                1,
                Flow::Render,
                SessionState::Active,
                Level::High,
            ),
        ];
        b.push(10_000, &call);
        b.push(200_000, &[]);
        let mut t = b.finish(TraceKind::Synthetic, "call", 400_000);
        t.labels.push(Label {
            start_ms: 10_000,
            end_ms: 200_000,
            app: "discord".into(),
            kind: LabelKind::Call,
            note: String::new(),
        });
        t
    }

    #[test]
    fn replay_and_score_a_call() {
        let trace = call_trace();
        let cfg = DetectConfig::default();
        let prompts = replay(&trace, &cfg);
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].t_ms, 18_000);
        let s = score(&trace, &prompts);
        assert_eq!(
            (
                s.prompts,
                s.calls,
                s.false_prompts,
                s.missed_calls,
                s.duplicate_prompts
            ),
            (1, 1, 0, 0, 0)
        );
        assert_eq!(s.delays_ms, vec![8_000]);
    }

    #[test]
    fn a_missed_call_a_false_prompt_and_a_duplicate_are_counted() {
        let mut trace = call_trace();
        trace.labels[0].app = "zoom".into();
        let prompts = replay(&trace, &DetectConfig::default());
        let s = score(&trace, &prompts);
        assert_eq!((s.false_prompts, s.missed_calls), (1, 1));

        let trace = call_trace();
        let twice = vec![
            ReplayPrompt {
                t_ms: 18_000,
                app: "discord".into(),
                key: "discord".into(),
            },
            ReplayPrompt {
                t_ms: 90_000,
                app: "discord".into(),
                key: "discord".into(),
            },
        ];
        let s = score(&trace, &twice);
        assert_eq!(
            (s.duplicate_prompts, s.false_prompts, s.missed_calls),
            (1, 0, 0)
        );
    }

    #[test]
    fn recording_and_meet_events_reach_the_detector() {
        let mut trace = call_trace();
        trace.events[0].recording = Some(vec!["discord".into()]);
        assert!(replay(&trace, &DetectConfig::default()).is_empty());

        let mut meet = Trace {
            events: vec![
                TraceEvent {
                    t_ms: 1_000,
                    sessions: None,
                    recording: None,
                    meet: vec![MeetMessage {
                        tab: 4,
                        state: MeetState::Joined,
                    }],
                },
                TraceEvent {
                    t_ms: 100_000,
                    sessions: None,
                    recording: None,
                    meet: vec![MeetMessage {
                        tab: 4,
                        state: MeetState::Left,
                    }],
                },
            ],
            ..call_trace()
        };
        meet.labels.clear();
        let prompts = replay(&meet, &DetectConfig::default());
        assert_eq!(prompts.len(), 1);
        assert_eq!((prompts[0].app.as_str(), prompts[0].t_ms), ("meet", 11_000));
    }
}
