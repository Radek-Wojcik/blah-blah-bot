//! A recording session: one session clock, one journal, and any number of sources that can
//! be added and removed while recording.
//!
//! Capture itself is behind [`CaptureBackend`]. [`WasapiBackend`] talks to the Windows audio
//! stack; [`ToneBackend`] generates sine tones in real time through the same writer and
//! journal, which lets tests, the demo mode and soak runs exercise everything except the
//! audio hardware.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::Serialize;

use crate::capture::{run_track, CaptureContext, SharedState, Source, TrackState, TrackStatus};
use crate::error::{RecorderError, Result};
use crate::journal::{Entry, Journal, JOURNAL_FILE_NAME};
use crate::source_state::{CaptureStatus, EndReason, SourceMachine, SourcePhase, TransitionError};
use crate::sysutil::{qpc_now_hns, unix_ms};
use crate::timeline::{ms_to_samples, segment_samples, SAMPLE_RATE};
use crate::track::TrackWriter;

/// Length of a segment in a real recording, in seconds.
pub const DEFAULT_SEGMENT_SECONDS: u32 = 60;

/// Errors specific to managing sources of a session.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("source `{0}` is already being recorded")]
    SourceAlreadyAdded(String),
    #[error("source `{0}` is not part of this recording")]
    SourceNotFound(String),
    #[error(transparent)]
    Transition(#[from] TransitionError),
    #[error(transparent)]
    Recorder(#[from] RecorderError),
}

/// What can be recorded.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SourceSpec {
    /// The default microphone.
    Microphone,
    /// One application (with its child processes).
    Application { pid: u32 },
    /// A generated sine tone, for tests and demos.
    Tone { hz: u32 },
}

impl SourceSpec {
    /// Parse an id as used by the UI: `mic:default`, `app:<pid>` or `tone:<hz>`.
    pub fn parse(id: &str) -> Result<Self> {
        let bad = || RecorderError::InvalidArgument(format!("unknown source id `{id}`"));
        let (kind, rest) = id.split_once(':').ok_or_else(bad)?;
        match kind {
            "mic" => Ok(Self::Microphone),
            "app" => Ok(Self::Application {
                pid: rest.parse().map_err(|_| bad())?,
            }),
            "tone" => Ok(Self::Tone {
                hz: rest.parse().map_err(|_| bad())?,
            }),
            _ => Err(bad()),
        }
    }

    /// The id the UI uses for this source.
    pub fn id(&self) -> String {
        match self {
            Self::Microphone => "mic:default".into(),
            Self::Application { pid } => format!("app:{pid}"),
            Self::Tone { hz } => format!("tone:{hz}"),
        }
    }

    fn base_track_name(&self) -> String {
        match self {
            Self::Microphone => "mic".into(),
            Self::Application { pid } => format!("pid-{pid}"),
            Self::Tone { hz } => format!("tone-{hz}"),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Microphone => "mic",
            Self::Application { .. } => "process-loopback",
            Self::Tone { .. } => "tone",
        }
    }
}

/// Runs the capture loop of one track on the calling thread until `ctx.stop` is set.
pub trait CaptureBackend: Send + Sync + 'static {
    fn run(&self, spec: SourceSpec, track: String, ctx: Arc<CaptureContext>, state: SharedState);
}

/// Real capture through WASAPI.
pub struct WasapiBackend;

impl CaptureBackend for WasapiBackend {
    fn run(&self, spec: SourceSpec, track: String, ctx: Arc<CaptureContext>, state: SharedState) {
        let source = match spec {
            SourceSpec::Microphone => Source::Mic(None),
            SourceSpec::Application { pid } => Source::Process { pid, tree: true },
            SourceSpec::Tone { .. } => {
                if let Ok(mut s) = state.lock() {
                    s.status = TrackStatus::Failed;
                    s.error = Some("tone sources need the tone backend".into());
                }
                return;
            }
        };
        run_track(source, track, ctx, state);
    }
}

/// Synthetic capture: a sine tone paced by the session clock.
pub struct ToneBackend;

