//! WASAPI capture threads: microphone, system loopback and per-process loopback.

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
use crate::journal::{Entry, Journal};
use crate::mic_tuning::{MicTuning, MmcssGuard};
use crate::sysutil::qpc_now_hns;
use crate::timeline::{hns_to_ms, SAMPLE_RATE};
use crate::track::TrackWriter;
use crate::tracker::{PacketFlags, Plan, Stats, Tracker};

/// Where a track gets its audio from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Default capture device.
    /// Capture device whose name contains the text, or the default one.
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
    pub fn parse_pid(text: &str) -> Result<Source, String> {
        let (pid_text, tree) = match text.split_once(':') {
            Some((p, "tree")) => (p, true),
            Some((p, "exclude")) => (p, false),
            Some((_, other)) => {
                return Err(format!(
                    "unknown pid option `{other}`, use `:tree` or `:exclude`"
                ))
            }
            None => (text, true),
        };
        let pid = pid_text
            .parse::<u32>()
            .map_err(|_| format!("`{pid_text}` is not a process id"))?;
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

    pub fn kind(&self) -> &'static str {
        match self {
            Source::Mic(_) => "mic",
            Source::System => "system-loopback",
            Source::Process { .. } => "process-loopback",
        }
    }
}

/// Data shared by all capture threads.
pub struct Shared {
    pub clock_start_hns: u64,
    pub stop: AtomicBool,
    pub journal: Arc<Mutex<Journal>>,
    pub out_dir: PathBuf,
    pub seg_samples: u64,
    /// Store sources as captured, without drift correction (for comparison runs).
    pub no_drift_correction: bool,
    /// Optional microphone-only capture tuning (default: none).
    pub mic_tuning: MicTuning,
}

impl Shared {
    /// Milliseconds on the session clock now.
    pub fn now_ms(&self) -> f64 {
        hns_to_ms(qpc_now_hns().saturating_sub(self.clock_start_hns))
    }

    /// Convert a WASAPI packet timestamp (100 ns, performance counter base).
    pub fn ts_to_ms(&self, hns: u64) -> f64 {
        (hns as f64 - self.clock_start_hns as f64) / 10_000.0
    }
}

/// Live state of one track, read by the main thread for the summary.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TrackState {
    pub state: String,
    pub error: Option<String>,
    pub init_log: Vec<String>,
    pub format: Option<String>,
    pub offset_ms: f64,
    pub stored_samples: u64,
    pub stats: Stats,
    /// Drift correction applied to the stream in ppm (None when not used).
    pub drift_correction_ppm: Option<f64>,
}

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

fn describe(e: &WasapiError) -> String {
    format!("{e}")
}

/// Create and initialize a client for the source in one specific format.
pub fn try_format(source: &Source, format: Format) -> Result<AudioClient, String> {
    try_format_tuned(source, format, &MicTuning::default())
}

/// [`try_format`] with the event buffer duration of `tuning` (microphone sources only).
pub fn try_format_tuned(
    source: &Source,
    format: Format,
    tuning: &MicTuning,
) -> Result<AudioClient, String> {
    let buffer_duration_hns = if matches!(source, Source::Mic(_)) {
        tuning.buffer_hns()
    } else {
        0
    };
    let attempt: Result<AudioClient, WasapiError> = (|| {
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
            buffer_duration_hns,
        };
        client.initialize_client(&wave_format(format), Direction::Capture, mode, None)?;
        Ok(client)
    })();
    attempt.map_err(|e| describe(&e))
}

