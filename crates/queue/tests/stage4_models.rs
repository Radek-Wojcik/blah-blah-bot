//! Stage 4 with the real worker, real models and a GPU: a synthetic meeting of a cycle goes
//! through the whole pipeline, including `extract` and `reduce` with a managed `llama-server`.
//!
//! Ignored by default; run locally before a release (and by the night run):
//!
//! ```text
//! BBB_AUDIO_DIR=<folder with the synthetic meetings, one folder each> \
//! BBB_LLM_MODEL=<GGUF file of the language model> \
//! BBB_LLAMA_SERVER=<llama-server.exe> \
//! cargo test -p queue --test stage4_models -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! The speech models (Whisper, the diarizer, the speaker embedder) come from the worker's
//! usual model cache, as for `speakers_models`. Each meeting folder holds `mixed.wav`,
//! `reference.rttm`, `annotation.json` (language, date and the speakers of the script) and
//! `speakers.json`. The recordings are synthetic and not part of the repository.
//!
//! The meeting is registered as a meeting of the cycle `NOMAD` whose participants are the
//! speakers of the script. The voices are named by the user after `clips` (here from the
//! reference), so `extract` sees the participants' names. The job then runs with the real
//! GPU guard, and the draft is checked in three report modes: `fixed`, `auto` (Polish
//! meeting only, to keep the run short) and `template`, whose draft is then edited.
//!
//! The cloud refusal needs no model and is covered by `stage4_fake_worker`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cycle_store::draft::Draft;
use cycle_store::{
    Cycle, ItemType, Language, Participant as CycleParticipant, ReportMode, Schema, Template,
    TemplateSection, Variant,
};
use queue::cleanup::AudioRetention;
use queue::context::{CycleData, CycleSource};
use queue::draft::{self, DraftFiles, ItemPatch};
use queue::gpu::{GuardPolicy, NvmlGate, NvmlSampler};
use queue::runner::{PipelineRunner, RunnerSettings};
use queue::voices::{self, VoiceSettings};
use queue::worker::{JobOptions, LlmOptions, WorkerConfig};
use queue::{GateDecision, GpuGate, RunError, Stage, StepOutcome, Supervisor, STAGES};
use store::{LlmKind, Meeting, MeetingState, ReportSettings, Segment, StageStatus, Store, Track};

const SEGMENT_SAMPLES: usize = 16_000 * 60;
const CYCLE_ID: &str = "NOMAD";
const CTX: u32 = 16_384;
/// 2026-11-13 10:00 UTC, the date of the synthetic meetings.
const STARTED_UNIX_MS: i64 = 1_794_564_000_000;
const TEMPLATE_NAME: &str = "sprint";
const TEMPLATE_BODY: &str =
    "# {title}\n\n{date} · {participants}\n\n{summary_section}\n\n{sections}\n";
/// How long a stage may wait for the GPU guard before the test gives up.
const GATE_PATIENCE: Duration = Duration::from_secs(45 * 60);

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn env_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set")))
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

/// `(start_s, end_s, speaker)` of the reference.
fn read_rttm(path: &Path) -> Vec<(f64, f64, String)> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split_whitespace().collect();
            (p.len() >= 8 && p[0] == "SPEAKER").then(|| {
                let start: f64 = p[3].parse().unwrap();
                (
                    start,
                    start + p[4].parse::<f64>().unwrap(),
                    p[7].to_string(),
                )
            })
        })
        .collect()
}

/// Which reference speaker each detected voice is, by the most overlapping speech.
fn truth_of_voices(root: &Path, meeting: &str, source: &Path) -> HashMap<String, String> {
    let reference = read_rttm(&source.join("reference.rttm"));
    let doc: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join(meeting).join("work/diarize/meet.json")).unwrap(),
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

/// A gate that asks the real GPU guard and remembers what it answered.
struct Watched {
    inner: Arc<dyn GpuGate>,
    log: Mutex<Vec<(Stage, bool)>>,
}

impl GpuGate for Watched {
    fn check(&self, stage: Stage) -> GateDecision {
        let decision = self.inner.check(stage);
        self.log
            .lock()
            .unwrap()
            .push((stage, decision == GateDecision::Go));
        decision
    }
}

/// What the test knows about the synthetic meeting.
struct Script {
    name: String,
    source: PathBuf,
    /// `YYYY-MM-DD`.
    date: String,
    /// Speaker ids of the script, with display names.
    speakers: Vec<(String, String)>,
    duration_ms: u64,
}

