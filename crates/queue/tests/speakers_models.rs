//! Speaker recognition with the real worker, real models and a GPU, on synthetic meetings.
//!
//! Ignored by default; run locally before a release:
//!
//! ```text
//! BBB_AUDIO_DIR=<folder with the synthetic meetings, one folder each> \
//! cargo test -p queue --test speakers_models -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! Set `BBB_RESULTS_OUT=<file>` to write the measured numbers as JSON.
//!
//! Each meeting folder holds `mixed.wav` (one mixed track), `reference.rttm` (who speaks when)
//! and `speakers.json` (the synthesized voice of every speaker). The recordings are synthetic
//! and not part of the repository.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use queue::cleanup::AudioRetention;
use queue::runner::{PipelineRunner, RunnerSettings};
use queue::voices::{self, VoiceSettings};
use queue::worker::{JobOptions, WorkerConfig};
use queue::{GateDecision, GpuGate, Stage, StepOutcome, Supervisor};
use store::{Meeting, MeetingState, Segment, Store, Track, VoiceDecision};

const SEGMENT_SAMPLES: usize = 16_000 * 60;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn audio_dir() -> PathBuf {
    PathBuf::from(std::env::var("BBB_AUDIO_DIR").expect("BBB_AUDIO_DIR is not set"))
}

fn read_pcm(path: &Path) -> Vec<u8> {
    let bytes = std::fs::read(path).unwrap();
    let pos = bytes
        .windows(4)
        .position(|w| w == b"data")
        .expect("data chunk");
    let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
    bytes[pos + 8..pos + 8 + len].to_vec()
}

fn write_wav(path: &Path, pcm: &[u8]) {
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&16_000u32.to_le_bytes());
    out.extend_from_slice(&32_000u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(pcm);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, out).unwrap();
}

struct Allow;
impl GpuGate for Allow {
    fn check(&self, _: Stage) -> GateDecision {
        GateDecision::Go
    }
}

struct Lab {
    root: PathBuf,
    store: Arc<Mutex<Store>>,
    sup: Supervisor,
}

fn lab(name: &str) -> Lab {
    let root = std::env::temp_dir().join(format!("bbb-spk-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let worker_dir = repo_root().join("worker");
    let runner_root = root.clone();
    let runner = PipelineRunner::new(
        store.clone(),
        move || RunnerSettings {
            worker: WorkerConfig {
                uv: PathBuf::from("uv"),
                worker_dir: worker_dir.clone(),
                env_dir: worker_dir.join(".venv"),
                models_dir: None,
                llama_server: None,
                hf_token: None,
                step_timeout: Duration::from_secs(3600),
            },
            meeting_root: runner_root.clone(),
            retention: AudioRetention::Delete,
            voice: VoiceSettings::default(),
        },
        |_, _, _, _| {},
    );
    let sup = Supervisor::new(store.clone(), Arc::new(runner), Arc::new(Allow), 3);
    Lab { root, store, sup }
}

/// Record a synthetic meeting into the lab the way the recorder writes it.
fn add_meeting(lab: &Lab, id: &str, source: &Path, speakers: u32) {
    {
        let store = lab.store.lock().unwrap();
        store
            .create_meeting(&Meeting {
                id: id.into(),
                title: id.into(),
                started_unix_ms: 1,
                duration_ms: None,
                state: MeetingState::Stopped,
                dir: id.into(),
            })
            .unwrap();
        store
            .upsert_track(
                id,
                &Track {
                    name: "meet".into(),
                    source: "app:meet.exe".into(),
                    pid: None,
                    offset_ms: 0.0,
                    ended: true,
                    end_offset_ms: None,
                    total_samples: None,
                },
            )
            .unwrap();
        let pcm = read_pcm(&source.join("mixed.wav"));
        for (i, chunk) in pcm.chunks(SEGMENT_SAMPLES * 2).enumerate() {
            let file = format!("meet/{i:06}.wav");
            write_wav(&lab.root.join(id).join(&file), chunk);
            store
                .add_segment(
                    id,
                    &Segment {
                        track: "meet".into(),
                        segment: i as u32,
                        file,
                        offset_ms: (i * 60_000) as f64,
                        samples: (chunk.len() / 2) as u64,
                    },
                )
                .unwrap();
        }
    }
    let options = serde_json::to_string(&JobOptions {
        max_speakers: Some(speakers),
        ..JobOptions::default()
    })
    .unwrap();
    lab.sup.enqueue(id, &options).unwrap();
}

/// `(start_s, end_s, speaker)` of the reference.
fn read_rttm(path: &Path) -> Vec<(f64, f64, String)> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split_whitespace().collect();
            (p.len() >= 8 && p[0] == "SPEAKER").then(|| {
                let start: f64 = p[3].parse().unwrap();
                (start, start + p[4].parse::<f64>().unwrap(), p[7].to_string())
            })
        })
        .collect()
}

