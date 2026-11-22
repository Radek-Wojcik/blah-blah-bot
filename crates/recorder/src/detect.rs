//! Meeting detection rules. Pure logic: the engine takes observations (audio sessions mapped
//! to applications, Google Meet events from the extension) with a time stamp and returns
//! prompt events. Nothing here reads the clock or talks to Windows, so the rules are tested
//! on recorded and synthetic traces (`detect_trace`).
//!
//! Per call the engine moves through `Idle -> Candidate -> Prompted -> Cooldown`:
//!
//! - **Candidate:** a known conferencing application uses the microphone (an active capture
//!   session), or a Google Meet tab was reported as joined. The state is dropped as soon as
//!   the condition ends.
//! - **Prompted:** the condition held for the sustain time of the sensitivity and the render
//!   condition is met (the application also has a playback session; how strict this is
//!   depends on the sensitivity). One prompt is raised for the call.
//! - **Cooldown:** the user answered, or the application is being recorded already. No second
//!   prompt for the same call. The call ends when the microphone has been unused for the
//!   end-of-call time (Meet: when the extension reports `left`); a rest period after "Not now"
//!   may follow.
//!
//! Browsers are never detected through their audio sessions (a voice search or a video call
//! in a tab looks the same as a call); Meet comes from the extension.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Applications whose use of the microphone is taken as a call.
pub const KNOWN_APPS: [&str; 6] = [
    "discord",
    "discordptb",
    "ms-teams",
    "teams",
    "zoom",
    "slack",
];

/// Browsers and browser engines. Their audio sessions never raise a prompt, even when the user
/// adds one to the list of known applications.
pub const BROWSERS: [&str; 10] = [
    "chrome",
    "msedge",
    "firefox",
    "brave",
    "opera",
    "vivaldi",
    "chromium",
    "iexplore",
    "arc",
    "msedgewebview2",
];

/// The application name of a Google Meet call reported by the extension.
pub const MEET_APP: &str = "meet";

/// How long a sound on the playback session of an application counts as "sound now".
pub const SOUND_MEMORY_MS: u64 = 5_000;

/// Direction of an audio session: playback (render) or microphone (capture).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Flow {
    Render,
    Capture,
}

/// State of an audio session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    /// At least one stream of the session runs.
    Active,
    /// The session has streams, none of them runs.
    Inactive,
    /// The session has no streams left.
    Expired,
}

impl SessionState {
    /// Parse the state as the Windows audio stack names it (`Active`, `Inactive`, `Expired`).
    pub fn parse(text: &str) -> Self {
        match text.to_ascii_lowercase().as_str() {
            "active" => Self::Active,
            "expired" => Self::Expired,
            _ => Self::Inactive,
        }
    }
}

/// Coarse peak level of a session: no sound, quiet or loud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    None,
    Low,
    High,
}

impl Level {
    /// Peak values (0.0 to 1.0) below this count as silence.
    pub const NONE_BELOW: f32 = 0.002;
    /// Peak values from this on count as loud.
    pub const HIGH_FROM: f32 = 0.05;

    /// Coarse level of a peak reading; an unknown reading is silence.
    pub fn from_peak(peak: Option<f32>) -> Self {
        match peak {
            Some(p) if p >= Self::HIGH_FROM => Self::High,
            Some(p) if p >= Self::NONE_BELOW => Self::Low,
            _ => Self::None,
        }
    }
}

/// One audio session with the application it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSession {
    /// Application, as the lowercase executable name without `.exe` (`ms-teams`).
    pub app: String,
    pub pid: u32,
    pub flow: Flow,
    pub state: SessionState,
    pub peak: Level,
}

/// Lowercase executable name without the extension: `Discord.exe` becomes `discord`.
pub fn app_stem(process: &str) -> String {
    let lower = process.trim().to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
}

/// Name shown in the prompt.
pub fn display_name(app: &str) -> String {
    match app {
        "discord" => "Discord".into(),
        "discordptb" => "Discord PTB".into(),
        "ms-teams" | "teams" => "Microsoft Teams".into(),
        "zoom" => "Zoom".into(),
        "slack" => "Slack".into(),
        MEET_APP => "Google Meet".into(),
        other => {
            let mut chars = other.chars();
            chars.next().map_or_else(String::new, |c| {
                c.to_uppercase().collect::<String>() + chars.as_str()
            })
        }
    }
}

/// The application a process belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedApp {
    /// Application stem (see [`app_stem`]).
    pub app: String,
    /// Topmost process of that application found on the way up.
    pub top_pid: u32,
}

/// Longest parent chain that is followed.
const MAX_CHAIN: usize = 8;

