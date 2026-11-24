//! The step before `extract`, done by the application: it writes what the worker needs to
//! know about the cycle and the speakers.
//!
//! * `work/context.json` (`schemas/cycle-context.schema.json`), built by
//!   [`cycle_store::build_context`] from the cycle, the meeting's report settings in the
//!   store and the items of the cycle's earlier meetings.
//! * `work/transcript.named.json`: `transcript.json` with the generic speaker labels
//!   replaced by the participants' names. Speaker ids and everything else are unchanged,
//!   so quotes and times stay valid. The user's edits stay in `edits.json`, which the
//!   worker reads itself.
//!
//! The queue does not know where cycle repositories are: the application supplies a
//! [`CycleSource`] that returns the loaded cycle, its templates, topics and cached issues.
//!
//! # Earlier meetings
//!
//! [`cycle_store::build_context`] reads `<dir>/<folder>/meeting.json` for every directory in
//! [`CycleData::previous_dirs`]. Two places can hold earlier meetings of the cycle:
//!
//! * the cycle repository (`meetings/<ID>`): the source puts that directory in
//!   `previous_dirs`;
//! * this application's own meetings folder, where a finished draft lives at
//!   `<meetings dir>/<meeting id>/work/draft.json` (it is published to the repository only
//!   in Stage 5). [`stage_local_drafts`] copies those drafts into
//!   `<this meeting>/work/context-previous/<meeting id>/meeting.json`, a directory in the
//!   shape the context builder reads, and [`prepare_context`] adds it to the list. The
//!   copy is rebuilt on every run, so edits to an earlier draft are picked up.
//!
//! A meeting present in both places is used once (the builder skips repeated meeting ids).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use cycle_store::time::format_utc_date;
use cycle_store::{
    build_context, ContextRequest, Cycle, IssueLink, LanguageSetting, OpenIssue, ReportMode,
    Template, Topic,
};
use store::Store;

use crate::supervisor::RunError;
use crate::transcript::{compose, Edits, RawTranscript};
use crate::voices::{apply_names, speaker_names};

/// The context, written for the worker.
pub const CONTEXT_FILE: &str = "context.json";
/// The transcript with participant names, written for the worker.
pub const NAMED_TRANSCRIPT_FILE: &str = "transcript.named.json";
/// Folder under `work/` that holds copies of the cycle's earlier local drafts.
pub const PREVIOUS_DIR: &str = "context-previous";

/// Everything about a cycle that the context needs. Read by the application from its
/// registered cycle repository.
pub struct CycleData {
    pub cycle: Cycle,
    /// The cycle's templates (needed for template and by-type mode).
    pub templates: Vec<Template>,
    /// All topics of the cycle, open and closed.
    pub topics: Vec<Topic>,
    /// Open issues from the offline cache.
    pub issues: Vec<OpenIssue>,
    /// Numbers of issues known to be closed.
    pub closed_issue_ids: Vec<String>,
    /// The issues created from items of earlier meetings (open and closed).
    pub issue_links: Vec<IssueLink>,
    /// Directories that hold one `<folder>/meeting.json` per earlier meeting, for example
    /// `meetings/<ID>` of the repository. The application's local drafts are added
    /// separately.
    pub previous_dirs: Vec<PathBuf>,
}

/// Supplies the data of a cycle by its id. `Ok(None)` means the cycle is not available
/// (its repository is not registered or was removed).
pub trait CycleSource: Send + Sync {
    fn load(&self, cycle_id: &str) -> Result<Option<CycleData>, RunError>;
}

impl<F> CycleSource for F
where
    F: Fn(&str) -> Result<Option<CycleData>, RunError> + Send + Sync,
{
    fn load(&self, cycle_id: &str) -> Result<Option<CycleData>, RunError> {
        self(cycle_id)
    }
}

/// What [`prepare_context`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextOutcome {
    /// The meeting has no cycle; nothing was written.
    NoCycle,
    Written {
        /// Problems found on the way (an earlier meeting that could not be read, a
        /// template that was replaced by the default).
        warnings: Vec<String>,
        /// Earlier local drafts handed to the builder.
        local_drafts: usize,
        /// Whether `transcript.named.json` was written (it needs `transcript.json`).
        named_transcript: bool,
    },
}

