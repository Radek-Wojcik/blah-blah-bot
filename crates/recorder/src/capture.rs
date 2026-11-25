//! WASAPI capture threads: microphone, system loopback and per-process loopback.
//!
//! [`run_track`] is the body of one capture thread. The caller (the session layer) spawns one
//! thread per source, shares a [`CaptureContext`] between them and reads progress from the
//! [`SharedState`] of each track.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use wasapi::{
    initialize_mta, AudioCaptureClient, AudioClient, DeviceEnumerator, Direction, SampleType,
    StreamMode, WasapiError, WaveFormat,
};

use crate::convert::{Converter, Format};
use crate::drift::Corrector;
use crate::error::{RecorderError, Result};
use crate::journal::{Entry, Journal};
use crate::mic_tuning::{MicTuning, MmcssGuard};
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

/// Number of waveform bars in the live audio envelope.
pub const LEVEL_BARS: usize = 12;

/// Fast attack factor when incoming energy exceeds the current envelope.
const ENVELOPE_ATTACK: f32 = 0.85;
/// Smooth decay factor when incoming energy is below the current envelope.
const ENVELOPE_DECAY: f32 = 0.65;
/// Values below this floor snap to `0.0`.
const ENVELOPE_FLOOR: f32 = 0.0005;

/// Real-time audio levels and 12-bar waveform envelope, normalized to `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AudioLevels {
    pub peak: f32,
    pub rms: f32,
    pub bars: [f32; LEVEL_BARS],
}

impl Default for AudioLevels {
    fn default() -> Self {
        Self {
            peak: 0.0,
            rms: 0.0,
            bars: [0.0; LEVEL_BARS],
        }
    }
}

impl AudioLevels {
    /// Combine two level readings by taking the element-wise maximum, clamped to `0.0..=1.0`.
    pub fn merge(self, other: Self) -> Self {
        let mut bars = [0.0; LEVEL_BARS];
        for (i, b) in bars.iter_mut().enumerate() {
            *b = self.bars[i].max(other.bars[i]).clamp(0.0, 1.0);
        }
        Self {
            peak: self.peak.max(other.peak).clamp(0.0, 1.0),
            rms: self.rms.max(other.rms).clamp(0.0, 1.0),
            bars,
        }
    }
}

fn is_demo_env() -> bool {
    static DEMO: OnceLock<bool> = OnceLock::new();
    *DEMO.get_or_init(|| std::env::var_os("BBB_DEMO").is_some())
}

