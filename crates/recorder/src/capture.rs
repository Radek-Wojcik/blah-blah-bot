//! WASAPI capture threads: microphone, system loopback and per-process loopback.
//!
//! [`run_track`] is the body of one capture thread. The caller (the session layer) spawns one
//! thread per source, shares a [`CaptureContext`] between them and reads progress from the
//! [`SharedState`] of each track.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use wasapi::{
    initialize_mta, AudioCaptureClient, AudioClient, DeviceEnumerator, Direction, SampleType,
    StreamMode, WasapiError, WaveFormat,
};

use crate::convert::{Converter, Format};
use crate::drift::Corrector;
use crate::error::{RecorderError, Result};
use crate::journal::{Entry, Journal};
use crate::sysutil::qpc_now_hns;
use crate::timeline::{hns_to_ms, SAMPLE_RATE};
use crate::track::TrackWriter;
use crate::tracker::{PacketFlags, Plan, Stats, Tracker};

/// Timestamp shortfall below which a packet is treated as jitter, not as a gap, in ms.
const GAP_TOLERANCE_MS: f64 = 30.0;

/// Where a track gets its audio from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Capture device whose name contains the text, or the default capture device.
    Mic(Option<String>),
    /// Everything played on the default render device (reference track).
    System,
    /// One process with its child processes (`tree`), or everything except them.
    Process { pid: u32, tree: bool },
}

impl Source {
    /// Parse `PID`, `PID:tree` or `PID:exclude`.
    ///
    /// The Windows API knows two modes only: capture the target process together with its
    /// child processes, or capture everything except them. There is no mode for the
    /// target process alone, so `PID` and `PID:tree` both mean "target and children";
    /// `PID:exclude` selects the opposite mode (everything except the tree).
    pub fn parse_pid(text: &str) -> Result<Source> {
        let (pid_text, tree) = match text.split_once(':') {
            Some((p, "tree")) => (p, true),
            Some((p, "exclude")) => (p, false),
            Some((_, other)) => {
                return Err(RecorderError::InvalidArgument(format!(
                    "unknown pid option `{other}`, use `:tree` or `:exclude`"
                )))
            }
            None => (text, true),
        };
        let pid = pid_text.parse::<u32>().map_err(|_| {
            RecorderError::InvalidArgument(format!("`{pid_text}` is not a process id"))
        })?;
        Ok(Source::Process { pid, tree })
    }

    /// Track name, also the directory name.
    pub fn track_name(&self) -> String {
        match self {
            Source::Mic(_) => "mic".into(),
            Source::System => "system".into(),
            Source::Process { pid, tree: true } => format!("pid-{pid}"),
            Source::Process { pid, tree: false } => format!("pid-{pid}-excl"),
        }
    }

    /// Kind of source as written to the journal.
    pub fn kind(&self) -> &'static str {
        match self {
            Source::Mic(_) => "mic",
            Source::System => "system-loopback",
            Source::Process { .. } => "process-loopback",
        }
    }
}

/// Data shared by all capture threads of a recording.
pub struct CaptureContext {
    /// Performance counter value (100 ns units) that is time 0 of the session clock.
    pub clock_start_hns: u64,
    /// Set to true to ask every capture thread to finish.
    pub stop: AtomicBool,
    /// Journal that receives the entries of every track.
    pub journal: Arc<Mutex<Journal>>,
    /// Recording directory; every track gets a sub-directory.
    pub out_dir: PathBuf,
    /// Samples in a full segment.
    pub seg_samples: u64,
    /// Correct clock drift of sources with real device timestamps. Switch off to store
    /// sources as captured (for comparison runs).
    pub drift_correction: bool,
}

impl CaptureContext {
    /// Milliseconds on the session clock now.
    pub fn now_ms(&self) -> f64 {
        hns_to_ms(qpc_now_hns().saturating_sub(self.clock_start_hns))
    }

    /// Convert a WASAPI packet timestamp (100 ns, performance counter base).
    pub fn ts_to_ms(&self, hns: u64) -> f64 {
        (hns as f64 - self.clock_start_hns as f64) / 10_000.0
    }
}

/// Lifecycle of a capture thread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackStatus {
    /// The thread was created but the stream is not running yet.
    #[default]
    Starting,
    /// Audio is being captured.
    Running,
    /// The thread finished normally.
    Stopped,
    /// The thread ended with an error, see [`TrackState::error`].
    Failed,
}

/// Live state of one track, read by the controlling thread.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TrackState {
    /// Lifecycle of the capture thread.
    pub status: TrackStatus,
    /// Error message when `status` is [`TrackStatus::Failed`].
    pub error: Option<String>,
    /// One line per audio format that was tried while opening the source.
    pub init_log: Vec<String>,
    /// Label of the format in use.
    pub format: Option<String>,
    /// Start of the track on the session clock, in ms.
    pub offset_ms: f64,
    /// Samples stored so far (updated every few seconds).
    pub stored_samples: u64,
    /// Packet counters (updated every few seconds).
    pub stats: Stats,
    /// Drift correction applied to the stream in ppm (None when not used).
    pub drift_correction_ppm: Option<f64>,
}