/// Inputs of [`prepare_context`] that do not come from the store or the cycle.
pub struct ContextEnv<'a> {
    /// The recordings root: meeting folders are relative to it.
    pub meeting_root: &'a Path,
    /// Offset of the user's time zone from UTC in minutes, to date a meeting by the local
    /// calendar day it started on.
    pub utc_offset_minutes: i32,
}

fn err(e: impl std::fmt::Display) -> RunError {
    RunError::new(e.to_string())
}

/// `YYYY-MM-DD` of a start time in the user's time zone.
pub fn local_date(started_unix_ms: i64, utc_offset_minutes: i32) -> String {
    let shifted = started_unix_ms.saturating_add(i64::from(utc_offset_minutes) * 60_000);
    format_utc_date(shifted.max(0) as u64)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), RunError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(err)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, bytes).map_err(err)?;
    fs::rename(&tmp, path).map_err(err)
}

/// Builds `work/context.json` and `work/transcript.named.json` for a meeting.
///
/// Returns [`ContextOutcome::NoCycle`] without touching anything when the meeting has no
/// cycle. Fails when the cycle is not available, the report settings do not fit the cycle
/// (a template mode without the template) or a file cannot be written.
pub fn prepare_context(
    store: &Store,
    source: &dyn CycleSource,
    env: &ContextEnv<'_>,
    meeting_id: &str,
) -> Result<ContextOutcome, RunError> {
    let Some(cycle_id) = cycle_id_of(store, meeting_id)? else {
        return Ok(ContextOutcome::NoCycle);
    };
    let data = load_cycle(source, &cycle_id)?;
    prepare_context_with(store, &data, env, meeting_id)
}

/// The cycle a meeting belongs to, if any. A quick read, so the caller can fetch the cycle
/// (which touches the disk and Git) without holding the store.
pub fn cycle_id_of(store: &Store, meeting_id: &str) -> Result<Option<String>, RunError> {
    Ok(store
        .get_report_settings(meeting_id)
        .map_err(err)?
        .and_then(|s| s.cycle_id))
}

/// Loads a cycle from `source`; a cycle that is not available is an error with a clear text.
pub fn load_cycle(source: &dyn CycleSource, cycle_id: &str) -> Result<CycleData, RunError> {
    source.load(cycle_id)?.ok_or_else(|| {
        RunError::new(format!(
            "the cycle {cycle_id} is not available: its repository is not registered"
        ))
    })
}

/// [`prepare_context`] with the cycle already loaded (see [`load_cycle`]), so the work that
/// needs the store does not wait for the repository.
pub fn prepare_context_with(
    store: &Store,
    data: &CycleData,
    env: &ContextEnv<'_>,
    meeting_id: &str,
) -> Result<ContextOutcome, RunError> {
    let details = store
        .get_meeting(meeting_id)
        .map_err(err)?
        .ok_or_else(|| RunError::new("the meeting no longer exists"))?;
    let settings = store
        .get_report_settings(meeting_id)
        .map_err(err)?
        .unwrap_or_default();
    let Some(cycle_id) = settings.cycle_id.as_deref() else {
        return Ok(ContextOutcome::NoCycle);
    };

    let mode = settings
        .report_mode
        .as_deref()
        .map(|m| parse::<ReportMode>(m, "report mode"))
        .transpose()?;
    let output_language = settings
        .output_language
        .as_deref()
        .map(|l| parse::<LanguageSetting>(l, "output language"))
        .transpose()?;

    let meeting_dir = env.meeting_root.join(&details.meeting.dir);
    let work = meeting_dir.join("work");
    let previous = work.join(PREVIOUS_DIR);
    let local_drafts =
        stage_local_drafts(store, cycle_id, env.meeting_root, meeting_id, &previous)?;
    let mut previous_dirs = data.previous_dirs.clone();
    previous_dirs.push(previous);

    let date = local_date(details.meeting.started_unix_ms, env.utc_offset_minutes);
    let mut req = ContextRequest::new(&data.cycle, &date);
    req.templates = &data.templates;
    req.topics = &data.topics;
    req.issues = &data.issues;
    req.closed_issue_ids = &data.closed_issue_ids;
    req.issue_links = &data.issue_links;
    req.mode = mode;
    req.meeting_type = settings.meeting_type.as_deref();
    req.template = settings.report_template.as_deref();
    // The meeting's own choice wins over the cycle's default.
    req.output_language = output_language;
    req.previous_dirs = &previous_dirs;
    req.exclude_meeting_id = Some(meeting_id);
    let context = build_context(&req).map_err(|e| RunError::new(format!("context: {e}")))?;

    write_atomic(
        &work.join(CONTEXT_FILE),
        &serde_json::to_vec_pretty(&context.value).map_err(err)?,
    )?;
    let named_transcript = write_named_transcript(store, meeting_id, &meeting_dir)?;
    Ok(ContextOutcome::Written {
        warnings: context.warnings,
        local_drafts,
        named_transcript,
    })
}

