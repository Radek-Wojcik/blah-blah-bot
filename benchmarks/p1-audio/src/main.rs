//! Prototype P1: record the microphone and selected applications as separate segmented tracks.

mod capture;
mod convert;
mod journal;
mod play;
mod record;
mod sessions;
mod sysutil;
mod timeline;
mod track;
mod tracker;
mod wav;

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use clap::{Parser, Subcommand};

use capture::Source;
use convert::Format;

#[derive(Parser)]
#[command(
    version,
    about = "P1 prototype: per-process loopback and microphone capture"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Record tracks into segment files with a journal and a summary.
    Record {
        /// Output directory.
        #[arg(long)]
        out: PathBuf,
        /// Recording length in seconds.
        #[arg(long)]
        duration: f64,
        /// Record the default microphone as track `mic`.
        #[arg(long)]
        mic: bool,
        /// Record everything played on the default device as track `system` (reference).
        #[arg(long)]
        system: bool,
        /// Record one process and its children as track `pid-<pid>`; `PID:exclude` records
        /// everything except that process tree as track `pid-<pid>-excl`.
        #[arg(long = "pid", value_name = "PID[:tree]")]
        pids: Vec<String>,
        /// Segment format. Only `pcm` is implemented; see the README for the FLAC measurement.
        #[arg(long, default_value = "pcm", value_parser = ["pcm"])]
        format: String,
        /// Segment length in seconds.
        #[arg(long, default_value_t = 60)]
        segment_s: u32,
    },
    /// List audio sessions (process id, parent, process name, state).
    Sessions {
        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Check a recorded session directory against its journal.
    Replay {
        /// Directory with `journal.jsonl`.
        dir: PathBuf,
    },
    /// List active playback devices (no sound is produced).
    Devices,
    /// Test player: render a tone or clicks to a named playback device. The default device
    /// is never used implicitly, so nothing is played unless a device is named.
    Play {
        /// Part of the playback device name, for example `CABLE Input`.
        #[arg(long)]
        device: String,
        /// Sine frequency in Hz (default 440).
        #[arg(long, conflicts_with = "click")]
        tone: Option<f32>,
        /// Play one click per second instead of a tone.
        #[arg(long)]
        click: bool,
        #[arg(long, default_value_t = 10.0)]
        seconds: f64,
        #[arg(long, default_value_t = 0.25)]
        amplitude: f32,
    },
    /// Set volume and mute state of a process's audio sessions (Windows remembers them per
    /// application, and process loopback records after the session volume).
    SessionVolume {
        #[arg(long)]
        pid: u32,
        /// Volume from 0.0 to 1.0.
        #[arg(long, default_value_t = 1.0)]
        level: f32,
        /// Mute the session.
        #[arg(long)]
        mute: bool,
    },
    /// Report whether process loopback can be activated here, and in which formats.
    Probe {
        /// Target process id (default: this process).
        #[arg(long)]
        pid: Option<u32>,
    },
    /// Run the automated hardware tests (needs ffmpeg, ffplay and uv); extra arguments are
    /// passed to the test script.
    Selftest {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

fn cmd_sessions(json: bool) -> Result<(), String> {
    let list = sessions::list_sessions()?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    println!(
        "{:>7} {:>7}  {:<24} {:<9} {:>6} {:>5} {:<28} DISPLAY NAME",
        "PID", "PARENT", "PROCESS", "STATE", "VOLUME", "PEAK", "DEVICE"
    );
    for s in list {
        println!(
            "{:>7} {:>7}  {:<24} {:<9} {:>6} {:>5} {:<28} {}",
            s.pid,
            s.parent_pid.map_or("-".to_string(), |p| p.to_string()),
            s.process,
            s.state,
            match (s.volume, s.muted) {
                (_, Some(true)) => "muted".to_string(),
                (Some(v), _) => format!("{:.0}%", v * 100.0),
                _ => "-".to_string(),
            },
            s.peak.map_or("-".to_string(), |p| format!("{p:.2}")),
            s.device,
            s.display_name
        );
    }
    Ok(())
}

fn cmd_replay(dir: &std::path::Path) -> Result<(), String> {
    let r = journal::replay_file(&dir.join("journal.jsonl")).map_err(|e| e.to_string())?;
    println!(
        "session: {} Hz, {} samples per segment, ended: {}, undecodable lines: {}",
        r.sample_rate, r.segment_samples, r.session_ended, r.bad_lines
    );
    let mut bad = false;
    for (name, t) in &r.tracks {
        let problems = t.problems(r.segment_samples, r.sample_rate);
        println!(
            "{name}: {} segments, {:.1} s, {:.1} ms .. {:.1} ms, loss events {}, silence fills {}, ended {}",
            t.segments.len(),
            t.total_samples() as f64 / f64::from(r.sample_rate),
            t.offset_ms,
            t.end_offset_ms(r.sample_rate),
            t.loss_events(),
            t.discontinuities.len() - t.loss_events(),
            t.ended
        );
        for p in &problems {
            println!("  problem: {p:?}");
        }
        for s in &t.segments {
            let path = dir.join(&s.file);
            match wav::read_samples(&path) {
                Ok((_, samples)) if samples.len() as u64 == s.samples => {}
                Ok((_, samples)) => {
                    println!(
                        "  {}: file has {} samples, journal {}",
                        s.file,
                        samples.len(),
                        s.samples
                    );
                    bad = true;
                }
                Err(e) => {
                    println!("  {}: {e}", s.file);
                    bad = true;
                }
            }
        }
        bad |= !problems.is_empty();
    }
    if bad {
        Err("journal and files are inconsistent".into())
    } else {
        println!("ok");
        Ok(())
    }
}

fn cmd_probe(pid: Option<u32>) -> Result<(), String> {
    let _ = wasapi::initialize_mta();
    let pid = pid.unwrap_or_else(std::process::id);
    let os = sysutil::os_info();
    println!("os: {} (build {})", os.long_version, os.build);
    println!("target pid: {pid}");
    for tree in [false, true] {
        for format in [Format::Pcm16Mono16k, Format::Float32Stereo48k] {
            let source = Source::Process { pid, tree };
            match capture::try_format(&source, format) {
                Ok(_) => println!("include_tree={tree} {}: ok", format.label()),
                Err(e) => println!("include_tree={tree} {}: ERROR {e}", format.label()),
            }
        }
    }
    Ok(())
}

fn cmd_selftest(args: &[String]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("selftest")
        .join("selftest.py");
    let status = Command::new("uv")
        .arg("run")
        .arg("--quiet")
        .arg(&script)
        .arg("--exe")
        .arg(&exe)
        .args(args)
        .status()
        .map_err(|e| format!("cannot start `uv`: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("selftest failed: {status}"))
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Cmd::Record {
            out,
            duration,
            mic,
            system,
            pids,
            format: _,
            segment_s,
        } => {
            let mut sources = Vec::new();
            if mic {
                sources.push(Source::Mic);
            }
            if system {
                sources.push(Source::System);
            }
            for p in &pids {
                sources.push(Source::parse_pid(p)?);
            }
            record::record(record::RecordArgs {
                out,
                duration_s: duration,
                sources,
                segment_s,
            })
        }
        Cmd::Sessions { json } => cmd_sessions(json),
        Cmd::Replay { dir } => cmd_replay(&dir),
        Cmd::Probe { pid } => cmd_probe(pid),
        Cmd::Devices => {
            for name in play::list_render_devices()? {
                println!("{name}");
            }
            Ok(())
        }
        Cmd::Play {
            device,
            tone,
            click,
            seconds,
            amplitude,
        } => {
            let signal = if click {
                play::Signal::Click
            } else {
                play::Signal::Sine(tone.unwrap_or(440.0))
            };
            play::play(&device, signal, amplitude.clamp(0.0, 1.0), seconds)
        }
        Cmd::SessionVolume { pid, level, mute } => {
            let n = sessions::set_session_levels(pid, level.clamp(0.0, 1.0), mute)?;
            println!("{n} session(s) of pid {pid} updated");
            Ok(())
        }
        Cmd::Selftest { args } => cmd_selftest(&args),
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
