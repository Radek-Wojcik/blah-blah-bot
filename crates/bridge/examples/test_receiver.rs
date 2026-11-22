//! A stand-alone bridge for testing the extension without the application.
//!
//! It runs the real server and hub, accepts every tab stream as if a recording were running
//! and prints one JSON object per line on stdout (`listening`, `pairing_code`, `paired`,
//! `stream_started`, `stream_ended`, `summary`), so scripts can follow it. With `--out-dir`
//! every stream is also written as raw 16 kHz mono s16le to `<stream_id>.pcm`.
//!
//! ```text
//! cargo run -p bridge --example test_receiver -- [--port N] [--code XXXXXXXX]
//!     [--extension-id ID]... [--duration SECONDS] [--out-dir DIR] [--idle]
//! ```
//!
//! `--port 0` picks a free port (printed in `listening`), `--code` fixes the pairing code,
//! `--idle` refuses streams like an application that is not recording. Without `--duration`
//! it runs until the process is stopped.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bridge::hub::{FeedEvent, HubObserver, TabStream};
use bridge::pairing::PairedClient;
use bridge::protocol::RefuseReason;
use bridge::{Hub, HubConfig, Server, ServerConfig};
use serde_json::json;

#[derive(Default, Clone, serde::Serialize)]
struct StreamStats {
    frames: u64,
    samples: u64,
    lost_frames: u64,
    late_frames: u64,
    max_gap_ms: f64,
    end: Option<String>,
}

struct Receiver {
    hub: Mutex<Option<Arc<Hub>>>,
    out_dir: Option<PathBuf>,
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
        let (id, stats, out_dir) = (
            s.stream_id.clone(),
            self.stats.clone(),
            self.out_dir.clone(),
        );
        // The hub accepts the stream after this returns; take its feed from another thread.
        std::thread::spawn(move || consume(&hub, &id, &stats, out_dir));
        Ok(track)
    }

    fn meet(&self, m: &bridge::protocol::Meet) {
        emit(json!({"event": "meet", "tab_id": m.tab_id, "host": m.host, "state": m.state}));
    }

    fn group(&self, g: &bridge::protocol::Group) {
        emit(json!({"event": "group", "group_id": g.group_id, "tabs": g.tabs.len()}));
    }
}

fn consume(
    hub: &Hub,
    id: &str,
    stats: &Mutex<BTreeMap<String, StreamStats>>,
    out_dir: Option<PathBuf>,
) {
    let mut feed = None;
    for _ in 0..100 {
        if let Ok(f) = hub.attach(id) {
            feed = Some(f);
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let Some(mut feed) = feed else { return };
    let mut file = out_dir.and_then(|d| {
        std::fs::create_dir_all(&d).ok()?;
        std::fs::File::create(d.join(format!("{id}.pcm"))).ok()
    });
    let mut next_seq: Option<u32> = None;
    let mut last_arrival: Option<Instant> = None;
    loop {
        match feed.next(Duration::from_millis(200)) {
            FeedEvent::Frame(f) => {
                let now = Instant::now();
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
                    s.max_gap_ms = s
                        .max_gap_ms
                        .max(now.duration_since(prev).as_secs_f64() * 1000.0);
                }
                last_arrival = Some(now);
                if let Some(file) = file.as_mut() {
                    let bytes: Vec<u8> = f.samples.iter().flat_map(|v| v.to_le_bytes()).collect();
                    let _ = file.write_all(&bytes);
                }
            }
            FeedEvent::Idle => {}
            FeedEvent::End(reason) => {
                let mut st = stats.lock().unwrap_or_else(|e| e.into_inner());
                let s = st.entry(id.to_string()).or_default();
                s.end = Some(reason.as_str().to_string());
                emit(
                    json!({"event": "stream_ended", "stream_id": id, "reason": reason.as_str(), "stats": s.clone()}),
                );
                return;
            }
        }
    }
}

struct Args {
    port: u16,
    code: Option<String>,
    ids: Vec<String>,
    duration: Option<Duration>,
    out_dir: Option<PathBuf>,
    idle: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        port: bridge::server::DEFAULT_PORT,
        code: None,
        ids: Vec::new(),
        duration: None,
        out_dir: None,
        idle: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--port" => a.port = value()?.parse().map_err(|_| "bad --port")?,
            "--code" => a.code = Some(value()?),
            "--extension-id" => a.ids.push(value()?),
            "--duration" => {
                a.duration = Some(Duration::from_secs_f64(
                    value()?.parse().map_err(|_| "bad --duration")?,
                ))
            }
            "--out-dir" => a.out_dir = Some(PathBuf::from(value()?)),
            "--idle" => a.idle = true,
            other => return Err(format!("unknown argument {other}")),
        }
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
        stats: Arc::new(Mutex::new(BTreeMap::new())),
        next_track: Mutex::new(0),
    });
    let hub = Hub::new(
        HubConfig {
            app_version: format!("test-receiver {}", env!("CARGO_PKG_VERSION")),
            ..HubConfig::default()
        },
        Vec::new(),
        receiver.clone(),
    );
    if let Ok(mut h) = receiver.hub.lock() {
        *h = Some(hub.clone());
    }
    let ports = if args.port == 0 {
        vec![0]
    } else {
        vec![args.port]
    };
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
