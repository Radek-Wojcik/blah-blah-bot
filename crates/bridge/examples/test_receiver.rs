//! A stand-alone bridge for testing the extension without the application.
//!
//! It runs the real server and hub, accepts every tab stream as if a recording were running
//! and prints one JSON object per line on stdout (`listening`, `pairing_code`, `paired`,
//! `stream_started`, `stream_ended`, `summary`), so scripts can follow it. With `--out-dir`
//! every stream is also written as raw 16 kHz mono s16le to `<stream_id>.pcm`.
//!
//! With `--record-dir` every stream also goes through the recorder's `PushTrack` (as in the
//! application) into `<DIR>/<stream_id>/`; `stream_ended` and `summary` then include what the
//! recorder stored and journaled (`recorder`).
//!
//! ```text
//! cargo run -p bridge --example test_receiver -- [--port N] [--code XXXXXXXX]
//!     [--extension-id ID]... [--duration SECONDS] [--out-dir DIR] [--record-dir DIR]
//!     [--clients-file FILE] [--idle]
//! ```
//!
//! `--port 0` picks a free port (printed in `listening`); `--port` may be repeated to try
//! several in order. `--code` fixes the pairing code, `--idle` refuses streams like an
//! application that is not recording. `--clients-file` keeps the paired clients in a JSON file
//! across runs (an application that restarts). Without `--duration` it runs until the
//! process is stopped.
//!
//! Commands on stdin, one per line: `drop` closes every connection (a network failure),
//! `record` and `idle` switch the recording state.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bridge::hub::{FeedEvent, HubObserver, TabStream};
use bridge::pairing::PairedClient;
use bridge::protocol::RefuseReason;
use bridge::{Hub, HubConfig, Server, ServerConfig};
use recorder::journal::{replay_file, DiscKind, Journal};
use recorder::push::{PushFrame, PushTrack};
use serde_json::json;

/// Samples per second on the wire (16 kHz mono).
const RATE: f64 = 16_000.0;

/// What the recorder stored for a stream (with `--record-dir`).
#[derive(Default, Clone, serde::Serialize)]
struct RecorderStats {
    stored_samples: u64,
    stored_s: f64,
    frames: u64,
    missing_frames: u64,
    stale_frames: u64,
    stream_gap_events: usize,
    silence_filled_events: usize,
    correction_ppm: f64,
}

/// Arrival statistics of a stream. `max_gap_ms` is the longest time between two frames,
/// `max_stall_ms` the part of it beyond the audio the earlier frame carried (a burst after a
/// stall does not hide the stall), `max_sender_gap_ms` the same measured on the sender clock,
/// `drift_ms` the wall time between the first and the last arrival minus the audio before the
/// last frame (it includes the burst of frames that waited for the start answer),
/// `sender_drift_ms` the same on the sender clock: how far the browser's audio clock runs
/// behind or ahead of its wall clock (negative: audio runs slower than the wall clock).
#[derive(Default, Clone, serde::Serialize)]
struct StreamStats {
    frames: u64,
    samples: u64,
    lost_frames: u64,
    late_frames: u64,
    max_gap_ms: f64,
    max_stall_ms: f64,
    max_sender_gap_ms: f64,
    wall_ms: f64,
    audio_ms: f64,
    drift_ms: f64,
    sender_drift_ms: f64,
    end: Option<String>,
    recorder: Option<RecorderStats>,
}

/// The recorder side of one stream: the same `PushTrack` the application uses.
struct RecorderSink {
    track: PushTrack,
    dir: PathBuf,
}

impl RecorderSink {
    fn open(root: &Path, stream: &str, now_ms: f64) -> Option<Self> {
        let dir = root.join(stream);
        std::fs::create_dir_all(&dir).ok()?;
        let journal = Arc::new(Mutex::new(
            Journal::create(&dir.join("journal.jsonl")).ok()?,
        ));
        let track = PushTrack::start("tab", &dir, journal, now_ms, 60 * 16_000, true).ok()?;
        Some(Self { track, dir })
    }

    fn finish(self, now_ms: f64) -> Option<RecorderStats> {
        let (pushed, correction_ppm) = (self.track.stats.clone(), self.track.correction_ppm());
        let stored = self.track.finish(now_ms).ok()?;
        let replay = replay_file(&self.dir.join("journal.jsonl")).ok()?;
        let events = |kind: DiscKind| {
            replay
                .tracks
                .values()
                .flat_map(|t| t.discontinuities.iter())
                .filter(|d| d.kind == kind)
                .count()
        };
        Some(RecorderStats {
            stored_samples: stored,
            stored_s: stored as f64 / RATE,
            frames: pushed.frames,
            missing_frames: pushed.missing_frames,
            stale_frames: pushed.stale_frames,
            stream_gap_events: events(DiscKind::StreamGap),
            silence_filled_events: events(DiscKind::SilenceFilled),
            correction_ppm,
        })
    }
}