/// Which reference speaker each detected voice is, by the most overlapping speech.
fn truth_of_voices(lab: &Lab, meeting: &str, source: &Path) -> HashMap<String, String> {
    let reference = read_rttm(&source.join("reference.rttm"));
    let doc: serde_json::Value = serde_json::from_slice(
        &std::fs::read(lab.root.join(meeting).join("work/diarize/meet.json")).unwrap(),
    )
    .unwrap();
    let mut overlap: HashMap<(String, String), f64> = HashMap::new();
    for t in doc["turns"].as_array().unwrap() {
        let (s, e) = (
            t["start_ms"].as_f64().unwrap() / 1000.0,
            t["end_ms"].as_f64().unwrap() / 1000.0,
        );
        let label = t["speaker"].as_str().unwrap().to_string();
        for (rs, re, who) in &reference {
            let ov = e.min(*re) - s.max(*rs);
            if ov > 0.0 {
                *overlap.entry((label.clone(), who.clone())).or_default() += ov;
            }
        }
    }
    let mut best: HashMap<String, (String, f64)> = HashMap::new();
    for ((label, who), ov) in overlap {
        if best.get(&label).is_none_or(|b| ov > b.1) {
            best.insert(label, (who, ov));
        }
    }
    best.into_iter()
        .map(|(label, (who, _))| (voices::voice_id("meet", &label), who))
        .collect()
}

fn synthesized_voice(source: &Path, speaker: &str) -> String {
    let doc: serde_json::Value =
        serde_json::from_slice(&std::fs::read(source.join("speakers.json")).unwrap()).unwrap();
    doc[speaker]["voice"].as_str().unwrap().to_string()
}

fn speaker_count(source: &Path) -> u32 {
    read_rttm(&source.join("reference.rttm"))
        .into_iter()
        .map(|r| r.2)
        .collect::<std::collections::BTreeSet<_>>()
        .len() as u32
}

/// Give a participant per synthesized voice, created on first sight.
fn participant_for(lab: &Lab, voice: &str) -> String {
    let store = lab.store.lock().unwrap();
    match store.find_participant_by_name(voice).unwrap() {
        Some(p) => p.id,
        None => {
            store
                .create_participant(&format!("p-{voice}"), voice, 1)
                .unwrap();
            format!("p-{voice}")
        }
    }
}

#[test]
#[ignore = "needs real models and a GPU"]
fn a_voice_named_in_one_meeting_is_recognized_in_the_next_and_a_new_voice_stays_unknown() {
    let audio = audio_dir();
    let lab = lab("pair");
    let (a, b) = (audio.join("pl-04a"), audio.join("pl-04b"));
    add_meeting(&lab, "a", &a, speaker_count(&a));
    add_meeting(&lab, "b", &b, speaker_count(&b));

    // First meeting: nothing is known, every voice has clips, and naming teaches the profiles.
    assert_eq!(lab.sup.drive("a").unwrap(), StepOutcome::Finished);
    let truth_a = truth_of_voices(&lab, "a", &a);
    {
        let store = lab.store.lock().unwrap();
        let voices_a = store.list_meeting_voices("a").unwrap();
        assert_eq!(voices_a.len(), 4);
        assert!(voices_a.iter().all(|v| v.decision == VoiceDecision::Unknown));
        let clips = store.list_clips("a").unwrap();
        for v in &voices_a {
            assert!(
                clips.iter().any(|c| c.voice_id == v.voice_id),
                "no clip for {}",
                v.voice_id
            );
        }
        // The audio is gone after processing, the clips are still there.
        assert!(!lab.root.join("a/meet").exists());
        for c in &clips {
            assert!(lab.root.join(&c.file).exists());
        }
    }
    for (voice_id, speaker) in &truth_a {
        let pid = participant_for(&lab, &synthesized_voice(&a, speaker));
        let store = lab.store.lock().unwrap();
        voices::assign_voice(&store, "a", voice_id, Some(&pid), 10).unwrap();
    }

    // Second meeting: known voices are named automatically, the new voice is not.
    assert_eq!(lab.sup.drive("b").unwrap(), StepOutcome::Finished);
    let truth_b = truth_of_voices(&lab, "b", &b);
    let store = lab.store.lock().unwrap();
    let voices_b = store.list_meeting_voices("b").unwrap();
    let enrolled: Vec<String> = truth_a
        .values()
        .map(|s| synthesized_voice(&a, s))
        .collect();
    for v in &voices_b {
        let person = synthesized_voice(&b, &truth_b[&v.voice_id]);
        if enrolled.contains(&person) {
            assert_eq!(v.decision, VoiceDecision::Auto, "{person} not recognized: {v:?}");
            assert_eq!(v.assigned_participant.as_deref(), Some(format!("p-{person}").as_str()));
        } else {
            assert_ne!(v.decision, VoiceDecision::Auto, "unknown {person} was named: {v:?}");
            assert_eq!(v.assigned_participant, None);
        }
    }
}