fn parse<T: serde::de::DeserializeOwned>(value: &str, what: &str) -> Result<T, RunError> {
    serde_json::from_value(serde_json::Value::String(value.to_string()))
        .map_err(|_| RunError::new(format!("unknown {what} {value:?}")))
}

/// Copies the finished drafts of the cycle's other meetings from this application's
/// meetings folder into `target`, one `<meeting id>/meeting.json` each, which is the shape
/// [`cycle_store::build_context`] reads. `target` is emptied first. Meetings without a
/// `work/draft.json` are left out; a meeting's `work/corrections.json` goes along as
/// `corrections.json`. Returns the number of drafts copied.
pub fn stage_local_drafts(
    store: &Store,
    cycle_id: &str,
    meeting_root: &Path,
    exclude_meeting_id: &str,
    target: &Path,
) -> Result<usize, RunError> {
    match fs::remove_dir_all(target) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(err(e)),
    }
    let mut copied = 0;
    for meeting in store.list_meetings_by_cycle(cycle_id).map_err(err)? {
        if meeting.id == exclude_meeting_id {
            continue;
        }
        let draft = meeting_root
            .join(&meeting.dir)
            .join("work")
            .join(crate::draft::DRAFT_FILE);
        if !draft.is_file() {
            continue;
        }
        let folder = target.join(&meeting.id);
        fs::create_dir_all(&folder).map_err(err)?;
        fs::copy(&draft, folder.join("meeting.json")).map_err(err)?;
        // The corrections written after the meeting was published (published or not yet).
        let corrections = meeting_root
            .join(&meeting.dir)
            .join("work")
            .join(cycle_store::corrections::CORRECTIONS_FILE);
        if corrections.is_file() {
            fs::copy(
                &corrections,
                folder.join(cycle_store::corrections::CORRECTIONS_FILE),
            )
            .map_err(err)?;
        }
        copied += 1;
    }
    Ok(copied)
}

