//! The `record` command: start the tracks, watch resources, write the summary.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::capture::{run_track, Shared, Source, TrackState};
use crate::journal::{replay_file, Entry, Journal};
use crate::sysutil::{
    calibrate_cycles_per_second, logical_cpus, os_info, process_cpu_seconds, process_cycles,
    qpc_now_hns, unix_ms, working_set_bytes, OsInfo,
};
use crate::timeline::{segment_samples, SAMPLE_RATE};

/// Parameters of a recording.
pub struct RecordArgs {
    pub out: PathBuf,
    pub duration_s: f64,
    pub sources: Vec<Source>,
    pub segment_s: u32,
}

#[derive(Serialize)]
struct CpuSummary {
    /// Larger of the two estimates below.
    avg_percent_total: f64,
    /// From `GetProcessTimes` (scheduler-tick sampled, may under-count short wake-ups).
    avg_percent_total_ticks: f64,
    /// From the process cycle counter and a calibrated clock rate.
    avg_percent_total_cycles: f64,
    calibrated_ghz: f64,
    cpu_cycles: u64,
    max_window_percent_total: f64,
    cpu_seconds: f64,
    logical_cpus: usize,
    window_s: f64,
}

#[derive(Serialize)]
struct MemorySummary {
    peak_working_set_mb: f64,
    working_set_mb: f64,
}

#[derive(Serialize)]
struct Continuity {
    segments: usize,
    problems: Vec<String>,
    ok: bool,
    stored_s: f64,
}

#[derive(Serialize)]
struct TrackSummary {
    #[serde(flatten)]
    state: TrackState,
    source: String,
    dropped_buffers: u64,
    stored_s: f64,
    clock_ratio_ppm: Option<f64>,
    timestamp_ratio_ppm: Option<f64>,
    continuity: Option<Continuity>,
}

#[derive(Serialize)]
struct PairDrift {
    a: String,
    b: String,
    ppm_difference: Option<f64>,
    ms_over_run: Option<f64>,
    ms_over_10h: Option<f64>,
}

#[derive(Serialize)]
struct Totals {
    dropped_buffers: u64,
    timestamp_errors: u64,
    silence_fill_events: u64,
    silence_filled_s: f64,
    failed_tracks: Vec<String>,
}

#[derive(Serialize)]
struct Summary {
    complete: bool,
    started_unix_ms: u64,
    requested_duration_s: f64,
    elapsed_s: f64,
    os: OsInfo,
    sample_rate: u32,
    segment_s: u32,
    format: &'static str,
    totals: Totals,
    cpu: CpuSummary,
    memory: MemorySummary,
    tracks: BTreeMap<String, TrackSummary>,
    drift_pairs: Vec<PairDrift>,
}

struct Resources {
    cpu_start: f64,
    cycles_start: u64,
    cycles_per_s: f64,
    wall_start: Instant,
    last_cycles: u64,
    last_wall: Instant,
    max_window_percent: f64,
    peak_ws: u64,
}

impl Resources {
    fn new() -> Self {
        let cycles_per_s = calibrate_cycles_per_second();
        let now = Instant::now();
        let cpu = process_cpu_seconds();
        Self {
            cpu_start: cpu,
            cycles_start: process_cycles(),
            cycles_per_s,
            wall_start: now,
            last_cycles: process_cycles(),
            last_wall: now,
            max_window_percent: 0.0,
            peak_ws: 0,
        }
    }

    /// Take a sample; windows shorter than 5 s are skipped.
    fn sample(&mut self) {
        let (ws, peak) = working_set_bytes();
        self.peak_ws = self.peak_ws.max(peak).max(ws);
        let now = Instant::now();
        let window = now.duration_since(self.last_wall).as_secs_f64();
        if window >= 5.0 {
            let cycles = process_cycles();
            let used = cycles.saturating_sub(self.last_cycles) as f64 / self.cycles_per_s;
            let pct = used / window / logical_cpus() as f64 * 100.0;
            self.max_window_percent = self.max_window_percent.max(pct);
            self.last_cycles = cycles;
            self.last_wall = now;
        }
    }