/// Map a process to its application through the parent chain.
///
/// `lookup` gives the executable name and the parent of a process. The topmost ancestor whose
/// name is in `known` wins: the new Teams plays its audio from an `msedgewebview2` child of
/// `ms-teams.exe`, and a Discord helper is a child of `Discord.exe`. Without a known ancestor
/// the process stands for itself, represented by the topmost process of the same name (the
/// same rule the source list uses to record a whole application).
pub fn resolve_app(
    pid: u32,
    known: &[String],
    lookup: impl Fn(u32) -> Option<(String, Option<u32>)>,
) -> Option<ResolvedApp> {
    let (own_name, mut parent) = lookup(pid)?;
    let own = app_stem(&own_name);
    let mut best_known: Option<ResolvedApp> = known.contains(&own).then(|| ResolvedApp {
        app: own.clone(),
        top_pid: pid,
    });
    let mut same_name_top = pid;
    let mut same_name_ok = true;
    let mut seen = vec![pid];
    for _ in 0..MAX_CHAIN {
        let Some(p) = parent else { break };
        if seen.contains(&p) {
            break;
        }
        seen.push(p);
        let Some((name, next)) = lookup(p) else { break };
        let stem = app_stem(&name);
        if known.contains(&stem) {
            best_known = Some(ResolvedApp {
                app: stem.clone(),
                top_pid: p,
            });
        }
        if same_name_ok {
            if stem == own {
                same_name_top = p;
            } else {
                same_name_ok = false;
            }
        }
        parent = next;
    }
    Some(best_known.unwrap_or(ResolvedApp {
        app: own,
        top_pid: same_name_top,
    }))
}

/// How the playback side of an application must look before a prompt is raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderRule {
    /// A playback session of the application is active and has made sound lately.
    ActiveWithSound,
    /// The application has a playback session (any state but expired).
    SessionExists,
    /// No condition on playback.
    Ignored,
}

/// Sensitivity choice.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Sensitivity {
    /// Fewer prompts, later.
    Low,
    /// The default.
    #[default]
    Balanced,
    /// Earlier prompts, a few more false ones.
    High,
}

impl Sensitivity {
    pub const ALL: [Sensitivity; 3] = [Self::Low, Self::Balanced, Self::High];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Balanced => "balanced",
            Self::High => "high",
        }
    }
}

impl fmt::Display for Sensitivity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Sensitivity {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "low" => Ok(Self::Low),
            "balanced" => Ok(Self::Balanced),
            "high" => Ok(Self::High),
            other => Err(format!("unknown sensitivity `{other}`")),
        }
    }
}

/// The numbers of one sensitivity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preset {
    /// How long a known application must use the microphone before a prompt.
    pub sustain_ms: u64,
    /// Condition on the playback session of that application.
    pub render: RenderRule,
    /// How long a Meet call must be joined (audible) before a prompt.
    pub meet_audible_ms: u64,
    /// How long the microphone must stay unused for the call to be over.
    pub end_of_call_ms: u64,
    /// Rest after "Not now" beyond the end of the call (0: the rest of the call only).
    pub not_now_extra_ms: u64,
    /// Interval of the audio session poll.
    pub poll_ms: u64,
}

/// The presets of the three sensitivities. These numbers are the whole definition of a
/// sensitivity; changing the default or a number is a change of configuration, not of rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presets {
    pub low: Preset,
    pub balanced: Preset,
    pub high: Preset,
}

impl Default for Presets {
    fn default() -> Self {
        Self {
            low: Preset {
                sustain_ms: 20_000,
                render: RenderRule::ActiveWithSound,
                meet_audible_ms: 30_000,
                end_of_call_ms: 120_000,
                not_now_extra_ms: 30 * 60_000,
                poll_ms: 2_000,
            },
            balanced: Preset {
                sustain_ms: 8_000,
                render: RenderRule::SessionExists,
                meet_audible_ms: 10_000,
                end_of_call_ms: 60_000,
                not_now_extra_ms: 0,
                poll_ms: 1_000,
            },
            high: Preset {
                sustain_ms: 3_000,
                render: RenderRule::Ignored,
                meet_audible_ms: 3_000,
                end_of_call_ms: 30_000,
                not_now_extra_ms: 0,
                poll_ms: 1_000,
            },
        }
    }
}

impl Presets {
    pub fn get(&self, s: Sensitivity) -> Preset {
        match s {
            Sensitivity::Low => self.low,
            Sensitivity::Balanced => self.balanced,
            Sensitivity::High => self.high,
        }
    }
}

/// Everything that configures the detector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectConfig {
    /// Detection on or off.
    pub enabled: bool,
    pub sensitivity: Sensitivity,
    pub presets: Presets,
    /// Applications taken as conferencing apps (stems, see [`app_stem`]).
    pub known_apps: Vec<String>,
    /// Applications that never raise a prompt, whatever `known_apps` says.
    pub excluded_apps: Vec<String>,
    /// Applications the user opted out of ("Never for ...").
    pub opted_out: Vec<String>,
    /// Use Google Meet events from the extension.
    pub meet: bool,
}

impl Default for DetectConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sensitivity: Sensitivity::default(),
            presets: Presets::default(),
            known_apps: KNOWN_APPS.iter().map(|s| (*s).to_string()).collect(),
            excluded_apps: BROWSERS.iter().map(|s| (*s).to_string()).collect(),
            opted_out: Vec::new(),
            meet: true,
        }
    }
}

impl DetectConfig {
    /// The numbers in force.
    pub fn preset(&self) -> Preset {
        self.presets.get(self.sensitivity)
    }

