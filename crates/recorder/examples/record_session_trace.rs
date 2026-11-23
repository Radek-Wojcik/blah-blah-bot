//! Records a session trace for the meeting detector: which audio sessions (playback and
//! microphone) exist on this machine, their state and a coarse peak (none, low, high), once a
//! second, keeping the changes only. ENUMERATION ONLY: no audio is captured, opened or stored.
//!
//! The raw trace holds program names and process ids of this machine and stays under
//! `target/traces` (never committed). The anonymized copy replaces process ids by their order
//! and programs other than the known conferencing applications by `app-N` / `browser-N`; it is
//! the file that may be committed.
//!
//! ```text
//! cargo run -p recorder --example record_session_trace -- --minutes 3 --idle \
//!     --description "Idle audio sessions of a Windows 10 machine" \
//!     --out crates/recorder/tests/traces/detect/idle-real.json
//! ```
//!
//! Options: `--minutes N` (default 3; a working day is `--minutes 480`), `--poll-ms N`
//! (default 1000), `--out FILE` (anonymized trace; default `target/traces/session-trace.json`),
//! `--raw FILE` (default `target/traces/session-trace.raw.json`), `--description TEXT`,
//! `--idle` (label the whole trace as a stretch without calls and expect no prompt).
//!
//! The tool prints how long a poll takes and the CPU share of the process, to check the
//! 0.1 % budget of the detector.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use recorder::detect::Sensitivity;
use recorder::detect::{BROWSERS, KNOWN_APPS};
use recorder::detect_trace::{anonymize, Expect, Label, LabelKind, TraceBuilder, TraceKind};
use recorder::sessions::SessionMonitor;
use recorder::sysutil::process_cpu_seconds;

fn workspace_target() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("traces")
}

fn write(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut minutes = 3.0_f64;
    let mut poll_ms = 1000_u64;
    let mut out = workspace_target().join("session-trace.json");
    let mut raw_out = workspace_target().join("session-trace.raw.json");
    let mut description = String::from("Audio sessions of a Windows machine, enumeration only");
    let mut idle = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "--minutes" => minutes = value("--minutes")?.parse()?,
            "--poll-ms" => poll_ms = value("--poll-ms")?.parse()?,
            "--out" => out = PathBuf::from(value("--out")?),
            "--raw" => raw_out = PathBuf::from(value("--raw")?),
            "--description" => description = value("--description")?,
            "--idle" => idle = true,
            other => return Err(format!("unknown option {other}").into()),
        }
    }

    let known: Vec<String> = KNOWN_APPS.iter().map(|s| (*s).to_string()).collect();
    let browsers: Vec<String> = BROWSERS.iter().map(|s| (*s).to_string()).collect();
    let mut monitor = SessionMonitor::new(known.clone())?;
    let mut builder = TraceBuilder::new(poll_ms);

    let total = Duration::from_secs_f64(minutes * 60.0);
    let cpu0 = process_cpu_seconds();
    let started = Instant::now();
    let (mut polls, mut poll_time, mut worst) = (0_u32, Duration::ZERO, Duration::ZERO);
    let mut first = Duration::ZERO;
    let mut next = started;
    while started.elapsed() < total {
        let t0 = Instant::now();
        let sessions = monitor.poll()?;
        let spent = t0.elapsed();
        if polls == 0 {
            first = spent;
        }
        poll_time += spent;
        worst = worst.max(spent);
        polls += 1;
        builder.push(u64::try_from(started.elapsed().as_millis())?, &sessions);
        next += Duration::from_millis(poll_ms);
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    let duration_ms = u64::try_from(started.elapsed().as_millis())?;
    let cpu = process_cpu_seconds() - cpu0;

    let mut raw = builder.finish(TraceKind::Real, &description, duration_ms);
    if idle {
        raw.labels.push(Label {
            start_ms: 0,
            end_ms: duration_ms,
            app: "*".into(),
            kind: LabelKind::NoPrompt,
            note: "idle machine, no call".into(),
        });
        for level in Sensitivity::ALL {
            raw.expect.insert(level, Expect { prompts: 0 });
        }
    }
    let anon = anonymize(&raw, &known, &browsers);
    write(&raw_out, &raw.to_json())?;
    write(&out, &anon.to_json())?;

    println!("polls: {polls}, events kept: {}", anon.events.len());
    println!("first poll: {:.3} ms", first.as_secs_f64() * 1000.0);
    println!(
        "poll time: mean {:.3} ms, worst {:.3} ms; process CPU {:.4} s over {:.0} s = {:.4} % of one core",
        poll_time.as_secs_f64() * 1000.0 / f64::from(polls.max(1)),
        worst.as_secs_f64() * 1000.0,
        cpu,
        started.elapsed().as_secs_f64(),
        cpu / started.elapsed().as_secs_f64() * 100.0
    );
    println!("anonymized trace: {}", out.display());
    println!("raw trace (do not commit): {}", raw_out.display());
    Ok(())
}