/// Writes `work/transcript.named.json`: the worker's `transcript.json` with every speaker
/// label that has a participant's name replaced by it (an automatic match or the user's
/// choice; a voice merged into another one takes that one's label). Returns `false` when
/// there is no transcript yet.
pub fn write_named_transcript(
    store: &Store,
    meeting_id: &str,
    meeting_dir: &Path,
) -> Result<bool, RunError> {
    let raw_path = meeting_dir.join("work").join("transcript.json");
    let bytes = match fs::read(&raw_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(err(e)),
    };
    let mut raw: RawTranscript = serde_json::from_slice(&bytes).map_err(err)?;
    let voices = store.list_meeting_voices(meeting_id).map_err(err)?;
    let participants = store.list_participants().map_err(err)?;
    let names = speaker_names(&voices, &participants);

    // Reuse the label rules of the transcript view (names, merged voices) and copy the
    // resulting labels back: only `label` changes, never an id.
    let mut view = compose(&raw, &Edits::default());
    apply_names(&mut view, &voices, &names);
    let labels: HashMap<&str, &str> = view
        .speakers
        .iter()
        .map(|s| (s.id.as_str(), s.label.as_str()))
        .collect();
    for speaker in &mut raw.speakers {
        if let Some(label) = labels.get(speaker.id.as_str()) {
            speaker.label = (*label).to_string();
        }
    }
    write_atomic(
        &meeting_dir.join("work").join(NAMED_TRANSCRIPT_FILE),
        &serde_json::to_vec_pretty(&raw).map_err(err)?,
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cycle_store::{Draft, Participant as CycleParticipant, Schema};
    use store::{Meeting, MeetingState, ReportSettings, VoiceDecision, VoiceRow};

    const TRANSCRIPT: &str = include_str!("../../../worker/tests/fixtures/transcript.json");
    const DRAFT: &str = include_str!("../../../worker/tests/fixtures/meeting.valid.json");

    struct Lab {
        root: tempfile::TempDir,
        store: Store,
    }

    fn lab() -> Lab {
        Lab {
            root: tempfile::tempdir().unwrap(),
            store: Store::open_in_memory().unwrap(),
        }
    }

    impl Lab {
        fn meeting(&self, id: &str, started: i64, cycle: Option<&str>) {
            self.store
                .create_meeting(&Meeting {
                    id: id.into(),
                    title: id.into(),
                    started_unix_ms: started,
                    duration_ms: None,
                    state: MeetingState::Stopped,
                    dir: id.into(),
                })
                .unwrap();
            fs::create_dir_all(self.root.path().join(id).join("work")).unwrap();
            self.store
                .set_report_settings(
                    id,
                    &ReportSettings {
                        cycle_id: cycle.map(str::to_string),
                        ..ReportSettings::default()
                    },
                )
                .unwrap();
        }

        fn work(&self, id: &str) -> PathBuf {
            self.root.path().join(id).join("work")
        }

        fn env(&self) -> ContextEnv<'_> {
            ContextEnv {
                meeting_root: self.root.path(),
                utc_offset_minutes: 0,
            }
        }
    }

    fn cycle_data(previous: Vec<PathBuf>) -> CycleData {
        let mut cycle = Cycle::new("TEAM", "Team");
        cycle.participants.push(CycleParticipant {
            id: "alex".into(),
            name: "Alex Example".into(),
            github: None,
            discord: None,
        });
        CycleData {
            cycle,
            templates: vec![Template::builtin_default()],
            topics: vec![],
            issues: vec![],
            closed_issue_ids: vec![],
            issue_links: vec![],
            previous_dirs: previous,
        }
    }

    fn source() -> impl Fn(&str) -> Result<Option<CycleData>, RunError> + Send + Sync {
        |id| Ok((id == "TEAM").then(|| cycle_data(vec![])))
    }

    fn read(path: &Path) -> serde_json::Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    // 2026-11-13 10:00 UTC
    const NOON: i64 = 1_794_564_000_000;

    #[test]
    fn a_meeting_without_a_cycle_writes_nothing() {
        let lab = lab();
        lab.meeting("m1", NOON, None);
        let out = prepare_context(&lab.store, &source(), &lab.env(), "m1").unwrap();
        assert_eq!(out, ContextOutcome::NoCycle);
        assert!(!lab.work("m1").join(CONTEXT_FILE).exists());
    }

    #[test]
    fn the_context_is_written_from_the_cycle_and_the_report_settings() {
        let lab = lab();
        lab.meeting("m1", NOON, Some("TEAM"));
        lab.store
            .set_report_settings(
                "m1",
                &ReportSettings {
                    cycle_id: Some("TEAM".into()),
                    report_mode: Some("auto".into()),
                    output_language: Some("pl".into()),
                    ..ReportSettings::default()
                },
            )
            .unwrap();
        let out = prepare_context(&lab.store, &source(), &lab.env(), "m1").unwrap();
        assert!(matches!(out, ContextOutcome::Written { .. }));
        let ctx = read(&lab.work("m1").join(CONTEXT_FILE));
        assert!(cycle_store::schema::validate(Schema::Context, &ctx).is_empty());
        assert_eq!(ctx["cycle"], "TEAM");
        assert_eq!(ctx["mode"], "auto");
        // The meeting's choice wins over the cycle's `auto`.
        assert_eq!(ctx["output_language"], "pl");
        assert_eq!(ctx["participants"][0]["id"], "alex");
        assert_eq!(ctx["meeting_date"], "2026-11-13");
    }

    #[test]
    fn the_cycle_default_applies_without_an_override() {
        let lab = lab();
        lab.meeting("m1", NOON, Some("TEAM"));
        prepare_context(&lab.store, &source(), &lab.env(), "m1").unwrap();
        let ctx = read(&lab.work("m1").join(CONTEXT_FILE));
        assert_eq!(ctx["mode"], "fixed");
        assert_eq!(ctx["output_language"], "auto");
    }

    #[test]
    fn the_meeting_is_dated_by_the_local_day() {
        // 23:30 UTC is already the next day at UTC+2.
        let late = NOON + 13 * 3_600_000 + 30 * 60_000;
        assert_eq!(local_date(late, 0), "2026-11-13");
        assert_eq!(local_date(late, 120), "2026-11-14");
        assert_eq!(local_date(late, -300), "2026-11-13");
        assert_eq!(local_date(-5, 0), "1970-01-01");
    }

    #[test]
    fn an_unavailable_cycle_is_an_error_not_a_silent_skip() {
        let lab = lab();
        lab.meeting("m1", NOON, Some("GONE"));
        let e = prepare_context(&lab.store, &source(), &lab.env(), "m1").unwrap_err();
        assert!(e.message.contains("GONE"), "{e}");
    }

    #[test]
    fn a_template_mode_without_its_template_is_an_error() {
        let lab = lab();
        lab.meeting("m1", NOON, Some("TEAM"));
        lab.store
            .set_report_settings(
                "m1",
                &ReportSettings {
                    cycle_id: Some("TEAM".into()),
                    report_mode: Some("template".into()),
                    report_template: Some("missing".into()),
                    ..ReportSettings::default()
                },
            )
            .unwrap();
        let e = prepare_context(&lab.store, &source(), &lab.env(), "m1").unwrap_err();
        assert!(e.message.contains("missing"), "{e}");
    }

    #[test]
    fn earlier_local_drafts_of_the_cycle_reach_the_context() {
        let lab = lab();
        lab.meeting("old", NOON - 7 * 86_400_000, Some("TEAM"));
        lab.meeting("other-cycle", NOON - 86_400_000, Some("ELSE"));
        lab.meeting("no-draft", NOON - 2 * 86_400_000, Some("TEAM"));
        lab.meeting("now", NOON, Some("TEAM"));
        // `old` has a finished draft; the other cycle's draft must not leak in; the
        // current meeting's own draft is never its own history.
        fs::write(lab.work("old").join("draft.json"), DRAFT).unwrap();
        fs::write(lab.work("other-cycle").join("draft.json"), DRAFT).unwrap();
        fs::write(lab.work("now").join("draft.json"), DRAFT).unwrap();

        let out = prepare_context(&lab.store, &source(), &lab.env(), "now").unwrap();
        match out {
            ContextOutcome::Written { local_drafts, .. } => assert_eq!(local_drafts, 1),
            other => panic!("{other:?}"),
        }
        let staged = lab.work("now").join(PREVIOUS_DIR);
        assert!(staged.join("old/meeting.json").is_file());
        assert!(!staged.join("other-cycle").exists());
        assert!(!staged.join("now").exists());

        let ctx = read(&lab.work("now").join(CONTEXT_FILE));
        let items = ctx["previous_items"].as_array().unwrap();
        // The fixture's draft is dated 2026-11-16 (after this meeting, so it is dropped as
        // a "future" meeting); date it before and run again.
        assert!(items.is_empty(), "{items:?}");
        let mut earlier = Draft::from_json_str(DRAFT).unwrap();
        earlier.date = "2026-11-06".into();
        earlier.meeting_id = "old".into();
        fs::write(
            lab.work("old").join("draft.json"),
            serde_json::to_vec(&earlier.to_value()).unwrap(),
        )
        .unwrap();
        prepare_context(&lab.store, &source(), &lab.env(), "now").unwrap();
        let ctx = read(&lab.work("now").join(CONTEXT_FILE));
        let texts: Vec<_> = ctx["previous_items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["text"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            texts,
            ["Prepare the monitor mount design", "Add a second arm"]
        );
        assert_eq!(ctx["previous_items"][0]["meeting"], "2026-11-06");
    }

    #[test]
    fn staging_replaces_an_earlier_copy() {
        let lab = lab();
        lab.meeting("old", NOON - 86_400_000, Some("TEAM"));
        lab.meeting("now", NOON, Some("TEAM"));
        let target = lab.work("now").join(PREVIOUS_DIR);
        fs::create_dir_all(target.join("stale")).unwrap();
        fs::write(target.join("stale/meeting.json"), "{}").unwrap();
        fs::write(lab.work("old").join("draft.json"), DRAFT).unwrap();
        let n = stage_local_drafts(&lab.store, "TEAM", lab.root.path(), "now", &target).unwrap();
        assert_eq!(n, 1);
        assert!(!target.join("stale").exists());
        assert!(target.join("old/meeting.json").is_file());
    }

    #[test]
    fn the_corrections_of_an_earlier_meeting_reach_the_context() {
        let lab = lab();
        lab.meeting("old", NOON - 86_400_000, Some("TEAM"));
        lab.meeting("now", NOON, Some("TEAM"));
        let mut earlier = Draft::from_json_str(DRAFT).unwrap();
        earlier.date = "2026-11-06".into();
        fs::write(
            lab.work("old").join("draft.json"),
            serde_json::to_vec(&earlier.to_value()).unwrap(),
        )
        .unwrap();
        let mut file = cycle_store::CorrectionsFile::new(&earlier.meeting_id);
        file.corrections.push(cycle_store::Correction {
            id: "c1".into(),
            date: "2026-11-09".into(),
            text: "The mount design moved to next week.".into(),
            item: None,
            issue: None,
        });
        file.write(
            &lab.work("old")
                .join(cycle_store::corrections::CORRECTIONS_FILE),
        )
        .unwrap();
        let outcome = prepare_context(&lab.store, &source(), &lab.env(), "now").unwrap();
        let ctx = read(&lab.work("now").join(CONTEXT_FILE));
        assert_eq!(
            ctx["corrections"][0]["text"], "The mount design moved to next week.",
            "{outcome:?}"
        );
        assert_eq!(ctx["corrections"][0]["meeting"], "2026-11-06");
        // Without the file the key is absent.
        fs::remove_file(
            lab.work("old")
                .join(cycle_store::corrections::CORRECTIONS_FILE),
        )
        .unwrap();
        prepare_context(&lab.store, &source(), &lab.env(), "now").unwrap();
        let ctx = read(&lab.work("now").join(CONTEXT_FILE));
        assert!(ctx.get("corrections").is_none());
    }

    #[test]
    fn the_named_transcript_carries_participant_names_and_unchanged_ids() {
        let lab = lab();
        lab.meeting("m1", NOON, Some("TEAM"));
        fs::write(lab.work("m1").join("transcript.json"), TRANSCRIPT).unwrap();
        let raw: RawTranscript = serde_json::from_str(TRANSCRIPT).unwrap();
        let named_speaker = raw
            .speakers
            .iter()
            .find(|s| s.id.contains(':'))
            .expect("the fixture has a diarized speaker")
            .clone();

        lab.store
            .create_participant("p1", "Alex Example", 1)
            .unwrap();
        lab.store
            .put_meeting_voices(
                "m1",
                &[VoiceRow {
                    voice_id: named_speaker.id.clone(),
                    track: named_speaker.track.clone().unwrap_or_default(),
                    label: named_speaker.label.clone(),
                    embedding: vec![1.0],
                    speech_ms: 9_000,
                    decision: VoiceDecision::Unknown,
                    score: None,
                    matched_participant: None,
                    assigned_participant: None,
                    confirmed: false,
                    merged_into: None,
                }],
            )
            .unwrap();
        lab.store
            .set_voice_assignment("m1", &named_speaker.id, Some("p1"))
            .unwrap();

        assert!(write_named_transcript(&lab.store, "m1", &lab.root.path().join("m1")).unwrap());
        let named: RawTranscript =
            serde_json::from_slice(&fs::read(lab.work("m1").join(NAMED_TRANSCRIPT_FILE)).unwrap())
                .unwrap();
        // Same ids, same segments; only the label of the named voice differs.
        assert_eq!(named.segments, raw.segments);
        let ids = |t: &RawTranscript| t.speakers.iter().map(|s| s.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&named), ids(&raw));
        for (a, b) in named.speakers.iter().zip(&raw.speakers) {
            if a.id == named_speaker.id {
                assert_eq!(a.label, "Alex Example");
            } else {
                assert_eq!(a.label, b.label);
            }
            assert_eq!((&a.kind, &a.track), (&b.kind, &b.track));
        }
    }

    /// A meeting of TEAM whose diarized voice is stored but not named yet; returns the voice id.
    fn meeting_with_an_unnamed_voice(lab: &Lab) -> String {
        lab.meeting("m1", NOON, Some("TEAM"));
        fs::write(lab.work("m1").join("transcript.json"), TRANSCRIPT).unwrap();
        let raw: RawTranscript = serde_json::from_str(TRANSCRIPT).unwrap();
        let voice = raw
            .speakers
            .iter()
            .find(|s| s.id.contains(':'))
            .expect("the fixture has a diarized speaker")
            .clone();
        lab.store
            .put_meeting_voices(
                "m1",
                &[VoiceRow {
                    voice_id: voice.id.clone(),
                    track: voice.track.clone().unwrap_or_default(),
                    label: "SPEAKER_00".into(),
                    embedding: vec![1.0],
                    speech_ms: 9_000,
                    decision: VoiceDecision::Unknown,
                    score: None,
                    matched_participant: None,
                    assigned_participant: None,
                    confirmed: false,
                    merged_into: None,
                }],
            )
            .unwrap();
        voice.id
    }

    fn named_labels(lab: &Lab) -> Vec<(String, String)> {
        let named: RawTranscript =
            serde_json::from_slice(&fs::read(lab.work("m1").join(NAMED_TRANSCRIPT_FILE)).unwrap())
                .unwrap();
        named
            .speakers
            .into_iter()
            .map(|s| (s.id, s.label))
            .collect()
    }

    #[test]
    fn a_voice_the_user_named_reaches_the_model_even_when_the_cycle_does_not_list_the_person() {
        let lab = lab();
        let voice = meeting_with_an_unnamed_voice(&lab);
        // Created by the Voices screen: a participant of the store that cycle.yaml does not list.
        lab.store
            .create_participant("team:radek-wojcik", "Radek Wójcik", 1)
            .unwrap();
        lab.store
            .set_voice_assignment("m1", &voice, Some("team:radek-wojcik"))
            .unwrap();

        let out = prepare_context(&lab.store, &source(), &lab.env(), "m1").unwrap();
        assert!(matches!(
            out,
            ContextOutcome::Written {
                named_transcript: true,
                ..
            }
        ));
        // The model reads the name on the lines; the participant list is the cycle's own.
        let labels = named_labels(&lab);
        assert!(
            labels.contains(&(voice.clone(), "Radek Wójcik".into())),
            "{labels:?}"
        );
        assert!(
            labels.contains(&("user".into(), "You".into())),
            "{labels:?}"
        );
        let ctx = read(&lab.work("m1").join(CONTEXT_FILE));
        let ids: Vec<&str> = ctx["participants"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["alex"]);
    }

    #[test]
    fn an_unnamed_voice_stays_a_generic_label_and_naming_it_later_is_picked_up() {
        let lab = lab();
        let voice = meeting_with_an_unnamed_voice(&lab);
        let dir = lab.root.path().join("m1");

        // Before the user names it: the generic, language-neutral label, never a role.
        assert!(write_named_transcript(&lab.store, "m1", &dir).unwrap());
        let before = named_labels(&lab);
        assert!(
            before.contains(&(voice.clone(), "Speaker 1".into())),
            "{before:?}"
        );

        // The Voices screen names the voice; the next run of the extraction writes the name.
        lab.store
            .create_participant("team:radek-wojcik", "Radek Wójcik", 2)
            .unwrap();
        lab.store
            .set_voice_assignment("m1", &voice, Some("team:radek-wojcik"))
            .unwrap();
        assert!(write_named_transcript(&lab.store, "m1", &dir).unwrap());
        let after = named_labels(&lab);
        assert!(after.contains(&(voice, "Radek Wójcik".into())), "{after:?}");
        assert_eq!(after.len(), before.len());
    }

    #[test]
    fn without_a_transcript_there_is_no_named_transcript() {
        let lab = lab();
        lab.meeting("m1", NOON, Some("TEAM"));
        assert!(!write_named_transcript(&lab.store, "m1", &lab.root.path().join("m1")).unwrap());
        // The context is still written.
        let out = prepare_context(&lab.store, &source(), &lab.env(), "m1").unwrap();
        assert!(matches!(
            out,
            ContextOutcome::Written {
                named_transcript: false,
                ..
            }
        ));
    }
}