    /// Whether the audio sessions of `app` may raise a prompt.
    pub fn watches(&self, app: &str) -> bool {
        self.enabled
            && self.known_apps.iter().any(|a| a == app)
            && !self.excluded_apps.iter().any(|a| a == app)
            && !self.opted_out.iter().any(|a| a == app)
    }

    fn watches_meet(&self) -> bool {
        self.enabled && self.meet && !self.opted_out.iter().any(|a| a == MEET_APP)
    }
}

/// What raised a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// A known application uses the microphone.
    Audio,
    /// The extension reported a joined Google Meet tab.
    Meet,
}

/// A prompt to raise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prompt {
    /// Identifies the call: the application, or `meet:<tab>`.
    pub key: String,
    pub app: String,
    pub display: String,
    pub source: Source,
}

/// What the detector asks the application to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Ask the user whether to record.
    Prompt(Prompt),
    /// Withdraw the prompt: the call ended, the application is recorded, or the user opted out.
    Cleared { key: String },
}

/// The user's answer to a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// Recording started (or is being started) for this call.
    Record,
    /// Not now: no second prompt for this call.
    NotNow,
    /// Never for this application.
    Never,
}

/// State of one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Idle,
    Candidate,
    Prompted,
    Cooldown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// The condition holds since this time.
    Candidate {
        since: u64,
    },
    Prompted,
    /// Answered or recorded. `extra_ms` of rest follow the end of the call.
    Cooldown {
        extra_ms: u64,
    },
    /// The call is over; nothing is raised for this key until `until`.
    Rest {
        until: u64,
    },
}

#[derive(Debug, Clone)]
struct Call {
    app: String,
    source: Source,
    stage: Stage,
    /// Since when the call has no signal (microphone unused); `None` while it has one.
    quiet_since: Option<u64>,
}

/// The detector. One instance per application run; feed it every poll.
#[derive(Debug, Clone)]
pub struct Detector {
    cfg: DetectConfig,
    calls: BTreeMap<String, Call>,
    /// Joined Meet tabs with the time the extension said so.
    meet_tabs: BTreeMap<u64, u64>,
    /// Applications being recorded.
    recorded: Vec<String>,
    /// Last time a playback session of an application was active and loud enough.
    last_sound: BTreeMap<String, u64>,
}

impl Detector {
    pub fn new(cfg: DetectConfig) -> Self {
        Self {
            cfg,
            calls: BTreeMap::new(),
            meet_tabs: BTreeMap::new(),
            recorded: Vec::new(),
            last_sound: BTreeMap::new(),
        }
    }

    pub fn config(&self) -> &DetectConfig {
        &self.cfg
    }

    /// Interval at which [`Detector::observe`] should be called.
    pub fn poll_ms(&self) -> u64 {
        self.cfg.preset().poll_ms
    }

    /// Replace the configuration. A prompt for a call that is not watched any more is
    /// withdrawn and its state dropped.
    pub fn set_config(&mut self, cfg: DetectConfig) -> Vec<Event> {
        self.cfg = cfg;
        let mut events = Vec::new();
        let gone: Vec<String> = self
            .calls
            .iter()
            .filter(|(_, c)| !self.allowed(c))
            .map(|(k, _)| k.clone())
            .collect();
        for key in gone {
            if let Some(call) = self.calls.remove(&key) {
                if call.stage == Stage::Prompted {
                    events.push(Event::Cleared { key });
                }
            }
        }
        events
    }

    fn allowed(&self, call: &Call) -> bool {
        match call.source {
            Source::Audio => self.cfg.watches(&call.app),
            Source::Meet => self.cfg.watches_meet(),
        }
    }

    /// Tell the detector which applications are being recorded (the stems; `meet` for a
    /// recorded Meet tab). No prompt is raised for them.
    pub fn set_recorded(&mut self, apps: &[String]) {
        self.recorded = apps.to_vec();
    }

    /// The extension reported a Google Meet tab as joined or left.
    pub fn meet(&mut self, now_ms: u64, tab_id: u64, joined: bool) {
        if joined {
            self.meet_tabs.entry(tab_id).or_insert(now_ms);
        } else {
            self.meet_tabs.remove(&tab_id);
        }
    }

    /// State of the call with this key.
    pub fn phase(&self, key: &str) -> Phase {
        match self.calls.get(key).map(|c| c.stage) {
            None | Some(Stage::Rest { .. }) => Phase::Idle,
            Some(Stage::Candidate { .. }) => Phase::Candidate,
            Some(Stage::Prompted) => Phase::Prompted,
            Some(Stage::Cooldown { .. }) => Phase::Cooldown,
        }
    }

    /// The prompt on offer, if any.
    pub fn prompt(&self) -> Option<Prompt> {
        self.calls
            .iter()
            .find(|(_, c)| c.stage == Stage::Prompted)
            .map(|(k, c)| prompt_of(k, c))
    }