/// Create a client, trying the formats in order and logging every attempt.
fn open_client(
    source: &Source,
    tuning: &MicTuning,
    log: &mut Vec<String>,
) -> Result<(AudioClient, Format), String> {
    let mut last_err = String::from("no format tried");
    let wants_buffer = matches!(source, Source::Mic(_)) && tuning.buffer_ms.is_some();
    for format in [Format::Pcm16Mono16k, Format::Float32Stereo48k] {
        let mut note = String::new();
        let mut attempt = try_format_tuned(source, format, tuning);
        if wants_buffer {
            match &attempt {
                Ok(_) => note = format!(" (buffer {} ms)", tuning.buffer_ms.unwrap_or(0)),
                Err(e) => {
                    eprintln!(
                        "warning: tuned microphone buffer refused ({e}), using the default buffer"
                    );
                    log.push(format!("{}: tuned buffer failed: {e}", format.label()));
                    attempt = try_format(source, format);
                }
            }
        }
        match attempt {
            Ok(client) => {
                log.push(format!("{}: ok{note}", format.label()));
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
    shared: &Shared,
    name: &str,
    position_ms: f64,
) -> std::io::Result<()> {
    for (kind, frames) in &plan.events {
        shared
            .journal
            .lock()
            .map_err(|_| std::io::Error::other("journal lock poisoned"))?
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

/// Thread body for one track.
pub fn run_track(source: Source, shared: Arc<Shared>, state: SharedState) {
    let name = source.track_name();
    let result = run_track_inner(&source, &shared, &state, &name);
    if let Err(e) = result {
        set_state(&state, |s| {
            s.state = "failed".into();
            s.error = Some(e);
        });
    }
}

fn run_track_inner(
    source: &Source,
    shared: &Arc<Shared>,
    state: &SharedState,
    name: &str,
) -> Result<(), String> {
    // COM must be initialized on every thread that talks to WASAPI.
    initialize_mta()
        .ok()
        .map_err(|e| format!("COM init: {e}"))?;

    let mut init_log = Vec::new();
    // Experiment: microphone-only tuning; every failure falls back to the default behaviour.
    let tuning = shared.mic_tuning;
    let _mmcss = (matches!(source, Source::Mic(_)) && tuning.mmcss)
        .then(MmcssGuard::enter)
        .flatten();
    if _mmcss.is_some() {
        init_log.push("mmcss: Pro Audio".to_string());
    }
    let opened = open_client(source, &tuning, &mut init_log);
    set_state(state, |s| s.init_log = init_log.clone());
    let (client, format) = opened?;
    let capture: AudioCaptureClient = client
        .get_audiocaptureclient()
        .map_err(|e| format!("capture client: {e}"))?;
    let event = client
        .set_get_eventhandle()
        .map_err(|e| format!("event handle: {e}"))?;

    client.start_stream().map_err(|e| format!("start: {e}"))?;
    let offset_ms = shared.now_ms();
    shared
        .journal
        .lock()
        .map_err(|_| "journal lock poisoned".to_string())?
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
        })
        .map_err(|e| format!("journal: {e}"))?;
    set_state(state, |s| {
        s.state = "running".into();
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
    )
    .map_err(|e| format!("track writer: {e}"))?;
    let mut tracker = Tracker::new(SAMPLE_RATE, offset_ms, 30.0);
    let mut converter = Converter::new(format);

    let frame_bytes = format.frame_bytes();
    // One second of audio is far more than a packet; the buffer is reused.
    let mut buf = vec![0u8; frame_bytes * 48_000];
    let mut samples: Vec<i16> = Vec::with_capacity(48_000);
    let mut corrected: Vec<i16> = Vec::with_capacity(48_000);
    // Process loopback derives its timestamps from the sample count, so only sources with
    // real device timestamps can be corrected.
    let drift_correction = !matches!(source, Source::Process { .. }) && !shared.no_drift_correction;
    let mut corrector = Corrector::new(drift_correction);
    let mut last_publish = std::time::Instant::now();

    let io_err = |e: std::io::Error| format!("io: {e}");

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
            Err(e) => return Err(format!("wait: {e}")),
        }
        loop {
            let packet_frames = capture
                .get_next_packet_size()
                .map_err(|e| format!("packet size: {e}"))?
                .unwrap_or(0);
            if packet_frames == 0 {
                break;
            }
            let (frames, info) = capture
                .read_from_device(&mut buf)
                .map_err(|e| format!("read: {e}"))?;
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
            apply_plan(&plan, &mut writer, shared, name, tracker.position_ms()).map_err(io_err)?;
            writer.push(&corrected).map_err(io_err)?;
            if shared.stop.load(Ordering::Relaxed) {
                break 'outer;
            }
        }
    }

    let _ = client.stop_stream();
    let plan = tracker.on_stop(shared.now_ms());
    apply_plan(&plan, &mut writer, shared, name, tracker.position_ms()).map_err(io_err)?;
    let total = writer.finish().map_err(io_err)?;
    shared
        .journal
        .lock()
        .map_err(|_| "journal lock poisoned".to_string())?
        .append(&Entry::TrackEnd {
            track: name.to_string(),
            total_samples: total,
            end_offset_ms: crate::timeline::sample_position_ms(offset_ms, total, SAMPLE_RATE),
        })
        .map_err(|e| format!("journal: {e}"))?;
    let stats = tracker.stats.clone();
    let ppm = drift_correction.then(|| corrector.correction_ppm());
    set_state(state, |s| {
        s.stats = stats;
        s.stored_samples = total;
        s.drift_correction_ppm = ppm;
        s.state = "stopped".into();
    });
    Ok(())
}