    fn cpu_summary(&self) -> CpuSummary {
        let wall = self.wall_start.elapsed().as_secs_f64().max(1e-9);
        let cpu = process_cpu_seconds() - self.cpu_start;
        let cycles = process_cycles().saturating_sub(self.cycles_start);
        let ncpu = logical_cpus() as f64;
        let ticks = cpu / wall / ncpu * 100.0;
        let by_cycles = cycles as f64 / self.cycles_per_s / wall / ncpu * 100.0;
        CpuSummary {
            avg_percent_total: ticks.max(by_cycles),
            avg_percent_total_ticks: ticks,
            avg_percent_total_cycles: by_cycles,
            calibrated_ghz: self.cycles_per_s / 1e9,
            cpu_cycles: cycles,
            max_window_percent_total: self.max_window_percent,
            cpu_seconds: cpu,
            logical_cpus: logical_cpus(),
            window_s: 5.0,
        }
    }

    fn memory_summary(&self) -> MemorySummary {
        let (ws, peak) = working_set_bytes();
        MemorySummary {
            peak_working_set_mb: self.peak_ws.max(peak) as f64 / 1_048_576.0,
            working_set_mb: ws as f64 / 1_048_576.0,
        }
    }
}

fn build_summary(
    args: &RecordArgs,
    started_unix_ms: u64,
    elapsed_s: f64,
    complete: bool,
    states: &[(Source, Arc<Mutex<TrackState>>)],
    res: &Resources,
    out_dir: &Path,
) -> Summary {
    let seg_samples = segment_samples(args.segment_s, SAMPLE_RATE);
    let replay = replay_file(&out_dir.join("journal.jsonl")).ok();

    let mut tracks = BTreeMap::new();
    let mut totals = Totals {
        dropped_buffers: 0,
        timestamp_errors: 0,
        silence_fill_events: 0,
        silence_filled_s: 0.0,
        failed_tracks: Vec::new(),
    };
    for (source, state) in states {
        let st = state.lock().map(|s| s.clone()).unwrap_or_default();
        let name = source.track_name();
        let ppm = st.stats.clock_ratio_ppm();
        let ts_ppm = st.stats.timestamp_ratio_ppm();
        totals.dropped_buffers += st.stats.dropped_buffers();
        totals.timestamp_errors += st.stats.timestamp_errors;
        totals.silence_fill_events += st.stats.silence_fill_events;
        totals.silence_filled_s += st.stats.silence_filled_samples as f64 / f64::from(SAMPLE_RATE);
        if st.state == "failed" {
            totals.failed_tracks.push(name.clone());
        }
        let continuity = replay.as_ref().and_then(|r| {
            r.tracks.get(&name).map(|t| {
                let problems: Vec<String> = t
                    .problems(seg_samples, SAMPLE_RATE)
                    .iter()
                    .map(|p| format!("{p:?}"))
                    .collect();
                Continuity {
                    segments: t.segments.len(),
                    ok: problems.is_empty(),
                    problems,
                    stored_s: t.total_samples() as f64 / f64::from(SAMPLE_RATE),
                }
            })
        });
        tracks.insert(
            name,
            TrackSummary {
                source: source.kind().to_string(),
                dropped_buffers: st.stats.dropped_buffers(),
                stored_s: st.stored_samples as f64 / f64::from(SAMPLE_RATE),
                clock_ratio_ppm: ppm,
                timestamp_ratio_ppm: ts_ppm,
                continuity,
                state: st,
            },
        );
    }

    let mut drift_pairs = Vec::new();
    let names: Vec<&String> = tracks.keys().collect();
    for (i, a) in names.iter().enumerate() {
        for b in &names[i + 1..] {
            let diff = match (tracks[*a].clock_ratio_ppm, tracks[*b].clock_ratio_ppm) {
                (Some(x), Some(y)) => Some(x - y),
                _ => None,
            };
            drift_pairs.push(PairDrift {
                a: (*a).clone(),
                b: (*b).clone(),
                ppm_difference: diff,
                ms_over_run: diff.map(|d| d * elapsed_s / 1e3),
                ms_over_10h: diff.map(|d| d * 36_000.0 / 1e3),
            });
        }
    }

    Summary {
        complete,
        started_unix_ms,
        requested_duration_s: args.duration_s,
        elapsed_s,
        os: os_info(),
        sample_rate: SAMPLE_RATE,
        segment_s: args.segment_s,
        format: "pcm16-wav",
        totals,
        cpu: res.cpu_summary(),
        memory: res.memory_summary(),
        tracks,
        drift_pairs,
    }
}