    /// Apply the answer of the user to the prompt of `key`.
    pub fn answer(&mut self, key: &str, answer: Answer) {
        let extra = self.cfg.preset().not_now_extra_ms;
        let Some(call) = self.calls.get_mut(key) else {
            return;
        };
        if call.stage != Stage::Prompted {
            return;
        }
        match answer {
            Answer::Record => call.stage = Stage::Cooldown { extra_ms: 0 },
            Answer::NotNow => call.stage = Stage::Cooldown { extra_ms: extra },
            Answer::Never => {
                let app = call.app.clone();
                if !self.cfg.opted_out.contains(&app) {
                    self.cfg.opted_out.push(app);
                }
                self.calls.remove(key);
            }
        }
    }

    fn render_ok(&self, rule: RenderRule, app: &str, now: u64, sessions: &[AppSession]) -> bool {
        let renders = || {
            sessions
                .iter()
                .filter(|s| s.app == app && s.flow == Flow::Render)
        };
        match rule {
            RenderRule::Ignored => true,
            RenderRule::SessionExists => renders().any(|s| s.state != SessionState::Expired),
            RenderRule::ActiveWithSound => {
                renders().any(|s| s.state == SessionState::Active)
                    && self
                        .last_sound
                        .get(app)
                        .is_some_and(|t| now.saturating_sub(*t) <= SOUND_MEMORY_MS)
            }
        }
    }

    /// Feed one poll of the audio sessions. `now_ms` must not go backwards.
    pub fn observe(&mut self, now_ms: u64, sessions: &[AppSession]) -> Vec<Event> {
        let preset = self.cfg.preset();
        for s in sessions {
            if s.flow == Flow::Render && s.state == SessionState::Active && s.peak > Level::None {
                self.last_sound.insert(s.app.clone(), now_ms);
            }
        }

        // Which calls have a signal in this poll.
        let mut live: BTreeMap<String, (String, Source)> = BTreeMap::new();
        for s in sessions {
            if s.flow == Flow::Capture
                && s.state == SessionState::Active
                && self.cfg.watches(&s.app)
            {
                live.insert(s.app.clone(), (s.app.clone(), Source::Audio));
            }
        }
        if self.cfg.watches_meet() {
            for tab in self.meet_tabs.keys() {
                live.insert(
                    format!("{MEET_APP}:{tab}"),
                    (MEET_APP.to_string(), Source::Meet),
                );
            }
        }

        let mut keys: Vec<String> = live.keys().cloned().collect();
        for k in self.calls.keys() {
            if !live.contains_key(k) {
                keys.push(k.clone());
            }
        }

        let mut events = Vec::new();
        let mut prompted = self.calls.values().any(|c| c.stage == Stage::Prompted);
        for key in keys {
            let signal = live.get(&key);
            let meet_since = key
                .strip_prefix("meet:")
                .and_then(|t| t.parse::<u64>().ok())
                .and_then(|t| self.meet_tabs.get(&t).copied());

            // A rest that is over frees the key for this very poll.
            if let Some(Call {
                stage: Stage::Rest { until },
                ..
            }) = self.calls.get(&key)
            {
                if now_ms >= *until {
                    self.calls.remove(&key);
                } else {
                    continue;
                }
            }

            if !self.calls.contains_key(&key) {
                let Some((app, source)) = signal else {
                    continue;
                };
                // A Meet call counts from the moment the extension reported it.
                let since = meet_since.unwrap_or(now_ms);
                self.calls.insert(
                    key.clone(),
                    Call {
                        app: app.clone(),
                        source: *source,
                        stage: Stage::Candidate { since },
                        quiet_since: None,
                    },
                );
            }

            let recorded = self
                .calls
                .get(&key)
                .is_some_and(|c| self.recorded.contains(&c.app));
            let (app, source, stage, quiet_since) = {
                let c = &self.calls[&key];
                (c.app.clone(), c.source, c.stage, c.quiet_since)
            };
            let end_after = match source {
                Source::Audio => preset.end_of_call_ms,
                Source::Meet => 0,
            };

            // Track the end of the call.
            let quiet_since = if signal.is_some() {
                None
            } else {
                Some(quiet_since.unwrap_or(now_ms))
            };
            let ended = quiet_since.is_some_and(|q| now_ms.saturating_sub(q) >= end_after);

            match stage {
                Stage::Candidate { since } => {
                    if signal.is_none() {
                        self.calls.remove(&key);
                        continue;
                    }
                    let sustain = match source {
                        Source::Audio => preset.sustain_ms,
                        Source::Meet => preset.meet_audible_ms,
                    };
                    let long_enough = now_ms.saturating_sub(since) >= sustain;
                    let render_ok = match source {
                        Source::Audio => self.render_ok(preset.render, &app, now_ms, sessions),
                        Source::Meet => true,
                    };
                    if long_enough && render_ok {
                        if recorded {
                            self.set_stage(&key, Stage::Cooldown { extra_ms: 0 }, quiet_since);
                        } else if !prompted {
                            prompted = true;
                            self.set_stage(&key, Stage::Prompted, quiet_since);
                            events.push(Event::Prompt(Prompt {
                                key: key.clone(),
                                display: display_name(&app),
                                app,
                                source,
                            }));
                        }
                    }
                }
                Stage::Prompted => {
                    if recorded {
                        prompted = false;
                        self.set_stage(&key, Stage::Cooldown { extra_ms: 0 }, quiet_since);
                        events.push(Event::Cleared { key: key.clone() });
                    } else if ended {
                        prompted = false;
                        self.calls.remove(&key);
                        events.push(Event::Cleared { key: key.clone() });
                    } else {
                        self.set_stage(&key, Stage::Prompted, quiet_since);
                    }
                }
                Stage::Cooldown { extra_ms } => {
                    if ended {
                        if extra_ms > 0 {
                            self.set_stage(
                                &key,
                                Stage::Rest {
                                    until: now_ms + extra_ms,
                                },
                                None,
                            );
                        } else {
                            self.calls.remove(&key);
                        }
                    } else {
                        self.set_stage(&key, stage, quiet_since);
                    }
                }
                Stage::Rest { .. } => {}
            }
        }
        events
    }

