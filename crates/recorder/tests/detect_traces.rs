//! Replays the committed session traces (`tests/traces/detect`) through the meeting detector
//! with every sensitivity and checks the prompts against the `expect` block of each trace.

use std::path::PathBuf;

use recorder::detect::{DetectConfig, Sensitivity, KNOWN_APPS};
use recorder::detect_trace::{replay, score, LabelKind, Trace, TraceKind};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("traces")
        .join("detect")
}

fn traces() -> Vec<(String, Trace)> {
    let mut out: Vec<(String, Trace)> = std::fs::read_dir(dir())
        .expect("the trace folder exists")
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .map(|e| {
            let name = e
                .path()
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let text = std::fs::read_to_string(e.path()).expect("readable trace");
            let trace = Trace::parse(&text).unwrap_or_else(|m| panic!("{name}: {m}"));
            (name, trace)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn config(level: Sensitivity) -> DetectConfig {
    DetectConfig {
        sensitivity: level,
        ..DetectConfig::default()
    }
}

#[test]
fn every_expected_trace_is_there() {
    let names: Vec<String> = traces().into_iter().map(|(n, _)| n).collect();
    for wanted in [
        "idle-real",
        "discord-call",
        "teams-call-webview-child",
        "teams-startup-device-check",
        "discord-mic-test",
        "browser-voice-search",
        "windows-dictation",
        "muted-call",
        "back-to-back-calls",
        "already-recording",
        "meet-call",
    ] {
        assert!(names.iter().any(|n| n == wanted), "{wanted} is missing");
    }
}

#[test]
fn each_trace_raises_the_expected_prompts_with_every_sensitivity() {
    for (name, trace) in traces() {
        for level in Sensitivity::ALL {
            let expect = trace
                .expect
                .get(&level)
                .unwrap_or_else(|| panic!("{name} has no expectation for {level}"));
            let prompts = replay(&trace, &config(level));
            assert_eq!(
                prompts.len(),
                expect.prompts,
                "{name} with {level}: prompts {prompts:?}"
            );
        }
    }
}

#[test]
fn idle_traces_raise_no_prompt_with_any_sensitivity() {
    let mut idle = 0;
    for (name, trace) in traces() {
        let no_calls = trace.labels.iter().all(|l| l.kind == LabelKind::NoPrompt);
        if trace.kind == TraceKind::Real || name.starts_with("idle") {
            idle += 1;
            assert!(no_calls, "{name} is labelled with calls");
            for level in Sensitivity::ALL {
                assert!(
                    replay(&trace, &config(level)).is_empty(),
                    "{name} prompted with {level}"
                );
            }
        }
    }
    assert!(idle >= 1, "no idle trace found");
}

#[test]
fn a_call_gets_one_prompt_within_the_sustain_time_plus_two_seconds() {
    for level in Sensitivity::ALL {
        let preset = config(level).preset();
        for name in ["discord-call", "teams-call-webview-child", "muted-call"] {
            let (_, trace) = traces()
                .into_iter()
                .find(|(n, _)| n == name)
                .unwrap_or_else(|| panic!("{name} is missing"));
            let s = score(&trace, &replay(&trace, &config(level)));
            assert_eq!(
                (
                    s.prompts,
                    s.false_prompts,
                    s.missed_calls,
                    s.duplicate_prompts
                ),
                (1, 0, 0, 0),
                "{name} with {level}"
            );
            assert!(
                s.delays_ms.iter().all(|d| *d <= preset.sustain_ms + 2_000),
                "{name} with {level}: {:?}",
                s.delays_ms
            );
        }
    }
}

#[test]
fn the_default_sensitivity_is_balanced_and_known_apps_are_the_documented_ones() {
    let cfg = DetectConfig::default();
    assert_eq!(cfg.sensitivity, Sensitivity::Balanced);
    assert_eq!(cfg.known_apps, KNOWN_APPS);
}

#[test]
fn committed_traces_hold_no_names_beyond_the_known_conferencing_apps() {
    for (name, trace) in traces() {
        assert_eq!(trace.poll_ms, 1000, "{name}");
        for app in trace.apps.keys() {
            let generic = app
                .strip_prefix("app-")
                .or_else(|| app.strip_prefix("browser-"))
                .is_some_and(|n| n.parse::<u32>().is_ok());
            assert!(
                generic || KNOWN_APPS.contains(&app.as_str()),
                "{name}: `{app}` is neither generic nor a known conferencing app"
            );
        }
        for e in &trace.events {
            for s in e.sessions.iter().flatten() {
                assert!(s.pid < 100, "{name}: pid {} is not an order number", s.pid);
            }
        }
    }
}