impl Script {
    fn load(name: &str) -> Script {
        let source = env_path("BBB_AUDIO_DIR").join(name);
        let annotation: serde_json::Value =
            serde_json::from_slice(&std::fs::read(source.join("annotation.json")).unwrap())
                .unwrap();
        let speakers = annotation["speakers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                let id = s.as_str().unwrap().to_string();
                let mut chars = id.chars();
                let display = match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => id.clone(),
                };
                (id, display)
            })
            .collect();
        let pcm_len = read_pcm(&source.join("mixed.wav")).len() as u64;
        Script {
            name: name.to_string(),
            date: annotation["meeting_date"].as_str().unwrap().to_string(),
            source,
            speakers,
            duration_ms: pcm_len / 2 * 1000 / 16_000,
        }
    }

    fn cycle_participants(&self) -> Vec<CycleParticipant> {
        self.speakers
            .iter()
            .map(|(id, name)| CycleParticipant {
                id: id.clone(),
                name: name.clone(),
                github: None,
                discord: None,
            })
            .collect()
    }
}

fn template() -> Template {
    Template {
        name: TEMPLATE_NAME.into(),
        sections: vec![
            TemplateSection {
                id: "work".into(),
                title: "Work and tasks".into(),
                description: "What has to be done and by whom".into(),
            },
            TemplateSection {
                id: "money".into(),
                title: "Budget and decisions".into(),
                description: "Money spent or planned and what was decided".into(),
            },
        ],
        body: TEMPLATE_BODY.into(),
    }
}

fn cycle_source(script: &Script) -> Arc<dyn CycleSource> {
    let participants = script.cycle_participants();
    Arc::new(move |id: &str| -> Result<Option<CycleData>, RunError> {
        if id != CYCLE_ID {
            return Ok(None);
        }
        let mut cycle = Cycle::new(CYCLE_ID, "Nomad");
        cycle.participants = participants.clone();
        Ok(Some(CycleData {
            cycle,
            templates: vec![Template::builtin_default(), template()],
            topics: vec![],
            issues: vec![],
            closed_issue_ids: vec![],
            issue_links: vec![],
            previous_dirs: vec![],
        }))
    })
}

struct Lab {
    root: PathBuf,
    store: Arc<Mutex<Store>>,
    sup: Supervisor,
    gate: Arc<Watched>,
    script: Script,
    /// Wall time of every stage run in this test, in order.
    timings: Vec<(Stage, Duration)>,
    /// Removes the work folder when the lab is dropped; keep it the last field.
    _guard: tempfile::TempDir,
}