impl ToneBackend {
    fn run_inner(
        spec: &SourceSpec,
        track: &str,
        ctx: &Arc<CaptureContext>,
        state: &SharedState,
    ) -> Result<()> {
        let SourceSpec::Tone { hz } = spec else {
            return Err(RecorderError::InvalidArgument(
                "the tone backend only plays tone sources".into(),
            ));
        };
        let offset_ms = ctx.now_ms();
        ctx.journal
            .lock()
            .map_err(|_| RecorderError::LockPoisoned("journal"))?
            .append(&Entry::TrackStart {
                track: track.to_string(),
                source: spec.kind().to_string(),
                pid: None,
                include_tree: None,
                offset_ms,
            })?;
        if let Ok(mut s) = state.lock() {
            s.status = TrackStatus::Running;
            s.offset_ms = offset_ms;
        }
        let mut writer = TrackWriter::new(
            track,
            &ctx.out_dir,
            ctx.journal.clone(),
            offset_ms,
            SAMPLE_RATE,
            ctx.seg_samples,
        )?;
        let step = 2.0 * std::f64::consts::PI * f64::from(*hz) / f64::from(SAMPLE_RATE);
        let mut produced: u64 = 0;
        let mut buf: Vec<i16> = Vec::new();
        while !ctx.stop.load(Ordering::Relaxed) {
            let due = ms_to_samples(ctx.now_ms() - offset_ms, SAMPLE_RATE);
            if due > produced {
                buf.clear();
                for n in produced..due {
                    buf.push((f64::sin(step * n as f64) * 9_000.0) as i16);
                }
                produced = due;
                ctx.note_peak(&buf);
                writer.push(&buf)?;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let total = writer.finish()?;
        ctx.journal
            .lock()
            .map_err(|_| RecorderError::LockPoisoned("journal"))?
            .append(&Entry::TrackEnd {
                track: track.to_string(),
                total_samples: total,
                end_offset_ms: crate::timeline::sample_position_ms(offset_ms, total, SAMPLE_RATE),
            })?;
        if let Ok(mut s) = state.lock() {
            s.stored_samples = total;
            s.status = TrackStatus::Stopped;
        }
        Ok(())
    }
}

impl CaptureBackend for ToneBackend {
    fn run(&self, spec: SourceSpec, track: String, ctx: Arc<CaptureContext>, state: SharedState) {
        if let Err(e) = Self::run_inner(&spec, &track, &ctx, &state) {
            tracing::error!(track = %track, error = %e, "tone track failed");
            if let Ok(mut s) = state.lock() {
                s.status = TrackStatus::Failed;
                s.error = Some(e.to_string());
            }
        }
    }
}

struct ActiveSource {
    spec: SourceSpec,
    label: String,
    track: String,
    ctx: Arc<CaptureContext>,
    state: SharedState,
    handle: Option<JoinHandle<()>>,
    machine: SourceMachine,
    level: f32,
}

impl ActiveSource {
    fn observe(&mut self) {
        let status = match self.state.lock() {
            Ok(s) => s.status,
            Err(_) => TrackStatus::Failed,
        };
        self.machine.observe(match status {
            TrackStatus::Starting => CaptureStatus::Starting,
            TrackStatus::Running => CaptureStatus::Running,
            TrackStatus::Stopped => CaptureStatus::Stopped,
            TrackStatus::Failed => CaptureStatus::Failed,
        });
        // A thread that is gone without having reported a final status counts as failed.
        if self.machine.is_active() && self.handle.as_ref().is_some_and(JoinHandle::is_finished) {
            self.machine.observe(CaptureStatus::Failed);
        }
    }

    fn stop_and_join(&mut self) {
        self.ctx.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            if h.join().is_err() {
                tracing::error!(track = %self.track, "capture thread panicked");
                if let Ok(mut s) = self.state.lock() {
                    s.status = TrackStatus::Failed;
                    s.error = Some("capture thread panicked".into());
                }
            }
        }
        self.observe();
    }
}

/// How a source looks from the outside.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SourceSnapshot {
    pub id: String,
    pub label: String,
    pub track: String,
    pub phase: &'static str,
    /// Start on the session clock.
    pub offset_ms: f64,
    /// Peak level since the previous snapshot, 0 to 1.
    pub level: f32,
    /// Explanation for an ended or failed source.
    pub detail: Option<String>,
}