/// State of one track shared between its capture thread and the controller.
pub type SharedState = Arc<Mutex<TrackState>>;

fn set_state(state: &SharedState, f: impl FnOnce(&mut TrackState)) {
    if let Ok(mut s) = state.lock() {
        f(&mut s);
    }
}

fn wave_format(format: Format) -> WaveFormat {
    match format {
        Format::Pcm16Mono16k => WaveFormat::new(16, 16, SampleType::Int, 16_000, 1, None),
        Format::Float32Stereo48k => WaveFormat::new(32, 32, SampleType::Float, 48_000, 2, None),
    }
}

/// Create and initialize a client for the source in one specific format.
///
/// Can also be used to probe whether a source can be opened at all.
pub fn try_format(source: &Source, format: Format) -> Result<AudioClient> {
    let attempt: std::result::Result<AudioClient, WasapiError> = (|| {
        let mut client = match source {
            Source::Process { pid, tree } => {
                AudioClient::new_application_loopback_client(*pid, *tree)?
            }
            Source::Mic(None) => DeviceEnumerator::new()?
                .get_default_device(Direction::Capture)?
                .get_iaudioclient()?,
            Source::Mic(Some(name)) => {
                let wanted = name.to_lowercase();
                let devices = DeviceEnumerator::new()?.get_device_collection(Direction::Capture)?;
                let mut found = None;
                for d in &devices {
                    let d = d?;
                    if d.get_friendlyname()
                        .is_ok_and(|n| n.to_lowercase().contains(&wanted))
                    {
                        found = Some(d);
                        break;
                    }
                }
                found
                    .ok_or_else(|| WasapiError::DeviceNotFound(name.clone()))?
                    .get_iaudioclient()?
            }
            Source::System => DeviceEnumerator::new()?
                .get_default_device(Direction::Render)?
                .get_iaudioclient()?,
        };
        let mode = StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: 0,
        };
        client.initialize_client(&wave_format(format), Direction::Capture, mode, None)?;
        Ok(client)
    })();
    attempt.map_err(|e| RecorderError::audio("open audio client", e))
}

/// Create a client, trying the formats in order and logging every attempt.
fn open_client(source: &Source, log: &mut Vec<String>) -> Result<(AudioClient, Format)> {
    let mut last_err = RecorderError::audio("open audio client", "no format tried");
    for format in [Format::Pcm16Mono16k, Format::Float32Stereo48k] {
        match try_format(source, format) {
            Ok(client) => {
                log.push(format!("{}: ok", format.label()));
                return Ok((client, format));
            }
            Err(e) => {
                log.push(format!("{}: {}", format.label(), e));
                last_err = e;
            }
        }
    }
    Err(last_err)
}

fn apply_plan(
    plan: &Plan,
    writer: &mut TrackWriter,
    shared: &CaptureContext,
    name: &str,
    position_ms: f64,
) -> Result<()> {
    for (kind, frames) in &plan.events {
        if kind.is_loss() {
            tracing::warn!(track = name, ?kind, position_ms, "audio lost");
        } else {
            tracing::debug!(track = name, ?kind, frames, position_ms, "silence inserted");
        }
        shared
            .journal
            .lock()
            .map_err(|_| RecorderError::LockPoisoned("journal"))?
            .append(&Entry::Discontinuity {
                track: name.to_string(),
                offset_ms: position_ms,
                kind: *kind,
                frames: *frames,
            })?;
    }
    if plan.fill > 0 {
        writer.push_silence(plan.fill)?;
    }
    Ok(())
}

/// Thread body for one track: opens the source, captures until `shared.stop` is set (or an
/// error occurs) and finishes the track. Progress and failure are reported through `state`;
/// nothing is returned because the thread has no caller to hand an error to.
pub fn run_track(source: Source, shared: Arc<CaptureContext>, state: SharedState) {
    let name = source.track_name();
    tracing::info!(track = %name, "capture thread started");
    match run_track_inner(&source, &shared, &state, &name) {
        Ok(()) => tracing::info!(track = %name, "capture thread finished"),
        Err(e) => {
            tracing::error!(track = %name, error = %e, "capture thread failed");
            set_state(&state, |s| {
                s.status = TrackStatus::Failed;
                s.error = Some(e.to_string());
            });
        }
    }
}