fn lab(script: Script, speaker_count: u32) -> Lab {
    let model = env_path("BBB_LLM_MODEL");
    let llama = env_path("BBB_LLAMA_SERVER");
    assert!(model.is_file(), "BBB_LLM_MODEL is not a file: {model:?}");
    assert!(llama.is_file(), "BBB_LLAMA_SERVER is not a file: {llama:?}");

    let guard = tempfile::Builder::new()
        .prefix(&format!("bbb-s4-{}-", script.name))
        .tempdir()
        .unwrap();
    let root = guard.path().to_path_buf();
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));

    let worker_dir = repo_root().join("worker");
    let runner_root = root.clone();
    let runner = PipelineRunner::new(
        store.clone(),
        move || RunnerSettings {
            worker: WorkerConfig {
                uv_cache_dir: None,
                uv: PathBuf::from("uv"),
                worker_dir: worker_dir.clone(),
                env_dir: worker_dir.join(".venv"),
                models_dir: None,
                llama_server: Some(llama.clone()),
                hf_token: None,
                step_timeout: Duration::from_secs(3600),
                llm_api_key: None,
            },
            meeting_root: runner_root.clone(),
            retention: AudioRetention::Delete,
            voice: VoiceSettings::default(),
        },
        |_, _, _, _| {},
    )
    .with_cycle_source(cycle_source(&script));

    // The real GPU guard, as in production.
    let gate = Arc::new(Watched {
        inner: Arc::new(NvmlGate::new(NvmlSampler::new(), GuardPolicy::default())),
        log: Mutex::new(Vec::new()),
    });
    let sup = Supervisor::new(store.clone(), Arc::new(runner), gate.clone(), 3);

    let id = script.name.as_str();
    {
        let store = store.lock().unwrap();
        store
            .create_meeting(&Meeting {
                id: id.into(),
                title: id.into(),
                started_unix_ms: STARTED_UNIX_MS,
                duration_ms: Some(script.duration_ms as i64),
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
        let pcm = read_pcm(&script.source.join("mixed.wav"));
        for (i, chunk) in pcm.chunks(SEGMENT_SAMPLES * 2).enumerate() {
            let file = format!("meet/{i:06}.wav");
            write_wav(&root.join(id).join(&file), chunk);
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
        for (pid, name) in &script.speakers {
            store
                .create_participant_in(Some(CYCLE_ID), pid, name, 1)
                .unwrap();
        }
        store
            .set_report_settings(id, &report_settings(Some("fixed"), None))
            .unwrap();
    }
    let options = serde_json::to_string(&JobOptions {
        max_speakers: Some(speaker_count),
        llm: Some(LlmOptions {
            kind: LlmKind::Managed,
            model_path: Some(model.to_string_lossy().into_owned()),
            ctx: Some(CTX),
            base_url: None,
            model: None,
            cloud_confirmed: false,
        }),
        ..JobOptions::default()
    })
    .unwrap();
    sup.enqueue(id, &options).unwrap();
    Lab {
        root,
        store,
        sup,
        gate,
        script,
        timings: Vec::new(),
        _guard: guard,
    }
}

fn report_settings(mode: Option<&str>, template: Option<&str>) -> ReportSettings {
    ReportSettings {
        cycle_id: Some(CYCLE_ID.into()),
        report_mode: mode.map(str::to_string),
        report_template: template.map(str::to_string),
        ..ReportSettings::default()
    }
}

impl Lab {
    fn id(&self) -> &str {
        &self.script.name
    }

    fn dir(&self) -> PathBuf {
        self.root.join(self.id())
    }

    /// Runs stages until `until` has run (or the job is finished). A stage the GPU guard
    /// holds back is waited for; a failed stage fails the test with its error.
    fn advance(&mut self, until: Option<Stage>) {
        let mut waiting_since: Option<Instant> = None;
        loop {
            let started = Instant::now();
            match self.sup.step(self.id()).unwrap() {
                StepOutcome::Ran { stage } => {
                    waiting_since = None;
                    let took = started.elapsed();
                    println!(
                        "  stage {:<14} ran in {:>7.1} s",
                        stage.as_str(),
                        took.as_secs_f64()
                    );
                    self.timings.push((stage, took));
                    if until == Some(stage) {
                        return;
                    }
                }
                StepOutcome::Finished => return,
                StepOutcome::Waiting { stage, reason, .. } => {
                    let since = *waiting_since.get_or_insert_with(|| {
                        println!("  stage {:<14} waits: {reason}", stage.as_str());
                        Instant::now()
                    });
                    assert!(
                        since.elapsed() < GATE_PATIENCE,
                        "the GPU guard held {} back for too long: {reason}",
                        stage.as_str()
                    );
                    std::thread::sleep(Duration::from_secs(5));
                }
                StepOutcome::Failed { stage, error } => {
                    panic!("stage {} failed: {error}", stage.as_str())
                }
                other => panic!("unexpected outcome {other:?}"),
            }
        }
    }

    /// The user names every voice after the reference (what the naming screen does).
    fn name_voices(&self) {
        let truth = truth_of_voices(&self.root, self.id(), &self.script.source);
        assert!(!truth.is_empty(), "no voice was detected");
        let store = self.store.lock().unwrap();
        let known: BTreeSet<&str> = self.script.speakers.iter().map(|s| s.0.as_str()).collect();
        for (voice_id, speaker) in &truth {
            assert!(
                known.contains(speaker.as_str()),
                "{speaker} is not in the script"
            );
            let changed =
                voices::assign_voice(&store, self.id(), voice_id, Some(speaker), 10).unwrap();
            assert!(changed, "voice {voice_id} was not found");
            println!("  voice {voice_id} named {speaker}");
        }
    }

    /// Runs `extract` and `reduce` again with other report settings.
    fn rerun_llm_stages(&mut self, mode: &str, template: Option<&str>) {
        {
            let store = self.store.lock().unwrap();
            store
                .set_report_settings(self.id(), &report_settings(Some(mode), template))
                .unwrap();
            for stage in [Stage::Extract, Stage::Reduce] {
                store
                    .set_stage_status(self.id(), stage.as_str(), StageStatus::Pending, None, 20)
                    .unwrap();
            }
        }
        println!("report mode {mode}:");
        self.advance(None);
    }

    fn stage_rows(&self) -> Vec<store::StageRow> {
        self.store
            .lock()
            .unwrap()
            .get_job(self.id())
            .unwrap()
            .unwrap()
            .stages
    }

    fn assert_all_stages_done(&self) {
        let rows = self.stage_rows();
        for stage in STAGES {
            let row = rows
                .iter()
                .find(|r| r.stage == stage.as_str())
                .unwrap_or_else(|| panic!("no row for {}", stage.as_str()));
            assert_eq!(row.status, StageStatus::Done, "{}: {row:?}", stage.as_str());
        }
        // `extract` and `reduce` did real work, they were not skipped.
        for stage in [Stage::Extract, Stage::Reduce] {
            let row = rows.iter().find(|r| r.stage == stage.as_str()).unwrap();
            assert_eq!(row.note, None, "{} was skipped", stage.as_str());
        }
    }

    fn load_draft(&self) -> Draft {
        DraftFiles::new(&self.dir()).load().unwrap()
    }
}

/// Checks the draft that `reduce` left in `work/draft.json`.
fn check_draft(lab: &Lab, mode: ReportMode, lang: Language) -> Draft {
    // The file itself validates against `schemas/meeting.schema.json`.
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(lab.dir().join("work/draft.json")).unwrap()).unwrap();
    let issues = cycle_store::schema::validate(Schema::Meeting, &raw);
    assert!(
        issues.is_empty(),
        "draft.json does not validate: {issues:?}"
    );
    let draft = lab.load_draft();

    assert_eq!(draft.meeting_id, lab.id());
    assert_eq!(draft.cycle, CYCLE_ID);
    assert_eq!(draft.date, lab.script.date);
    assert_eq!(draft.language, lang, "language");
    assert_eq!(draft.mode, mode, "mode");

    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
    for item in &draft.items {
        *by_type.entry(format!("{:?}", item.item_type)).or_default() += 1;
    }
    println!(
        "  draft: {} items {by_type:?}, {} sections, {} warnings, summary {} chars",
        draft.items.len(),
        draft.sections.len(),
        draft.warnings.len(),
        draft.summary.chars().count()
    );
    for w in &draft.warnings {
        println!("  warning {:?}: {}", w.code, w.message);
    }
    assert!(
        draft.items.len() >= 4,
        "only {} items were found",
        draft.items.len()
    );
    assert!(
        draft.items.iter().any(|i| i.item_type == ItemType::Task
            && i.source.as_ref().is_some_and(|s| s.verified)),
        "no task has a verified quote"
    );

    // Every source lies inside the meeting (a second of slack for the end of the audio).
    let end = lab.script.duration_ms + 1_000;
    let ids: BTreeSet<&str> = lab.script.speakers.iter().map(|s| s.0.as_str()).collect();
    for item in &draft.items {
        if let Some(source) = &item.source {
            assert!(
                source.t_start_ms <= source.t_end_ms && source.t_end_ms <= end,
                "item {} has the time {}..{} ms in a meeting of {} ms",
                item.id,
                source.t_start_ms,
                source.t_end_ms,
                lab.script.duration_ms
            );
        }
        if let Some(who) = &item.assignee {
            assert!(
                ids.contains(who.as_str()),
                "item {} is assigned to {who}, who is not a participant",
                item.id
            );
        }
    }

    // The named transcript carries the participants' names.
    let named: serde_json::Value = serde_json::from_slice(
        &std::fs::read(lab.dir().join("work/transcript.named.json")).unwrap(),
    )
    .unwrap();
    let labels: BTreeSet<&str> = named["speakers"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["label"].as_str())
        .collect();
    let names: BTreeSet<&str> = lab.script.speakers.iter().map(|s| s.1.as_str()).collect();
    assert!(
        labels.intersection(&names).count() >= 2,
        "speaker labels {labels:?} do not carry the participants' names {names:?}"
    );

    // The deterministic rendering gives a report.
    let body = if mode == ReportMode::Template {
        TEMPLATE_BODY.to_string()
    } else {
        Template::builtin_default().body
    };
    let report = draft::render_report(
        &draft,
        &lab.script.cycle_participants(),
        &body,
        lang,
        Variant::Markdown,
    )
    .unwrap();
    assert!(!report.trim().is_empty(), "the report is empty");
    assert!(
        report.contains(&draft::default_title(&draft, lang)),
        "the report has no title:\n{report}"
    );
    assert!(
        report.contains("- [ ]"),
        "the report has no task:\n{report}"
    );
    println!("  report: {} lines", report.lines().count());
    draft
}

/// Draft operations on the real draft of a template-mode meeting.
fn check_draft_operations(lab: &Lab, draft: &Draft) {
    assert_eq!(draft.template.as_deref(), Some(TEMPLATE_NAME));
    let section_ids: Vec<&str> = draft.sections.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        section_ids,
        ["work", "money"],
        "sections come from the template"
    );
    assert!(!draft::has_edited_items(draft), "nothing was edited yet");

    let files = DraftFiles::new(&lab.dir());
    let target = &draft.items[0];
    let other = if target.section.as_deref() == Some("money") {
        "work"
    } else {
        "money"
    };

    // Move an item to the other section.
    files
        .edit(|d| draft::move_item(d, &target.id, Some(other), None))
        .unwrap();
    let after = files.load().unwrap();
    let moved = after.items.iter().find(|i| i.id == target.id).unwrap();
    assert_eq!(moved.section.as_deref(), Some(other));
    assert!(moved.edited);
    assert_eq!(after.items.len(), draft.items.len());
    assert!(draft::has_edited_items(&after));
    assert!(files.has_user_edits().unwrap());

    // Change the text of another item.
    let second = draft.items[1].id.clone();
    files
        .edit(|d| {
            draft::update_item(
                d,
                &second,
                &ItemPatch {
                    text: Some("Edited by the integration test".into()),
                    ..ItemPatch::default()
                },
            )
        })
        .unwrap();
    let after = files.load().unwrap();
    let changed = after.items.iter().find(|i| i.id == second).unwrap();
    assert_eq!(changed.text, "Edited by the integration test");
    assert!(changed.edited);

    // The edited draft still renders, with the moved item under its new section.
    let report = draft::render_report(
        &after,
        &lab.script.cycle_participants(),
        TEMPLATE_BODY,
        Language::En,
        Variant::Chat,
    )
    .unwrap();
    assert!(
        report.contains("Edited by the integration test"),
        "{report}"
    );
    assert!(report.contains("Budget and decisions"), "{report}");
    assert!(report.contains("Work and tasks"), "{report}");
    println!(
        "  draft operations: moved {} to {other}, edited {second}",
        target.id
    );
}