/// Measures recognition over all meetings of the folder in order: after each meeting the
/// user "confirms" the true names, which teaches the profiles.
#[test]
#[ignore = "needs real models and a GPU; takes several minutes"]
fn recognition_accuracy_and_false_accept_rate_over_a_series_of_meetings() {
    let audio = audio_dir();
    let order = ["pl-04a", "pl-04b", "pl-03", "pl-08", "en-03", "mixed-04", "pl-16"];
    let lab = lab("series");
    let mut known: Vec<String> = Vec::new();
    // Speech seconds by outcome.
    let (mut enrolled_total, mut enrolled_auto_ok, mut enrolled_suggested_ok) = (0.0, 0.0, 0.0);
    let (mut enrolled_wrong_auto, mut enrolled_unknown) = (0.0, 0.0);
    let (mut new_voices, mut new_voices_auto, mut new_voices_suggested) = (0u32, 0u32, 0u32);
    let mut rows = Vec::new();

    for name in order {
        let source = audio.join(name);
        if !source.exists() {
            continue;
        }
        add_meeting(&lab, name, &source, speaker_count(&source));
        assert_eq!(lab.sup.drive(name).unwrap(), StepOutcome::Finished, "{name}");
        let truth = truth_of_voices(&lab, name, &source);
        let voices_now = lab.store.lock().unwrap().list_meeting_voices(name).unwrap();
        let mut learned: BTreeMap<String, String> = BTreeMap::new();
        for v in &voices_now {
            let Some(speaker) = truth.get(&v.voice_id) else {
                continue;
            };
            let person = synthesized_voice(&source, speaker);
            let seconds = v.speech_ms as f64 / 1000.0;
            let predicted = v
                .matched_participant
                .as_ref()
                .map(|p| p.trim_start_matches("p-").to_string());
            let outcome;
            if known.contains(&person) {
                enrolled_total += seconds;
                match (v.decision, predicted.as_deref()) {
                    (VoiceDecision::Auto, Some(p)) if p == person => {
                        enrolled_auto_ok += seconds;
                        outcome = "auto, correct";
                    }
                    (VoiceDecision::Auto, _) => {
                        enrolled_wrong_auto += seconds;
                        outcome = "auto, WRONG";
                    }
                    (VoiceDecision::Suggested, Some(p)) if p == person => {
                        enrolled_suggested_ok += seconds;
                        outcome = "suggested, correct";
                    }
                    _ => {
                        enrolled_unknown += seconds;
                        outcome = "not recognized";
                    }
                }
            } else {
                new_voices += 1;
                outcome = match v.decision {
                    VoiceDecision::Auto => {
                        new_voices_auto += 1;
                        "new voice named automatically (FALSE ACCEPT)"
                    }
                    VoiceDecision::Suggested => {
                        new_voices_suggested += 1;
                        "new voice suggested"
                    }
                    VoiceDecision::Unknown => "new voice stays unknown",
                };
            }
            rows.push(serde_json::json!({
                "meeting": name, "voice": person, "enrolled": known.contains(&person),
                "speech_s": (seconds * 10.0).round() / 10.0,
                "decision": format!("{:?}", v.decision).to_lowercase(),
                "score": v.score.map(|s| (f64::from(s) * 1000.0).round() / 1000.0),
                "predicted": predicted, "outcome": outcome,
            }));
            learned.insert(v.voice_id.clone(), person);
        }
        // The user confirms the true names.
        for (voice_id, person) in &learned {
            let pid = participant_for(&lab, person);
            let store = lab.store.lock().unwrap();
            voices::assign_voice(&store, name, voice_id, Some(&pid), 100).unwrap();
            if !known.contains(person) {
                known.push(person.clone());
            }
        }
    }

    let accuracy = enrolled_auto_ok / enrolled_total.max(f64::MIN_POSITIVE);
    let far = f64::from(new_voices_auto) / f64::from(new_voices.max(1));
    println!(
        "speaker accuracy (auto, correct) {:.1}% of {:.0} s; wrong auto {:.1} s; suggested-correct {:.1} s; unknown {:.1} s",
        accuracy * 100.0, enrolled_total, enrolled_wrong_auto, enrolled_suggested_ok, enrolled_unknown
    );
    println!("false accept rate {:.1}% ({new_voices_auto} of {new_voices} new voices; {new_voices_suggested} suggested)", far * 100.0);
    if let Ok(out) = std::env::var("BBB_RESULTS_OUT") {
        let doc = serde_json::json!({
            "prototype": "stage3-speaker-recognition",
            "thresholds": {
                "accept": VoiceSettings::default().thresholds.accept,
                "suggest": VoiceSettings::default().thresholds.suggest,
                "margin": VoiceSettings::default().thresholds.margin,
            },
            "meetings": order,
            "enrolled_speech_s": enrolled_total,
            "speaker_accuracy_auto": accuracy,
            "enrolled_wrong_auto_s": enrolled_wrong_auto,
            "enrolled_suggested_correct_s": enrolled_suggested_ok,
            "enrolled_unknown_s": enrolled_unknown,
            "new_voices": new_voices,
            "new_voices_named_automatically": new_voices_auto,
            "new_voices_suggested": new_voices_suggested,
            "false_accept_rate": far,
            "voices": rows,
        });
        std::fs::write(out, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    }
    assert!(accuracy >= 0.90, "speaker accuracy {accuracy:.3} is below the 90% target");
    assert!(far <= 0.02, "false accept rate {far:.3} is above the 2% target");
}