/// The whole session at one moment.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SessionSnapshot {
    pub elapsed_ms: f64,
    pub sources: Vec<SourceSnapshot>,
}

fn phase_name(p: SourcePhase) -> &'static str {
    match p {
        SourcePhase::Starting => "starting",
        SourcePhase::Recording => "recording",
        SourcePhase::Ended(_) => "ended",
        SourcePhase::Failed => "failed",
    }
}

fn end_detail(p: SourcePhase, error: Option<String>) -> Option<String> {
    match p {
        SourcePhase::Ended(EndReason::Removed) => Some("Removed".into()),
        SourcePhase::Ended(EndReason::SessionStopped) => Some("Recording stopped".into()),
        SourcePhase::Ended(EndReason::SourceClosed) => Some("Source closed".into()),
        SourcePhase::Failed => Some(error.unwrap_or_else(|| "Capture failed".into())),
        _ => None,
    }
}

/// A recording in progress.
pub struct Session {
    backend: Arc<dyn CaptureBackend>,
    dir: PathBuf,
    journal: Arc<Mutex<Journal>>,
    clock_start_hns: u64,
    seg_samples: u64,
    started_unix_ms: u64,
    sources: Vec<ActiveSource>,
    used_names: HashSet<String>,
    next_screenshot: u32,
}

