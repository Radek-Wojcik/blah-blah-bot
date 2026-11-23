//! Replays the session traces through the meeting detector once per sensitivity and writes
//! `benchmarks/results/stage6-detection-<preset>.json`: false prompts, missed calls, duplicate
//! prompts and the delay from the start of a call to its prompt.
//!
//! ```text
//! cargo run -p recorder --example score_detection
//! cargo run -p recorder --example score_detection -- --traces <folder> --out <folder> --preset balanced
//! ```
//!
//! Options: `--traces DIR` (default `crates/recorder/tests/traces/detect`), `--out DIR`
//! (default `benchmarks/results`), `--preset low|balanced|high` (repeatable; default all).
//! Every `*.json` file of the trace folder is scored, so a trace recorded over a working day
//! with `record_session_trace` can be added to the folder, or scored from another one with
//! `--traces`.

use std::path::PathBuf;

use recorder::detect::{DetectConfig, Preset, Sensitivity};
use recorder::detect_trace::{replay, score, Trace, TraceKind, TraceScore};
use serde_json::{json, Value};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn preset_json(p: Preset) -> Value {
    json!({
        "sustain_s": p.sustain_ms / 1000,
        "playback_condition": p.render,
        "meet_audible_s": p.meet_audible_ms / 1000,
        "end_of_call_s": p.end_of_call_ms / 1000,
        "rest_after_not_now_min": p.not_now_extra_ms / 60_000,
        "poll_s": p.poll_ms / 1000,
    })
}