fn write_summary(out_dir: &Path, summary: &Summary) -> Result<(), String> {
    let tmp = out_dir.join("summary.json.tmp");
    let text = serde_json::to_string_pretty(summary).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, out_dir.join("summary.json")).map_err(|e| e.to_string())
}

/// Record until the duration passes or a file named `STOP` appears in the output directory.
pub fn record(args: RecordArgs) -> Result<(), String> {
    if args.sources.is_empty() {
        return Err("nothing to record: give --mic, --system or --pid".into());
    }
    std::fs::create_dir_all(&args.out).map_err(|e| e.to_string())?;
    let stop_file = args.out.join("STOP");
    let _ = std::fs::remove_file(&stop_file);

    let journal = Arc::new(Mutex::new(
        Journal::create(&args.out.join("journal.jsonl")).map_err(|e| e.to_string())?,
    ));
    let seg_samples = segment_samples(args.segment_s, SAMPLE_RATE);
    let started_unix_ms = unix_ms();
    let shared = Arc::new(Shared {
        clock_start_hns: qpc_now_hns(),
        stop: false.into(),
        journal: journal.clone(),
        out_dir: args.out.clone(),
        seg_samples,
    });
    journal
        .lock()
        .map_err(|_| "journal lock poisoned".to_string())?
        .append(&Entry::SessionStart {
            started_unix_ms,
            sample_rate: SAMPLE_RATE,
            segment_samples: seg_samples,
            format: "pcm16-wav".into(),
        })
        .map_err(|e| e.to_string())?;

    let mut res = Resources::new();
    let mut states = Vec::new();
    let mut threads = Vec::new();
    for source in &args.sources {
        let state: Arc<Mutex<TrackState>> = Arc::new(Mutex::new(TrackState {
            state: "starting".into(),
            ..Default::default()
        }));
        states.push((source.clone(), state.clone()));
        let sh = shared.clone();
        let src = source.clone();
        threads.push(
            std::thread::Builder::new()
                .name(format!("capture-{}", source.track_name()))
                .spawn(move || run_track(src, sh, state))
                .map_err(|e| e.to_string())?,
        );
    }

    let wall = Instant::now();
    let mut last_summary = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(500));
        res.sample();
        let elapsed = wall.elapsed().as_secs_f64();
        if elapsed >= args.duration_s || stop_file.exists() {
            break;
        }
        if threads.iter().all(|t| t.is_finished()) {
            break;
        }
        if last_summary.elapsed().as_secs() >= 60 {
            last_summary = Instant::now();
            let s = build_summary(
                &args,
                started_unix_ms,
                elapsed,
                false,
                &states,
                &res,
                &args.out,
            );
            write_summary(&args.out, &s)?;
        }
    }

    shared.stop.store(true, Ordering::Relaxed);
    for t in threads {
        let _ = t.join();
    }
    res.sample();
    let elapsed = wall.elapsed().as_secs_f64();
    journal
        .lock()
        .map_err(|_| "journal lock poisoned".to_string())?
        .append(&Entry::SessionEnd {
            duration_ms: shared.now_ms(),
        })
        .map_err(|e| e.to_string())?;
    let summary = build_summary(
        &args,
        started_unix_ms,
        elapsed,
        true,
        &states,
        &res,
        &args.out,
    );
    write_summary(&args.out, &summary)?;
    let _ = std::fs::remove_file(&stop_file);
    println!(
        "recorded {:.1} s; dropped buffers {}; cpu {:.3} %; peak memory {:.1} MB; summary: {}",
        elapsed,
        summary.totals.dropped_buffers,
        summary.cpu.avg_percent_total,
        summary.memory.peak_working_set_mb,
        args.out.join("summary.json").display()
    );
    if !summary.totals.failed_tracks.is_empty() {
        eprintln!("failed tracks: {}", summary.totals.failed_tracks.join(", "));
    }
    Ok(())
}