fn print_timings(lab: &Lab) {
    println!("--- {} ---", lab.id());
    println!("stages of the first run (wall time):");
    for (stage, took) in &lab.timings {
        println!("  {:<14} {:>7.1} s", stage.as_str(), took.as_secs_f64());
    }
    let total: f64 = lab.timings.iter().map(|t| t.1.as_secs_f64()).sum();
    println!(
        "  total          {total:>7.1} s for {:.1} s of audio",
        lab.script.duration_ms as f64 / 1000.0
    );
    println!("work folder: {}", lab.root.display());
}

fn run_meeting(name: &str, lang: Language, with_auto_mode: bool) {
    let script = Script::load(name);
    let speaker_count = script.speakers.len() as u32;
    let mut lab = lab(script, speaker_count);
    println!(
        "{name}: {} speakers, {} ms",
        speaker_count, lab.script.duration_ms
    );

    // Up to and including `clips`; then the user names the voices, as after a real run.
    lab.advance(Some(Stage::Clips));
    lab.name_voices();
    println!("first run (report mode fixed):");
    lab.advance(None);
    assert_eq!(lab.sup.step(lab.id()).unwrap(), StepOutcome::Finished);
    lab.assert_all_stages_done();
    // Audio is gone after processing, as the settings say; the work files stay.
    assert!(!lab.dir().join("meet").exists());
    assert!(lab.dir().join("work/context.json").exists());
    check_draft(&lab, ReportMode::Fixed, lang);
    print_timings(&lab);

    if with_auto_mode {
        lab.rerun_llm_stages("auto", None);
        lab.assert_all_stages_done();
        check_draft(&lab, ReportMode::Auto, lang);
    }

    lab.rerun_llm_stages("template", Some(TEMPLATE_NAME));
    lab.assert_all_stages_done();
    let draft = check_draft(&lab, ReportMode::Template, lang);
    check_draft_operations(&lab, &draft);

    // The GPU guard was asked for both language-model stages and let them run.
    let log = lab.gate.log.lock().unwrap();
    for stage in [Stage::Extract, Stage::Reduce] {
        assert!(
            log.iter().any(|(s, go)| *s == stage && *go),
            "the GPU guard never let {} run",
            stage.as_str()
        );
    }
    let waits = log.iter().filter(|(_, go)| !go).count();
    println!("GPU guard: {} questions, {waits} waits", log.len());
}

#[test]
#[ignore = "needs real models, a language model and a GPU; takes several minutes"]
fn a_polish_meeting_of_a_cycle_goes_through_all_nine_stages_to_a_valid_draft() {
    run_meeting("pl-03", Language::Pl, true);
}

#[test]
#[ignore = "needs real models, a language model and a GPU; takes several minutes"]
fn an_english_meeting_of_a_cycle_goes_through_all_nine_stages_to_a_valid_draft() {
    run_meeting("en-03", Language::En, false);
}