    fn set_stage(&mut self, key: &str, stage: Stage, quiet_since: Option<u64>) {
        if let Some(c) = self.calls.get_mut(key) {
            c.stage = stage;
            c.quiet_since = quiet_since;
        }
    }
}

fn prompt_of(key: &str, call: &Call) -> Prompt {
    Prompt {
        key: key.to_string(),
        app: call.app.clone(),
        display: display_name(&call.app),
        source: call.source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(app: &str, flow: Flow, state: SessionState, peak: Level) -> AppSession {
        AppSession {
            app: app.into(),
            pid: 1,
            flow,
            state,
            peak,
        }
    }
    fn mic(app: &str) -> AppSession {
        s(app, Flow::Capture, SessionState::Active, Level::Low)
    }
    fn speaker(app: &str, state: SessionState, peak: Level) -> AppSession {
        s(app, Flow::Render, state, peak)
    }
    fn detector(level: Sensitivity) -> Detector {
        Detector::new(DetectConfig {
            sensitivity: level,
            ..DetectConfig::default()
        })
    }
    /// Feed the same sessions once a second for `secs` seconds from `from`; returns the
    /// prompts with their times.
    fn run(d: &mut Detector, from: u64, secs: u64, sessions: &[AppSession]) -> Vec<(u64, Event)> {
        let step = d.poll_ms();
        let mut out = Vec::new();
        let mut t = from;
        while t < from + secs * 1000 {
            for e in d.observe(t, sessions) {
                out.push((t, e));
            }
            t += step;
        }
        out
    }
    fn prompts(events: &[(u64, Event)]) -> Vec<u64> {
        events
            .iter()
            .filter_map(|(t, e)| matches!(e, Event::Prompt(_)).then_some(*t))
            .collect()
    }

    #[test]
    fn the_presets_are_the_documented_numbers() {
        let p = Presets::default();
        let row = |x: Preset| {
            (
                x.sustain_ms / 1000,
                x.render,
                x.meet_audible_ms / 1000,
                x.end_of_call_ms / 1000,
                x.not_now_extra_ms / 60_000,
                x.poll_ms / 1000,
            )
        };
        assert_eq!(
            row(p.low),
            (20, RenderRule::ActiveWithSound, 30, 120, 30, 2)
        );
        assert_eq!(
            row(p.balanced),
            (8, RenderRule::SessionExists, 10, 60, 0, 1)
        );
        assert_eq!(row(p.high), (3, RenderRule::Ignored, 3, 30, 0, 1));
        assert_eq!(DetectConfig::default().sensitivity, Sensitivity::Balanced);
        assert_eq!(DetectConfig::default().preset(), p.balanced);
    }

    #[test]
    fn sensitivity_names_round_trip() {
        for level in Sensitivity::ALL {
            assert_eq!(level.as_str().parse::<Sensitivity>(), Ok(level));
        }
        assert!("loud".parse::<Sensitivity>().is_err());
    }

    #[test]
    fn a_call_is_prompted_after_the_sustain_time_once() {
        let mut d = detector(Sensitivity::Balanced);
        let call = [
            mic("discord"),
            speaker("discord", SessionState::Active, Level::Low),
        ];
        let ev = run(&mut d, 0, 120, &call);
        // Sustain 8 s, poll 1 s: the prompt comes at the poll that reaches 8 s.
        assert_eq!(prompts(&ev), vec![8_000]);
        assert_eq!(d.phase("discord"), Phase::Prompted);
        let Event::Prompt(p) = &ev[0].1 else {
            panic!("first event is the prompt")
        };
        assert_eq!((p.key.as_str(), p.display.as_str()), ("discord", "Discord"));
        assert_eq!(p.source, Source::Audio);
    }

    #[test]
    fn the_microphone_stopping_before_the_sustain_time_resets_the_candidate() {
        let mut d = detector(Sensitivity::Balanced);
        let call = [
            mic("discord"),
            speaker("discord", SessionState::Active, Level::None),
        ];
        let ev = run(&mut d, 0, 7, &call);
        assert!(prompts(&ev).is_empty());
        assert_eq!(d.phase("discord"), Phase::Candidate);
        d.observe(7_000, &[]);
        assert_eq!(d.phase("discord"), Phase::Idle);
        // Starting again begins a new sustain period.
        let ev = run(&mut d, 8_000, 20, &call);
        assert_eq!(prompts(&ev), vec![16_000]);
    }

    #[test]
    fn unknown_applications_never_prompt() {
        let mut d = detector(Sensitivity::High);
        let ev = run(&mut d, 0, 600, &[mic("app-1")]);
        assert!(prompts(&ev).is_empty());
    }

    #[test]
    fn browsers_are_excluded_even_when_listed_as_known() {
        let mut known = DetectConfig::default().known_apps;
        known.push("chrome".into());
        let mut d = Detector::new(DetectConfig {
            known_apps: known,
            sensitivity: Sensitivity::High,
            ..DetectConfig::default()
        });
        let ev = run(&mut d, 0, 600, &[mic("chrome")]);
        assert!(prompts(&ev).is_empty());
    }

    #[test]
    fn the_render_condition_follows_the_sensitivity() {
        // Microphone only: no playback session at all.
        let only_mic = [mic("slack")];
        let ev = run(&mut detector(Sensitivity::High), 0, 60, &only_mic);
        assert_eq!(prompts(&ev), vec![3_000]);
        assert!(prompts(&run(&mut detector(Sensitivity::Balanced), 0, 60, &only_mic)).is_empty());
        assert!(prompts(&run(&mut detector(Sensitivity::Low), 0, 120, &only_mic)).is_empty());

        // A playback session that exists but is idle: balanced is satisfied, low is not.
        let idle_out = [
            mic("slack"),
            speaker("slack", SessionState::Inactive, Level::None),
        ];
        assert_eq!(
            prompts(&run(&mut detector(Sensitivity::Balanced), 0, 60, &idle_out)),
            vec![8_000]
        );
        assert!(prompts(&run(&mut detector(Sensitivity::Low), 0, 120, &idle_out)).is_empty());

        // An active playback session with sound satisfies low too, after its 20 s.
        let talking = [
            mic("slack"),
            speaker("slack", SessionState::Active, Level::High),
        ];
        assert_eq!(
            prompts(&run(&mut detector(Sensitivity::Low), 0, 120, &talking)),
            vec![20_000]
        );
    }

    #[test]
    fn low_waits_for_sound_after_the_sustain_time() {
        let mut d = detector(Sensitivity::Low);
        let quiet = [
            mic("zoom"),
            speaker("zoom", SessionState::Active, Level::None),
        ];
        assert!(prompts(&run(&mut d, 0, 40, &quiet)).is_empty());
        // The other side starts talking at 40 s: the prompt follows at the next poll.
        let loud = [
            mic("zoom"),
            speaker("zoom", SessionState::Active, Level::Low),
        ];
        assert_eq!(prompts(&run(&mut d, 40_000, 10, &loud)), vec![40_000]);
    }

    #[test]
    fn one_prompt_per_call_and_none_again_after_not_now() {
        let mut d = detector(Sensitivity::Balanced);
        let call = [
            mic("ms-teams"),
            speaker("ms-teams", SessionState::Active, Level::Low),
        ];
        assert_eq!(prompts(&run(&mut d, 0, 20, &call)), vec![8_000]);
        d.answer("ms-teams", Answer::NotNow);
        assert_eq!(d.phase("ms-teams"), Phase::Cooldown);
        assert!(prompts(&run(&mut d, 20_000, 3_000, &call)).is_empty());
        // The call ends: after the end-of-call time nothing is left.
        run(&mut d, 3_020_000, 61, &[]);
        assert_eq!(d.phase("ms-teams"), Phase::Idle);
    }

    #[test]
    fn a_short_pause_does_not_end_the_call_but_a_long_one_does() {
        let mut d = detector(Sensitivity::Balanced);
        let call = [
            mic("discord"),
            speaker("discord", SessionState::Inactive, Level::None),
        ];
        assert_eq!(prompts(&run(&mut d, 0, 30, &call)).len(), 1);
        d.answer("discord", Answer::NotNow);
        // 40 s without the microphone: still the same call.
        run(&mut d, 30_000, 40, &[]);
        assert_eq!(d.phase("discord"), Phase::Cooldown);
        assert!(prompts(&run(&mut d, 70_000, 30, &call)).is_empty());
        // 70 s without the microphone ends it; the next use is a new call.
        run(&mut d, 100_000, 70, &[]);
        assert_eq!(d.phase("discord"), Phase::Idle);
        assert_eq!(prompts(&run(&mut d, 170_000, 30, &call)), vec![178_000]);
    }

    #[test]
    fn an_unanswered_prompt_is_withdrawn_when_the_call_ends() {
        let mut d = detector(Sensitivity::High);
        let call = [mic("zoom")];
        assert_eq!(prompts(&run(&mut d, 0, 10, &call)), vec![3_000]);
        let ev = run(&mut d, 10_000, 40, &[]);
        assert_eq!(ev, vec![(40_000, Event::Cleared { key: "zoom".into() })]);
        assert_eq!(d.prompt(), None);
    }

    #[test]
    fn nothing_is_raised_for_an_application_that_is_being_recorded() {
        let mut d = detector(Sensitivity::High);
        d.set_recorded(&["discord".to_string()]);
        let call = [mic("discord")];
        assert!(prompts(&run(&mut d, 0, 60, &call)).is_empty());
        assert_eq!(d.phase("discord"), Phase::Cooldown);
        // Another application is still prompted.
        assert_eq!(
            prompts(&run(&mut d, 60_000, 10, &[mic("zoom")])),
            vec![63_000]
        );
    }

    #[test]
    fn a_prompt_is_withdrawn_when_recording_of_the_application_starts() {
        let mut d = detector(Sensitivity::High);
        let call = [mic("slack")];
        assert_eq!(prompts(&run(&mut d, 0, 5, &call)), vec![3_000]);
        d.set_recorded(&["slack".to_string()]);
        let ev = run(&mut d, 5_000, 2, &call);
        assert_eq!(
            ev,
            vec![(
                5_000,
                Event::Cleared {
                    key: "slack".into()
                }
            )]
        );
        assert_eq!(d.phase("slack"), Phase::Cooldown);
    }

    #[test]
    fn opting_out_stops_prompts_and_clears_a_prompt_on_offer() {
        let mut d = detector(Sensitivity::High);
        let call = [mic("discord")];
        run(&mut d, 0, 5, &call);
        d.answer("discord", Answer::Never);
        assert!(d.config().opted_out.contains(&"discord".to_string()));
        assert!(prompts(&run(&mut d, 5_000, 600, &call)).is_empty());

        // The same through the configuration.
        let mut d = detector(Sensitivity::High);
        run(&mut d, 0, 5, &call);
        let mut cfg = d.config().clone();
        cfg.opted_out.push("discord".into());
        assert_eq!(
            d.set_config(cfg),
            vec![Event::Cleared {
                key: "discord".into()
            }]
        );
        assert_eq!(d.phase("discord"), Phase::Idle);
    }

    #[test]
    fn disabled_detection_raises_nothing() {
        let mut d = Detector::new(DetectConfig {
            enabled: false,
            ..DetectConfig::default()
        });
        assert!(prompts(&run(&mut d, 0, 600, &[mic("discord")])).is_empty());
        d.meet(0, 1, true);
        assert!(prompts(&run(&mut d, 600_000, 600, &[])).is_empty());
    }

    #[test]
    fn low_rests_for_30_minutes_after_not_now_balanced_and_high_do_not() {
        let call = [
            mic("discord"),
            speaker("discord", SessionState::Active, Level::High),
        ];
        for level in Sensitivity::ALL {
            let mut d = detector(level);
            let first = prompts(&run(&mut d, 0, 60, &call));
            assert_eq!(first.len(), 1, "{level}");
            d.answer("discord", Answer::NotNow);
            let end = d.config().preset().end_of_call_ms;
            // The call ends; two minutes after the end the application is used again.
            let gap_start = 60_000;
            run(&mut d, gap_start, end / 1000 + 5, &[]);
            let again = gap_start + end + 5_000 + 120_000;
            let ev = run(&mut d, again, 120, &call);
            let expected = usize::from(level != Sensitivity::Low);
            assert_eq!(prompts(&ev).len(), expected, "{level}");
        }
    }

    #[test]
    fn back_to_back_calls_are_one_call_when_the_gap_is_shorter_than_the_end_time() {
        let call = [
            mic("discord"),
            speaker("discord", SessionState::Active, Level::High),
        ];
        for (level, gap_secs, expected) in [
            (Sensitivity::Low, 90, 1),
            (Sensitivity::Balanced, 90, 2),
            (Sensitivity::High, 90, 2),
            (Sensitivity::High, 20, 1),
        ] {
            let mut d = detector(level);
            let mut n = prompts(&run(&mut d, 0, 300, &call)).len();
            d.answer("discord", Answer::NotNow);
            run(&mut d, 300_000, gap_secs, &[]);
            let ev = run(&mut d, 300_000 + gap_secs * 1000, 300, &call);
            n += prompts(&ev).len();
            assert_eq!(n, expected, "{level} gap {gap_secs}");
        }
    }

    #[test]
    fn meet_is_prompted_after_the_audible_time_and_ends_with_left() {
        for (level, wait) in [
            (Sensitivity::Low, 30_000),
            (Sensitivity::Balanced, 10_000),
            (Sensitivity::High, 3_000),
        ] {
            let mut d = detector(level);
            d.meet(5_000, 7, true);
            let ev = run(&mut d, 5_000, 120, &[]);
            assert_eq!(prompts(&ev).len(), 1, "{level}");
            let at = prompts(&ev)[0];
            assert!(
                at >= 5_000 + wait && at < 5_000 + wait + 2_000,
                "{level} at {at}"
            );
            let Some((_, Event::Prompt(p))) = ev.first() else {
                panic!("a prompt")
            };
            assert_eq!(p.key, "meet:7");
            assert_eq!(p.display, "Google Meet");
            assert_eq!(p.source, Source::Meet);
            d.meet(130_000, 7, false);
            let cleared = d.observe(131_000, &[]);
            assert_eq!(
                cleared,
                vec![Event::Cleared {
                    key: "meet:7".into()
                }]
            );
        }
    }

    #[test]
    fn a_meet_tab_left_early_raises_nothing_and_browser_audio_does_not_count() {
        let mut d = detector(Sensitivity::Balanced);
        d.meet(0, 3, true);
        // The browser has a microphone session as well; it is ignored.
        let sessions = [mic("chrome")];
        assert!(prompts(&run(&mut d, 0, 5, &sessions)).is_empty());
        d.meet(5_000, 3, false);
        assert!(prompts(&run(&mut d, 5_000, 120, &sessions)).is_empty());
        assert_eq!(d.phase("meet:3"), Phase::Idle);
    }

    #[test]
    fn meet_can_be_switched_off_and_a_recorded_tab_is_not_prompted() {
        let mut d = Detector::new(DetectConfig {
            meet: false,
            ..DetectConfig::default()
        });
        d.meet(0, 1, true);
        assert!(prompts(&run(&mut d, 0, 120, &[])).is_empty());

        let mut d = detector(Sensitivity::High);
        d.set_recorded(&[MEET_APP.to_string()]);
        d.meet(0, 1, true);
        assert!(prompts(&run(&mut d, 0, 120, &[])).is_empty());
    }

    #[test]
    fn two_calls_at_once_raise_one_prompt_at_a_time() {
        let mut d = detector(Sensitivity::High);
        let both = [mic("discord"), mic("zoom")];
        let ev = run(&mut d, 0, 10, &both);
        assert_eq!(prompts(&ev).len(), 1);
        let Some(first) = d.prompt() else {
            panic!("a prompt is on offer")
        };
        d.answer(&first.key, Answer::NotNow);
        let ev = run(&mut d, 10_000, 5, &both);
        assert_eq!(prompts(&ev).len(), 1);
        assert_ne!(d.prompt().map(|p| p.key), Some(first.key));
    }

    #[test]
    fn answering_record_keeps_the_call_quiet_until_it_ends() {
        let mut d = detector(Sensitivity::High);
        let call = [mic("slack")];
        run(&mut d, 0, 5, &call);
        d.answer("slack", Answer::Record);
        assert_eq!(d.phase("slack"), Phase::Cooldown);
        assert!(prompts(&run(&mut d, 5_000, 600, &call)).is_empty());
    }

    fn table(pid: u32) -> Option<(String, Option<u32>)> {
        // 10 ms-teams.exe <- 11 msedgewebview2.exe <- 12 msedgewebview2.exe
        // 20 Discord.exe <- 21 Discord.exe; 30 explorer.exe <- 31 tool.exe <- 32 tool.exe
        let rows: [(u32, &str, Option<u32>); 8] = [
            (10, "ms-teams.exe", Some(1)),
            (11, "msedgewebview2.exe", Some(10)),
            (12, "msedgewebview2.exe", Some(11)),
            (20, "Discord.exe", Some(30)),
            (21, "Discord.exe", Some(20)),
            (30, "explorer.exe", None),
            (31, "tool.exe", Some(30)),
            (32, "tool.exe", Some(31)),
        ];
        rows.iter()
            .find(|r| r.0 == pid)
            .map(|r| (r.1.to_string(), r.2))
    }

    fn known() -> Vec<String> {
        DetectConfig::default().known_apps
    }

    #[test]
    fn a_webview_child_belongs_to_the_teams_that_started_it() {
        let r = resolve_app(12, &known(), table).unwrap();
        assert_eq!(
            r,
            ResolvedApp {
                app: "ms-teams".into(),
                top_pid: 10
            }
        );
        // The parent (pid 1) is unknown to the table: the chain ends there.
        assert_eq!(resolve_app(10, &known(), table).unwrap().top_pid, 10);
    }

    #[test]
    fn a_helper_resolves_to_the_topmost_process_of_the_same_program() {
        let r = resolve_app(21, &known(), table).unwrap();
        assert_eq!(
            r,
            ResolvedApp {
                app: "discord".into(),
                top_pid: 20
            }
        );
        // An unknown program stands for itself, represented by its topmost process.
        let r = resolve_app(32, &known(), table).unwrap();
        assert_eq!(
            r,
            ResolvedApp {
                app: "tool".into(),
                top_pid: 31
            }
        );
        assert_eq!(resolve_app(99, &known(), table), None);
    }

    #[test]
    fn a_parent_loop_does_not_hang() {
        let looped = |pid: u32| match pid {
            1 => Some(("a.exe".to_string(), Some(2))),
            2 => Some(("a.exe".to_string(), Some(1))),
            _ => None,
        };
        assert_eq!(resolve_app(1, &known(), looped).unwrap().app, "a");
    }

    #[test]
    fn coarse_levels_and_names() {
        assert_eq!(Level::from_peak(None), Level::None);
        assert_eq!(Level::from_peak(Some(0.0)), Level::None);
        assert_eq!(Level::from_peak(Some(0.01)), Level::Low);
        assert_eq!(Level::from_peak(Some(0.4)), Level::High);
        assert_eq!(app_stem(" Discord.EXE "), "discord");
        assert_eq!(SessionState::parse("Active"), SessionState::Active);
        assert_eq!(SessionState::parse("weird"), SessionState::Inactive);
        assert_eq!(display_name("ms-teams"), "Microsoft Teams");
        assert_eq!(display_name("app-3"), "App-3");
    }
}