/// Where the frames of a stream go besides the statistics.
struct Sinks {
    out_dir: Option<PathBuf>,
    record_dir: Option<PathBuf>,
    /// Zero of the session clock handed to the recorder.
    started: Instant,
}

fn ms_since(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

struct Receiver {
    hub: Mutex<Option<Arc<Hub>>>,
    out_dir: Option<PathBuf>,
    record_dir: Option<PathBuf>,
    clients_file: Option<PathBuf>,
    started: Instant,
    stats: Arc<Mutex<BTreeMap<String, StreamStats>>>,
    next_track: Mutex<u32>,
}

fn emit(value: serde_json::Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

impl HubObserver for Receiver {
    fn clients_changed(&self, clients: &[PairedClient]) {
        if let Some(file) = &self.clients_file {
            if let Ok(text) = serde_json::to_string_pretty(clients) {
                let _ = std::fs::write(file, text);
            }
        }
        if let Some(c) = clients.last() {
            emit(
                json!({"event": "paired", "client_id": c.id, "client": c.name, "ext_version": c.ext_version}),
            );
        }
    }

    fn status_changed(&self) {}

    fn add_tab(&self, s: &TabStream) -> Result<String, RefuseReason> {
        let hub = self
            .hub
            .lock()
            .ok()
            .and_then(|h| h.clone())
            .ok_or(RefuseReason::Declined)?;
        let track = {
            let mut n = self.next_track.lock().map_err(|_| RefuseReason::Declined)?;
            *n += 1;
            format!("tab-{n}")
        };
        emit(
            json!({"event": "stream_started", "stream_id": s.stream_id, "tab_id": s.tab_id, "host": s.host, "title": s.title, "track": track}),
        );
        let sinks = Sinks {
            out_dir: self.out_dir.clone(),
            record_dir: self.record_dir.clone(),
            started: self.started,
        };
        let (id, stats) = (s.stream_id.clone(), self.stats.clone());
        // The hub accepts the stream after this returns; take its feed from another thread.
        std::thread::spawn(move || consume(&hub, &id, &stats, &sinks));
        Ok(track)
    }

    fn meet(&self, m: &bridge::protocol::Meet) {
        emit(json!({"event": "meet", "tab_id": m.tab_id, "host": m.host, "state": m.state}));
    }

    fn group(&self, g: &bridge::protocol::Group) {
        emit(json!({
            "event": "group",
            "group_id": g.group_id,
            "tabs": g.tabs.len(),
            "list": g.tabs,
        }));
    }
}

fn consume(hub: &Hub, id: &str, stats: &Mutex<BTreeMap<String, StreamStats>>, sinks: &Sinks) {
    let mut feed = None;
    for _ in 0..100 {
        if let Ok(f) = hub.attach(id) {
            feed = Some(f);
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let Some(mut feed) = feed else { return };
    let mut file = sinks.out_dir.as_ref().and_then(|d| {
        std::fs::create_dir_all(d).ok()?;
        std::fs::File::create(d.join(format!("{id}.pcm"))).ok()
    });
    let mut recorder = sinks
        .record_dir
        .as_ref()
        .and_then(|d| RecorderSink::open(d, id, ms_since(sinks.started)));
    let mut next_seq: Option<u32> = None;
    let mut last_arrival: Option<Instant> = None;
    let mut first_arrival: Option<Instant> = None;
    let mut last_len = 0usize;
    let mut last_sent: Option<f64> = None;
    let mut first_sent: Option<f64> = None;
    loop {
        match feed.next(Duration::from_millis(200)) {
            FeedEvent::Frame(f) => {
                let now = Instant::now();
                if let Some(r) = recorder.as_mut() {
                    let frame = PushFrame {
                        seq: f.header.seq,
                        sent_unix_ms: f.header.sent_ms,
                        samples: f.samples.clone(),
                    };
                    let _ = r.track.push(&frame, ms_since(sinks.started));
                }
                let mut st = stats.lock().unwrap_or_else(|e| e.into_inner());
                let s = st.entry(id.to_string()).or_default();
                s.frames += 1;
                s.samples += f.samples.len() as u64;
                match next_seq {
                    Some(n) if f.header.seq > n => s.lost_frames += u64::from(f.header.seq - n),
                    Some(n) if f.header.seq < n => s.late_frames += 1,
                    _ => {}
                }
                next_seq = Some(f.header.seq.wrapping_add(1).max(next_seq.unwrap_or(0)));
                if let Some(prev) = last_arrival {
                    let gap = now.duration_since(prev).as_secs_f64() * 1000.0;
                    s.max_gap_ms = s.max_gap_ms.max(gap);
                    s.max_stall_ms = s.max_stall_ms.max(gap - last_len as f64 * 1000.0 / RATE);
                }
                if let Some(prev) = last_sent {
                    s.max_sender_gap_ms = s.max_sender_gap_ms.max(f.header.sent_ms - prev);
                }
                last_sent = Some(f.header.sent_ms);
                first_sent.get_or_insert(f.header.sent_ms);
                last_len = f.samples.len();
                first_arrival.get_or_insert(now);
                last_arrival = Some(now);
                if let Some(file) = file.as_mut() {
                    let bytes: Vec<u8> = f.samples.iter().flat_map(|v| v.to_le_bytes()).collect();
                    let _ = file.write_all(&bytes);
                }
            }
            FeedEvent::Idle => {}
            FeedEvent::End(reason) => {
                let stored = recorder
                    .take()
                    .and_then(|r| r.finish(ms_since(sinks.started)));
                let mut st = stats.lock().unwrap_or_else(|e| e.into_inner());
                let s = st.entry(id.to_string()).or_default();
                s.end = Some(reason.as_str().to_string());
                s.recorder = stored;
                if let (Some(a), Some(b)) = (first_arrival, last_arrival) {
                    s.wall_ms = b.duration_since(a).as_secs_f64() * 1000.0;
                    s.audio_ms = (s.samples as f64 - last_len as f64) * 1000.0 / RATE;
                    s.drift_ms = s.wall_ms - s.audio_ms;
                }
                if let (Some(a), Some(b)) = (first_sent, last_sent) {
                    s.sender_drift_ms = (b - a) - s.audio_ms;
                }
                emit(
                    json!({"event": "stream_ended", "stream_id": id, "reason": reason.as_str(), "stats": s.clone()}),
                );
                return;
            }
        }
    }
}

struct Args {
    ports: Vec<u16>,
    clients_file: Option<PathBuf>,
    code: Option<String>,
    ids: Vec<String>,
    duration: Option<Duration>,
    out_dir: Option<PathBuf>,
    record_dir: Option<PathBuf>,
    idle: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        ports: Vec::new(),
        clients_file: None,
        code: None,
        ids: Vec::new(),
        duration: None,
        out_dir: None,
        record_dir: None,
        idle: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--port" => a.ports.push(value()?.parse().map_err(|_| "bad --port")?),
            "--clients-file" => a.clients_file = Some(PathBuf::from(value()?)),
            "--code" => a.code = Some(value()?),
            "--extension-id" => a.ids.push(value()?),
            "--duration" => {
                a.duration = Some(Duration::from_secs_f64(
                    value()?.parse().map_err(|_| "bad --duration")?,
                ))
            }
            "--out-dir" => a.out_dir = Some(PathBuf::from(value()?)),
            "--record-dir" => a.record_dir = Some(PathBuf::from(value()?)),
            "--idle" => a.idle = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if a.ports.is_empty() {
        a.ports.push(bridge::server::DEFAULT_PORT);
    }
    Ok(a)
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let receiver = Arc::new(Receiver {
        hub: Mutex::new(None),
        out_dir: args.out_dir.clone(),
        record_dir: args.record_dir.clone(),
        clients_file: args.clients_file.clone(),
        started: Instant::now(),
        stats: Arc::new(Mutex::new(BTreeMap::new())),
        next_track: Mutex::new(0),
    });
    let stored_clients: Vec<PairedClient> = args
        .clients_file
        .as_ref()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let hub = Hub::new(
        HubConfig {
            app_version: format!("test-receiver {}", env!("CARGO_PKG_VERSION")),
            ..HubConfig::default()
        },
        stored_clients,
        receiver.clone(),
    );
    if let Ok(mut h) = receiver.hub.lock() {
        *h = Some(hub.clone());
    }
    let ports = args.ports.clone();
    let server = match Server::start(
        ServerConfig {
            ports,
            allowed_extension_ids: args.ids.clone(),
            ..ServerConfig::default()
        },
        hub.clone(),
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot listen: {e}");
            std::process::exit(1);
        }
    };
    emit(json!({"event": "listening", "port": server.port()}));
    let now = Instant::now();
    let code = match &args.code {
        Some(c) => match bridge::pairing::normalize_code(c) {
            Some(c) => {
                hub.start_pairing_with(&c, now);
                c
            }
            None => {
                eprintln!("--code must be 8 characters of Crockford base32");
                std::process::exit(2);
            }
        },
        None => match hub.start_pairing(now) {
            Ok((c, _)) => c,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        },
    };
    emit(json!({"event": "pairing_code", "code": code}));
    if !args.idle {
        hub.set_recording(Some("test-receiver".into()));
    }
    let commands = hub.clone();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            match line.trim() {
                "drop" => commands.close_all("connection dropped by the test receiver"),
                "record" => commands.set_recording(Some("test-receiver".into())),
                "idle" => commands.set_recording(None),
                _ => {}
            }
        }
    });
    let started = Instant::now();
    while args.duration.is_none_or(|d| started.elapsed() < d) {
        std::thread::sleep(Duration::from_millis(100));
    }
    hub.set_recording(None);
    std::thread::sleep(Duration::from_millis(300));
    let stats = receiver.stats.lock().map(|s| s.clone()).unwrap_or_default();
    emit(json!({"event": "summary", "streams": stats}));
    server.shutdown();
}