fn stats(delays: &[u64]) -> Value {
    if delays.is_empty() {
        return json!({ "count": 0 });
    }
    let sum: u64 = delays.iter().sum();
    json!({
        "count": delays.len(),
        "mean_s": sum as f64 / delays.len() as f64 / 1000.0,
        "max_s": delays.iter().max().copied().unwrap_or(0) as f64 / 1000.0,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut traces_dir = root().join("tests").join("traces").join("detect");
    let mut out_dir = root()
        .join("..")
        .join("..")
        .join("benchmarks")
        .join("results");
    let mut levels: Vec<Sensitivity> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "--traces" => traces_dir = PathBuf::from(value("--traces")?),
            "--out" => out_dir = PathBuf::from(value("--out")?),
            "--preset" => levels.push(value("--preset")?.parse()?),
            other => return Err(format!("unknown option {other}").into()),
        }
    }
    if levels.is_empty() {
        levels = Sensitivity::ALL.to_vec();
    }

    let mut files: Vec<PathBuf> = std::fs::read_dir(&traces_dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let mut traces: Vec<(String, Trace)> = Vec::new();
    for f in &files {
        let name = f
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        traces.push((name, Trace::parse(&std::fs::read_to_string(f)?)?));
    }
    std::fs::create_dir_all(&out_dir)?;

    for level in levels {
        let cfg = DetectConfig {
            sensitivity: level,
            ..DetectConfig::default()
        };
        let preset = cfg.preset();
        let mut rows = Vec::new();
        let mut all_delays: Vec<u64> = Vec::new();
        let (mut real_false, mut real_ms) = (0_usize, 0_u64);
        let mut totals = TraceScore {
            prompts: 0,
            calls: 0,
            false_prompts: 0,
            missed_calls: 0,
            duplicate_prompts: 0,
            delays_ms: Vec::new(),
        };
        let mut synthetic_false = 0_usize;
        let mut expectations_met = true;
        for (name, trace) in &traces {
            let prompts = replay(trace, &cfg);
            let s = score(trace, &prompts);
            let expected = trace.expect.get(&level).map(|e| e.prompts);
            let met = expected.is_none_or(|n| n == s.prompts);
            expectations_met &= met;
            if trace.kind == TraceKind::Real {
                real_false += s.false_prompts;
                real_ms += trace.duration_ms;
            } else {
                synthetic_false += s.false_prompts;
            }
            totals.prompts += s.prompts;
            totals.calls += s.calls;
            totals.false_prompts += s.false_prompts;
            totals.missed_calls += s.missed_calls;
            totals.duplicate_prompts += s.duplicate_prompts;
            all_delays.extend(&s.delays_ms);
            rows.push(json!({
                "trace": name,
                "kind": trace.kind,
                "duration_s": trace.duration_ms / 1000,
                "labelled_calls": s.calls,
                "prompts": s.prompts,
                "false_prompts": s.false_prompts,
                "missed_calls": s.missed_calls,
                "duplicate_prompts": s.duplicate_prompts,
                "delays_s": s.delays_ms.iter().map(|d| *d as f64 / 1000.0).collect::<Vec<_>>(),
                "expected_prompts": expected,
                "expectation_met": met,
                "description": trace.description,
            }));
        }
        let max_delay = all_delays.iter().max().copied().unwrap_or(0);
        // Sustain time (the longer of the microphone and the Meet time) plus two seconds.
        let target_ms = preset.sustain_ms.max(preset.meet_audible_ms) + 2_000;
        let doc = json!({
            "name": format!("stage6-detection-{level}"),
            "preset": level.as_str(),
            "parameters": preset_json(preset),
            "how_measured": "Offline replay of session traces through recorder::detect (the rules the \
        application runs), polling at the interval of the preset and applying each trace event at the \
        first poll at or after its time. A trace lists audio sessions (playback and microphone, state, \
        coarse peak none/low/high) and, where labelled, Meet events and the applications being recorded; \
        it holds no audio. Prompts are never answered in a replay, so a call is prompted once and ends \
        after the end-of-call time without a signal. A prompt inside a labelled call of the same \
        application counts for that call (the first one is the prompt, later ones are duplicates); any \
        other prompt is false; a call without a prompt is missed; delay is the time from the start of \
        the call to its prompt.",
            "traces_are": {
                "real": "enumeration of the audio sessions of the reference machine while it was idle \
        (no call), anonymized: process ids replaced by their order, programs other than the known \
        conferencing applications named app-N, no device or display names",
                "synthetic": "made by hand to exercise one situation each (a call, a device check, a \
        microphone test, voice search, dictation, muted call, back-to-back calls, an application that is \
        being recorded, Meet). They are NOT recordings of real calls: the sequences of session states in \
        them are assumptions about how Discord, Teams and a browser behave, which a measurement on real \
        calls has to confirm",
            },
            "totals": {
                "traces": traces.len(),
                "labelled_calls": totals.calls,
                "prompts": totals.prompts,
                "false_prompts": totals.false_prompts,
                "false_prompts_real_idle_traces": real_false,
                "false_prompts_synthetic_traces": synthetic_false,
                "missed_calls": totals.missed_calls,
                "duplicate_prompts": totals.duplicate_prompts,
                "delay": stats(&all_delays),
                "max_delay_within_target_for_calls_that_prompted":
                    max_delay <= target_ms || all_delays.is_empty(),
                "all_trace_expectations_met": expectations_met,
            },
            "real_trace_duration_s": real_ms / 1000,
            "targets": {
                "false_prompts_from_idle_applications": 0,
                "prompts_per_labelled_call": 1,
                "delay_s_at_most": target_ms as f64 / 1000.0,
                "cpu_share_at_most_percent": 0.1,
            },
            "not_yet_measured": [
                "A working day (8 h) of real use: record one with record_session_trace --minutes 480 and add the anonymized file to the trace folder.",
                "Real Discord, Teams (new and classic) and Zoom calls: the state of their microphone and playback sessions during a call, a mute and a device check.",
                "Windows 11.",
            ],
            "traces": rows,
        });
        let path = out_dir.join(format!("stage6-detection-{level}.json"));
        std::fs::write(&path, serde_json::to_string_pretty(&doc)? + "\n")?;
        println!(
            "{level}: {} prompts, {} false ({} on real idle traces), {} missed, {} duplicate -> {}",
            totals.prompts,
            totals.false_prompts,
            real_false,
            totals.missed_calls,
            totals.duplicate_prompts,
            path.display()
        );
    }
    Ok(())
}