fn run_track_inner(
    source: &Source,
    shared: &Arc<CaptureContext>,
    state: &SharedState,
    name: &str,
) -> Result<()> {
    // COM must be initialized on every thread that talks to WASAPI.
    initialize_mta()
        .ok()
        .map_err(|e| RecorderError::audio("COM initialization", e))?;

    let mut init_log = Vec::new();
    let opened = open_client(source, &mut init_log);
    set_state(state, |s| s.init_log = init_log.clone());
    let (client, format) = opened?;
    let capture: AudioCaptureClient = client
        .get_audiocaptureclient()
        .map_err(|e| RecorderError::audio("capture client", e))?;
    let event = client
        .set_get_eventhandle()
        .map_err(|e| RecorderError::audio("event handle", e))?;

    client
        .start_stream()
        .map_err(|e| RecorderError::audio("start stream", e))?;
    let offset_ms = shared.now_ms();
    shared
        .journal
        .lock()
        .map_err(|_| RecorderError::LockPoisoned("journal"))?
        .append(&Entry::TrackStart {
            track: name.to_string(),
            source: source.kind().to_string(),
            pid: match source {
                Source::Process { pid, .. } => Some(*pid),
                _ => None,
            },
            include_tree: match source {
                Source::Process { tree, .. } => Some(*tree),
                _ => None,
            },
            offset_ms,
        })?;
    set_state(state, |s| {
        s.status = TrackStatus::Running;
        s.format = Some(format.label().to_string());
        s.offset_ms = offset_ms;
    });

    let mut writer = TrackWriter::new(
        name,
        &shared.out_dir,
        shared.journal.clone(),
        offset_ms,
        SAMPLE_RATE,
        shared.seg_samples,
    )?;
    let mut tracker = Tracker::new(SAMPLE_RATE, offset_ms, GAP_TOLERANCE_MS);
    let mut converter = Converter::new(format);

    let frame_bytes = format.frame_bytes();
    // One second of audio is far more than a packet; the buffer is reused.
    let mut buf = vec![0u8; frame_bytes * 48_000];
    let mut samples: Vec<i16> = Vec::with_capacity(48_000);
    let mut corrected: Vec<i16> = Vec::with_capacity(48_000);
    // Process loopback derives its timestamps from the sample count, so only sources with
    // real device timestamps can be corrected.
    let drift_correction = !matches!(source, Source::Process { .. }) && shared.drift_correction;
    let mut corrector = Corrector::new(drift_correction);
    let mut last_publish = std::time::Instant::now();

    'outer: while !shared.stop.load(Ordering::Relaxed) {
        if last_publish.elapsed().as_secs() >= 5 {
            last_publish = std::time::Instant::now();
            let stats = tracker.stats.clone();
            let stored = writer.written();
            let ppm = drift_correction.then(|| corrector.correction_ppm());
            set_state(state, |s| {
                s.stats = stats;
                s.stored_samples = stored;
                s.drift_correction_ppm = ppm;
            });
        }
        match event.wait_for_event(200) {
            Ok(()) => {}
            Err(WasapiError::EventTimeout) => continue,
            Err(e) => return Err(RecorderError::audio("wait for audio event", e)),
        }
        loop {
            let packet_frames = capture
                .get_next_packet_size()
                .map_err(|e| RecorderError::audio("packet size", e))?
                .unwrap_or(0);
            if packet_frames == 0 {
                break;
            }
            let (frames, info) = capture
                .read_from_device(&mut buf)
                .map_err(|e| RecorderError::audio("read packet", e))?;
            if frames == 0 {
                break;
            }
            let flags = PacketFlags {
                discontinuity: info.flags.data_discontinuity,
                silent: info.flags.silent,
                timestamp_error: info.flags.timestamp_error,
            };
            // Frames in the converted (16 kHz) domain; the converter may hold back a
            // fraction of a frame, so the count is taken from its output.
            samples.clear();
            converter.convert(&buf[..frames as usize * frame_bytes], &mut samples);
            let due = if flags.timestamp_error || info.timestamp == 0 {
                None
            } else {
                Some(shared.ts_to_ms(info.timestamp))
            };
            corrected.clear();
            corrector.process(due, &samples, &mut corrected);
            let plan = tracker.on_packet(due, corrected.len() as u64, shared.now_ms(), flags);
            apply_plan(&plan, &mut writer, shared, name, tracker.position_ms())?;
            writer.push(&corrected)?;
            if shared.stop.load(Ordering::Relaxed) {
                break 'outer;
            }
        }
    }

    if let Err(e) = client.stop_stream() {
        tracing::debug!(track = name, error = %e, "stop stream failed");
    }
    let plan = tracker.on_stop(shared.now_ms());
    apply_plan(&plan, &mut writer, shared, name, tracker.position_ms())?;
    let total = writer.finish()?;
    shared
        .journal
        .lock()
        .map_err(|_| RecorderError::LockPoisoned("journal"))?
        .append(&Entry::TrackEnd {
            track: name.to_string(),
            total_samples: total,
            end_offset_ms: crate::timeline::sample_position_ms(offset_ms, total, SAMPLE_RATE),
        })?;
    let stats = tracker.stats.clone();
    let ppm = drift_correction.then(|| corrector.correction_ppm());
    set_state(state, |s| {
        s.stats = stats;
        s.stored_samples = total;
        s.drift_correction_ppm = ppm;
        s.status = TrackStatus::Stopped;
    });
    Ok(())
}