#[inline]
fn load_f32(atom: &AtomicU32) -> f32 {
    let v = f32::from_bits(atom.load(Ordering::Relaxed));
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[inline]
fn store_f32(atom: &AtomicU32, val: f32) {
    let clamped = if val.is_finite() && val >= ENVELOPE_FLOOR {
        val.clamp(0.0, 1.0)
    } else {
        0.0
    };
    atom.store(clamped.to_bits(), Ordering::Relaxed);
}

#[inline]
fn smooth_step(prev: f32, target: f32) -> f32 {
    let next = if target >= prev {
        if prev <= ENVELOPE_FLOOR {
            target
        } else {
            prev + (target - prev) * ENVELOPE_ATTACK
        }
    } else {
        prev * ENVELOPE_DECAY + target * (1.0 - ENVELOPE_DECAY)
    };
    if next < ENVELOPE_FLOOR {
        0.0
    } else {
        next.clamp(0.0, 1.0)
    }
}

/// Generate a voice-like modulated 12-bar envelope from a demo tone block.
pub fn compute_demo_bars(base_level: f32, seq: u32) -> [f32; LEVEL_BARS] {
    if base_level <= 0.0 {
        return [0.0; LEVEL_BARS];
    }
    let t = seq as f32 * 0.14;
    let syllable = 0.55 + 0.45 * ((t * 0.75).sin() * (t * 0.27).cos()).abs();
    let gain = (base_level * 2.2 * syllable).clamp(0.05, 1.0);
    let mut bars = [0.0; LEVEL_BARS];
    for (i, bar) in bars.iter_mut().enumerate() {
        let pos = i as f32 / (LEVEL_BARS - 1) as f32;
        // Arch shape (stronger in midrange vocal formants) plus travelling harmonic ripple.
        let arch = 0.45 + 0.55 * (std::f32::consts::PI * pos).sin();
        let ripple = 0.65 + 0.35 * (t * 1.35 + i as f32 * 0.62).sin();
        *bar = (gain * arch * ripple).clamp(0.02, 1.0);
    }
    bars
}

/// Data shared by all capture threads of a recording.
pub struct CaptureContext {
    /// Largest absolute sample (0 to 32768) seen since the last [`CaptureContext::take_peak`].
    pub peak: AtomicU32,
    /// Smoothed normalized peak (`0.0..=1.0`, stored as `f32::to_bits`).
    pub envelope_peak: AtomicU32,
    /// Smoothed normalized RMS (`0.0..=1.0`, stored as `f32::to_bits`).
    pub rms: AtomicU32,
    /// Smoothed normalized 12-bar waveform envelope (`0.0..=1.0`, stored as `f32::to_bits`).
    pub bars: [AtomicU32; LEVEL_BARS],
    /// Monotonic packet counter used for demo envelope modulation.
    pub packet_seq: AtomicU32,
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
    /// Optional microphone-only capture tuning (default: none, device defaults).
    pub mic_tuning: MicTuning,
}

impl CaptureContext {
    /// Create a zeroed 12-bar atomic array.
    pub fn empty_bars() -> [AtomicU32; LEVEL_BARS] {
        std::array::from_fn(|_| AtomicU32::new(0))
    }

    /// Create a fresh [`CaptureContext`] for one track.
    pub fn new(
        clock_start_hns: u64,
        journal: Arc<Mutex<Journal>>,
        out_dir: PathBuf,
        seg_samples: u64,
        drift_correction: bool,
    ) -> Self {
        Self {
            peak: AtomicU32::new(0),
            envelope_peak: AtomicU32::new(0),
            rms: AtomicU32::new(0),
            bars: Self::empty_bars(),
            packet_seq: AtomicU32::new(0),
            clock_start_hns,
            stop: AtomicBool::new(false),
            journal,
            out_dir,
            seg_samples,
            drift_correction,
            mic_tuning: MicTuning::from_env(),
        }
    }

    /// Read and reset the peak level, scaled to 0.0 to 1.0.
    pub fn take_peak(&self) -> f32 {
        self.peak.swap(0, Ordering::Relaxed) as f32 / 32768.0
    }

    /// Record a block of PCM samples, updating [`CaptureContext::peak`], [`CaptureContext::rms`]
    /// and the 12-bar waveform envelope.
    pub fn note_samples(&self, samples: &[i16]) {
        self.note_samples_inner(samples, is_demo_env());
    }

    /// Record a block of PCM samples with an explicit demo-mode flag.
    pub fn note_samples_with_demo(&self, samples: &[i16], demo: bool) {
        self.note_samples_inner(samples, demo);
    }

    fn note_samples_inner(&self, samples: &[i16], demo: bool) {
        if samples.is_empty() {
            return;
        }
        let mut max_abs = 0u32;
        let mut sum_sq = 0u64;
        for &s in samples {
            let a = u32::from(s.unsigned_abs());
            if a > max_abs {
                max_abs = a;
            }
            sum_sq += u64::from(a) * u64::from(a);
        }
        self.peak.fetch_max(max_abs, Ordering::Relaxed);

        let len = samples.len();
        let block_peak = (max_abs as f32 / 32768.0).clamp(0.0, 1.0);
        let block_rms = (((sum_sq as f64 / len as f64).sqrt() / 32768.0) as f32).clamp(0.0, 1.0);

        let seq = self.packet_seq.fetch_add(1, Ordering::Relaxed);
        let target_bars = if max_abs == 0 {
            [0.0; LEVEL_BARS]
        } else if demo {
            compute_demo_bars(block_rms.max(block_peak * 0.7), seq)
        } else {
            let mut out = [0.0f32; LEVEL_BARS];
            for (i, bar) in out.iter_mut().enumerate() {
                let start = (i * len / LEVEL_BARS).min(len - 1);
                let end = (((i + 1) * len) / LEVEL_BARS).max(start + 1).min(len);
                let slice = &samples[start..end];
                let mut s_max = 0u32;
                let mut s_sq = 0u64;
                for &v in slice {
                    let a = u32::from(v.unsigned_abs());
                    if a > s_max {
                        s_max = a;
                    }
                    s_sq += u64::from(a) * u64::from(a);
                }
                let s_peak = s_max as f32 / 32768.0;
                let s_rms = ((s_sq as f64 / slice.len() as f64).sqrt() / 32768.0) as f32;
                *bar = (s_rms * 0.65 + s_peak * 0.35).clamp(0.0, 1.0);
            }
            out
        };

        store_f32(
            &self.envelope_peak,
            smooth_step(load_f32(&self.envelope_peak), block_peak),
        );
        store_f32(&self.rms, smooth_step(load_f32(&self.rms), block_rms));
        for (i, &target) in target_bars.iter().enumerate() {
            let prev = load_f32(&self.bars[i]);
            store_f32(&self.bars[i], smooth_step(prev, target));
        }
    }

    /// Record the peak and 12-bar envelope of a block of samples.
    pub fn note_peak(&self, samples: &[i16]) {
        self.note_samples(samples);
    }

    /// Read the current lock-free 12-bar audio envelope without modifying it.
    pub fn levels(&self) -> AudioLevels {
        let mut bars = [0.0; LEVEL_BARS];
        for (i, b) in bars.iter_mut().enumerate() {
            *b = load_f32(&self.bars[i]);
        }
        AudioLevels {
            peak: load_f32(&self.envelope_peak),
            rms: load_f32(&self.rms),
            bars,
        }
    }

    /// Read the current 12-bar audio envelope and apply one tick of decay so an idle stream
    /// smoothly falls off to zero between polls.
    pub fn take_levels(&self) -> AudioLevels {
        let current = self.levels();
        store_f32(&self.envelope_peak, current.peak * 0.7);
        store_f32(&self.rms, current.rms * 0.7);
        for (i, &b) in current.bars.iter().enumerate() {
            store_f32(&self.bars[i], b * 0.7);
        }
        current
    }

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
    try_format_tuned(source, format, &MicTuning::default())
}

/// [`try_format`] with the event buffer duration of `tuning` (microphone sources only; every
/// other source ignores the tuning).
pub fn try_format_tuned(
    source: &Source,
    format: Format,
    tuning: &MicTuning,
) -> Result<AudioClient> {
    let buffer_duration_hns = if matches!(source, Source::Mic(_)) {
        tuning.buffer_hns()
    } else {
        0
    };
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
            buffer_duration_hns,
        };
        client.initialize_client(&wave_format(format), Direction::Capture, mode, None)?;
        Ok(client)
    })();
    attempt.map_err(|e| RecorderError::audio("open audio client", e))
}