impl Session {
    /// Create the recording folder and journal and start the session clock.
    pub fn start(
        dir: &Path,
        segment_seconds: u32,
        backend: Arc<dyn CaptureBackend>,
    ) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let mut journal = Journal::create(&dir.join(JOURNAL_FILE_NAME))?;
        let seg_samples = segment_samples(segment_seconds, SAMPLE_RATE);
        let started_unix_ms = unix_ms();
        journal.append(&Entry::SessionStart {
            started_unix_ms,
            sample_rate: SAMPLE_RATE,
            segment_samples: seg_samples,
            format: "pcm_s16le_mono_16k".into(),
        })?;
        Ok(Self {
            backend,
            dir: dir.to_path_buf(),
            journal: Arc::new(Mutex::new(journal)),
            clock_start_hns: qpc_now_hns(),
            seg_samples,
            started_unix_ms,
            sources: Vec::new(),
            used_names: HashSet::new(),
            next_screenshot: 1,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn started_unix_ms(&self) -> u64 {
        self.started_unix_ms
    }

    /// Milliseconds on the session clock.
    pub fn now_ms(&self) -> f64 {
        crate::timeline::hns_to_ms(qpc_now_hns().saturating_sub(self.clock_start_hns))
    }

    fn unique_track_name(&mut self, spec: &SourceSpec) -> String {
        let base = spec.base_track_name();
        let mut name = base.clone();
        let mut n = 2;
        while self.used_names.contains(&name) {
            name = format!("{base}-{n}");
            n += 1;
        }
        self.used_names.insert(name.clone());
        name
    }

    /// Start recording a source. Its track starts at the current session time.
    pub fn add_source(
        &mut self,
        spec: SourceSpec,
        label: &str,
    ) -> std::result::Result<String, SessionError> {
        self.refresh();
        if self
            .sources
            .iter()
            .any(|s| s.spec == spec && s.machine.is_active())
        {
            return Err(SessionError::SourceAlreadyAdded(spec.id()));
        }
        let track = self.unique_track_name(&spec);
        let ctx = Arc::new(CaptureContext {
            peak: AtomicU32::new(0),
            clock_start_hns: self.clock_start_hns,
            stop: AtomicBool::new(false),
            journal: self.journal.clone(),
            out_dir: self.dir.clone(),
            seg_samples: self.seg_samples,
            drift_correction: true,
        });
        let state: SharedState = Arc::new(Mutex::new(TrackState::default()));
        let backend = self.backend.clone();
        let handle = {
            let (spec, track, ctx, state) =
                (spec.clone(), track.clone(), ctx.clone(), state.clone());
            std::thread::Builder::new()
                .name(format!("capture-{track}"))
                .spawn(move || backend.run(spec, track, ctx, state))
                .map_err(RecorderError::from)?
        };
        self.sources.push(ActiveSource {
            spec,
            label: label.to_string(),
            track: track.clone(),
            ctx,
            state,
            handle: Some(handle),
            machine: SourceMachine::new(),
            level: 0.0,
        });
        Ok(track)
    }

    /// Stop recording one source; the others continue.
    pub fn remove_source(&mut self, id: &str) -> std::result::Result<(), SessionError> {
        self.refresh();
        let src = self
            .sources
            .iter_mut()
            .find(|s| s.spec.id() == id && s.machine.is_active())
            .ok_or_else(|| SessionError::SourceNotFound(id.to_string()))?;
        src.machine.request_remove()?;
        src.stop_and_join();
        Ok(())
    }

    fn refresh(&mut self) {
        for s in &mut self.sources {
            s.observe();
        }
    }

    /// Journal a screenshot that was taken at `offset_ms` on the session clock. Returns its
    /// file name relative to the recording folder; the caller has written, or writes, the
    /// image to that name.
    pub fn log_screenshot(&mut self, offset_ms: f64) -> Result<String> {
        let name = crate::screenshot::screenshot_file_name(self.next_screenshot, offset_ms as u64);
        let file = format!("img/{name}");
        self.journal
            .lock()
            .map_err(|_| RecorderError::LockPoisoned("journal"))?
            .append(&Entry::Screenshot {
                file: file.clone(),
                offset_ms,
            })?;
        self.next_screenshot += 1;
        Ok(file)
    }

    /// Journal a screenshot taken now. Returns its file name and offset.
    pub fn next_screenshot(&mut self) -> Result<(String, f64)> {
        let offset_ms = self.now_ms();
        Ok((self.log_screenshot(offset_ms)?, offset_ms))
    }

    /// File name the next screenshot will get for a given offset, without journaling it.
    pub fn peek_screenshot_name(&self, offset_ms: f64) -> String {
        format!(
            "img/{}",
            crate::screenshot::screenshot_file_name(self.next_screenshot, offset_ms as u64)
        )
    }

    /// Current state of every source. Reading resets the level meters.
    pub fn snapshot(&mut self) -> SessionSnapshot {
        self.refresh();
        let elapsed_ms = self.now_ms();
        let sources = self
            .sources
            .iter_mut()
            .map(|s| {
                let peak = s.ctx.take_peak();
                // Fall off smoothly instead of jumping to zero between packets.
                s.level = if peak >= s.level { peak } else { s.level * 0.7 };
                let (offset_ms, error) = s
                    .state
                    .lock()
                    .map(|st| (st.offset_ms, st.error.clone()))
                    .unwrap_or((0.0, None));
                let phase = s.machine.phase();
                SourceSnapshot {
                    id: s.spec.id(),
                    label: s.label.clone(),
                    track: s.track.clone(),
                    phase: phase_name(phase),
                    offset_ms,
                    level: if s.machine.is_active() { s.level } else { 0.0 },
                    detail: end_detail(phase, error),
                }
            })
            .collect();
        SessionSnapshot {
            elapsed_ms,
            sources,
        }
    }

    /// Stop every source, close the journal and return the length of the recording in
    /// milliseconds.
    pub fn stop(mut self) -> Result<f64> {
        for s in &mut self.sources {
            s.machine.request_session_stop();
        }
        for s in &mut self.sources {
            s.stop_and_join();
        }
        let duration_ms = self.now_ms();
        self.journal
            .lock()
            .map_err(|_| RecorderError::LockPoisoned("journal"))?
            .append(&Entry::SessionEnd { duration_ms })?;
        Ok(duration_ms)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Dropped without `stop` (for example on an error path): ask the threads to finish.
        // No `session_end` is written, so the recording stays recoverable.
        for s in &self.sources {
            s.ctx.stop.store(true, Ordering::Relaxed);
        }
    }
}
