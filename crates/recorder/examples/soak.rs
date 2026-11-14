//! Soak run: record the default microphone and any number of applications for a fixed time
//! with the real WASAPI backend and write a JSON report.
//!
//! ```text
//! cargo run -p recorder --release --example soak -- \
//!     --seconds 7200 --mic --pid 1234 --pid 5678 --dir <folder> --result <file.json>
//! ```
//!
//! The report holds the measurements the Stage 1 thresholds are checked against: CPU of the
//! recording process as a share of all logical processors, working set, buffers lost, and
//! whether every track is complete and continuous.
#![cfg(windows)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use recorder::journal::replay_file;
use recorder::recovery::recover;
use recorder::session::{Session, SourceSpec, WasapiBackend};
use recorder::sysutil::{logical_cpus, os_info, process_cpu_seconds, working_set_bytes};

struct Args {
    seconds: u64,
    mic: bool,
    pids: Vec<u32>,
    dir: PathBuf,
    result: PathBuf,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        seconds: 60,
        mic: false,
        pids: Vec::new(),
        dir: PathBuf::new(),
        result: PathBuf::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match flag.as_str() {
            "--seconds" => a.seconds = value("--seconds")?.parse().map_err(|e| format!("{e}"))?,
            "--mic" => a.mic = true,
            "--pid" => a
                .pids
                .push(value("--pid")?.parse().map_err(|e| format!("{e}"))?),
            "--dir" => a.dir = value("--dir")?.into(),
            "--result" => a.result = value("--result")?.into(),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if a.dir.as_os_str().is_empty() || a.result.as_os_str().is_empty() {
        return Err("--dir and --result are required".into());
    }
    Ok(a)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse()?;
    let mut session = Session::start(&args.dir, 60, Arc::new(WasapiBackend))?;
    if args.mic {
        session.add_source(SourceSpec::Microphone, "microphone")?;
    }
    for pid in &args.pids {
        session.add_source(SourceSpec::Application { pid: *pid }, &format!("pid {pid}"))?;
    }

    let cpus = logical_cpus() as f64;
    let wall_start = Instant::now();
    let cpu_start = process_cpu_seconds();
    let mut peak_ws = 0u64;
    let mut samples = Vec::new();
    let mut next_sample = Duration::from_secs(30);
    while wall_start.elapsed() < Duration::from_secs(args.seconds) {
        std::thread::sleep(Duration::from_millis(500));
        let _ = session.snapshot(); // keeps the level meters moving like in the app
        let (ws, _) = working_set_bytes();
        peak_ws = peak_ws.max(ws);
        if wall_start.elapsed() >= next_sample {
            next_sample += Duration::from_secs(30);
            let cpu_pct =
                (process_cpu_seconds() - cpu_start) / wall_start.elapsed().as_secs_f64() / cpus
                    * 100.0;
            samples.push(serde_json::json!({
                "t_s": wall_start.elapsed().as_secs(),
                "cpu_pct_avg": cpu_pct,
                "working_set_mb": ws as f64 / 1_048_576.0,
            }));
        }
    }
    let wall_s = wall_start.elapsed().as_secs_f64();
    let cpu_pct = (process_cpu_seconds() - cpu_start) / wall_s / cpus * 100.0;
    let (final_ws, _) = working_set_bytes();
    let duration_ms = session.stop()?;

    let journal = replay_file(&args.dir.join(recorder::journal::JOURNAL_FILE_NAME))?;
    let meeting = recover(&args.dir)?;
    let tracks: Vec<_> = meeting
        .tracks
        .iter()
        .map(|t| {
            let loss = journal.tracks.get(&t.name).map_or(0, |j| j.loss_events());
            serde_json::json!({
                "track": t.name,
                "source": t.source,
                "offset_ms": t.offset_ms,
                "ended": t.ended,
                "end_offset_ms": t.end_offset_ms,
                "stored_s": t.total_samples as f64 / 16_000.0,
                "segments": t.segments.len(),
                "unusable_segments": t.unusable_segments,
                "continuity_problems": t.problems.len(),
                "lost_buffer_events": loss,
            })
        })
        .collect();
    let report = serde_json::json!({
        "measured": "stage 1 soak",
        "os": serde_json::to_value(format!("{:?}", os_info()))?,
        "logical_cpus": cpus,
        "requested_s": args.seconds,
        "wall_s": wall_s,
        "session_ms": duration_ms,
        "cpu_pct_of_total_avg": cpu_pct,
        "working_set_mb_peak": peak_ws as f64 / 1_048_576.0,
        "working_set_mb_final": final_ws as f64 / 1_048_576.0,
        "tracks": tracks,
        "samples": samples,
    });
    if let Some(parent) = args.result.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.result, serde_json::to_string_pretty(&report)?)?;
    println!(
        "done: {:.0} s, cpu {:.3} %, peak working set {:.1} MB, tracks {}",
        wall_s,
        cpu_pct,
        peak_ws as f64 / 1_048_576.0,
        meeting.tracks.len()
    );
    Ok(())
}