/// Create a client, trying the formats in order and logging every attempt.
///
/// A tuned buffer duration that the device refuses is logged as a warning and the same
/// format is opened again with the default buffer.
fn open_client(
    source: &Source,
    tuning: &MicTuning,
    log: &mut Vec<String>,
) -> Result<(AudioClient, Format)> {
    let mut last_err = RecorderError::audio("open audio client", "no format tried");
    let wants_buffer = matches!(source, Source::Mic(_)) && tuning.buffer_ms.is_some();
    for format in [Format::Pcm16Mono16k, Format::Float32Stereo48k] {
        let mut note = String::new();
        let mut attempt = try_format_tuned(source, format, tuning);
        if wants_buffer {
            match &attempt {
                Ok(_) => note = format!(" (buffer {} ms)", tuning.buffer_ms.unwrap_or(0)),
                Err(e) => {
                    tracing::warn!(error = %e, "tuned microphone buffer refused, using the default buffer");
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
pub fn run_track(source: Source, name: String, shared: Arc<CaptureContext>, state: SharedState) {
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
    // Experiment: microphone-only tuning; every failure falls back to the default behaviour.
    let tuning = shared.mic_tuning;
    let _mmcss = (matches!(source, Source::Mic(_)) && tuning.mmcss)
        .then(MmcssGuard::enter)
        .flatten();
    if _mmcss.is_some() {
        init_log.push("mmcss: Pro Audio".to_string());
    }
    if tuning.is_active() && matches!(source, Source::Mic(_)) {
        tracing::info!(track = name, tuning = %tuning.describe(), "microphone tuning requested");
    }
    let opened = open_client(source, &tuning, &mut init_log);
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
            shared.note_peak(&corrected);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::JOURNAL_FILE_NAME;

    fn test_context(label: &str) -> (test_tmp::TestDir, CaptureContext) {
        let dir = test_tmp::TestDir::new(&format!("capture-ctx-{label}"));
        let journal = Journal::create(&dir.path().join(JOURNAL_FILE_NAME)).unwrap();
        let ctx = CaptureContext::new(
            0,
            Arc::new(Mutex::new(journal)),
            dir.path().to_path_buf(),
            16_000,
            false,
        );
        (dir, ctx)
    }

    #[test]
    fn silence_produces_zero_audio_levels_and_bars() {
        let (_dir, ctx) = test_context("silence");
        let silence = vec![0i16; 320];
        ctx.note_samples_with_demo(&silence, false);
        let levels = ctx.levels();
        assert_eq!(levels.peak, 0.0);
        assert_eq!(levels.rms, 0.0);
        assert_eq!(levels.bars, [0.0; LEVEL_BARS]);

        // Even with demo mode enabled, pure silence must produce 0.0 across all bars.
        ctx.note_samples_with_demo(&silence, true);
        let demo_levels = ctx.levels();
        assert_eq!(demo_levels.peak, 0.0);
        assert_eq!(demo_levels.rms, 0.0);
        assert_eq!(demo_levels.bars, [0.0; LEVEL_BARS]);
    }

    #[test]
    fn non_silent_pcm_populates_peak_rms_and_twelve_normalized_bars() {
        let (_dir, ctx) = test_context("pcm");
        let samples: Vec<i16> = (0..360)
            .map(|i| {
                let phase = i as f32 * 0.22;
                let env = 0.4 + 0.6 * ((i as f32 / 360.0) * std::f32::consts::PI).sin();
                (phase.sin() * env * 16_384.0) as i16
            })
            .collect();
        ctx.note_samples_with_demo(&samples, false);
        let levels = ctx.levels();
        assert!(
            levels.peak > 0.3 && levels.peak <= 1.0,
            "unexpected peak {}",
            levels.peak
        );
        assert!(
            levels.rms > 0.1 && levels.rms <= 1.0,
            "unexpected rms {}",
            levels.rms
        );
        assert!(levels.rms <= levels.peak);
        for (i, &bar) in levels.bars.iter().enumerate() {
            assert!(
                bar > 0.0 && bar <= 1.0,
                "bar {i} out of range (0.0..=1.0]: {bar}"
            );
        }
    }

    #[test]
    fn demo_envelope_modulates_steady_tone_into_varied_normalized_bars() {
        let (_dir, ctx) = test_context("demo");
        let steady_tone: Vec<i16> = (0..320)
            .map(|i| ((i as f32 * 0.17).sin() * 9_000.0) as i16)
            .collect();
        ctx.note_samples_with_demo(&steady_tone, true);
        let levels = ctx.levels();
        assert!(levels.peak > 0.0 && levels.peak <= 1.0);
        assert!(levels.rms > 0.0 && levels.rms <= 1.0);
        for (i, &bar) in levels.bars.iter().enumerate() {
            assert!(bar > 0.0 && bar <= 1.0, "demo bar {i} out of range: {bar}");
        }
        let min_bar = levels.bars.iter().copied().fold(f32::INFINITY, f32::min);
        let max_bar = levels
            .bars
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(
            max_bar - min_bar > 0.02,
            "expected voice-like variation across demo bars: {:?}",
            levels.bars
        );
    }
}
