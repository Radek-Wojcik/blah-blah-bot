//! The `speaker_id` and `clips` stages and the rules for naming voices.
//!
//! Everything here runs in the application process on local files and the local database.
//! No embedding or clip is sent anywhere.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use speakers::{
    assign_unique, select_clips_with, AddOutcome, Candidate, ClipParams, ClipSpan, Decision,
    ProfileRef, Sample, SampleSet, Thresholds, Turn, MAX_SAMPLES,
};
use store::{ClipRow, ParticipantInfo, SampleRow, Store, VoiceDecision, VoiceRow};

use crate::supervisor::RunError;
use crate::transcript::{RawTranscript, TranscriptFiles, TranscriptView};

/// Sample rate of all recorded and processed audio.
const SAMPLE_RATE: u64 = 16_000;

/// Settings of both stages, read before every stage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceSettings {
    pub thresholds: Thresholds,
    pub clips: ClipParams,
    /// Clips of a meeting whose voices were never confirmed are deleted after this many days.
    pub clip_max_days: u32,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            thresholds: Thresholds::default(),
            clips: ClipParams::default(),
            clip_max_days: 30,
        }
    }
}

fn err(e: impl std::fmt::Display) -> RunError {
    RunError::new(e.to_string())
}

/// `work/diarize/<track>.json` as the worker writes it.
#[derive(Debug, Deserialize)]
struct DiarizeDoc {
    #[serde(default)]
    speakers: Vec<String>,
    #[serde(default)]
    turns: Vec<DiarizeTurn>,
    /// A voice whose embedding was not finite has `null` values, which count as no embedding.
    #[serde(default)]
    embeddings: Vec<Vec<Option<f32>>>,
}

impl DiarizeDoc {
    /// The embedding of voice `i`, or `None` when it is missing, empty or not finite.
    fn embedding(&self, i: usize) -> Option<Vec<f32>> {
        let row = self.embeddings.get(i)?;
        let values: Option<Vec<f32>> = row.iter().copied().collect();
        values.filter(|v| !v.is_empty() && v.iter().all(|x| x.is_finite()))
    }
}

#[derive(Debug, Deserialize)]
struct DiarizeTurn {
    start_ms: u64,
    end_ms: u64,
    speaker: String,
}

fn read_diarization(meeting_dir: &Path, track: &str) -> Result<Option<DiarizeDoc>, RunError> {
    let path = meeting_dir
        .join("work")
        .join("diarize")
        .join(format!("{track}.json"));
    match fs::read_to_string(&path) {
        // Older worker versions wrote `NaN` for a voice with too little speech, which is not
        // valid JSON; read it as `null`.
        Ok(text) => Ok(Some(
            serde_json::from_str(
                &text
                    .replace("-Infinity", "null")
                    .replace("Infinity", "null")
                    .replace("NaN", "null"),
            )
            .map_err(err)?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(err(e)),
    }
}

/// Id of a detected voice; equals the speaker id in the transcript.
pub fn voice_id(track: &str, label: &str) -> String {
    format!("{track}:{label}")
}

/// Profiles ready for matching: every participant's, whatever the cycle.
pub fn load_profiles(store: &Store) -> Result<Vec<ProfileRef>, store::StoreError> {
    profile_refs(store.all_profiles()?)
}

/// Profiles a meeting of `cycle_id` is matched against: those of the cycle's participants
/// and no others. `None` is a meeting without a cycle, which uses the legacy local list.
pub fn load_profiles_for(
    store: &Store,
    cycle_id: Option<&str>,
) -> Result<Vec<ProfileRef>, store::StoreError> {
    profile_refs(store.profiles_for(cycle_id)?)
}

fn profile_refs(
    profiles: Vec<(String, Vec<SampleRow>)>,
) -> Result<Vec<ProfileRef>, store::StoreError> {
    let mut out = Vec::new();
    for (participant_id, rows) in profiles {
        let set = SampleSet::new(rows.into_iter().map(to_sample).collect());
        if let Some(centroid) = set.centroid() {
            out.push(ProfileRef {
                participant_id,
                centroid,
            });
        }
    }
    Ok(out)
}

fn to_sample(r: SampleRow) -> Sample {
    Sample {
        key: r.source_key,
        embedding: r.embedding,
        speech_ms: r.speech_ms,
    }
}

/// Stage `speaker_id`: one embedding per detected voice, matched against the profiles.
///
/// A match at or above the accept threshold and clearly ahead of the runner-up names the
/// voice automatically; a weaker one becomes a suggestion; everything else stays unknown.
/// The user's earlier decisions are kept when the stage is repeated.
pub fn run_speaker_id(
    store: &Store,
    meeting_id: &str,
    meeting_dir: &Path,
    settings: &VoiceSettings,
) -> Result<usize, RunError> {
    let details = store
        .get_meeting(meeting_id)
        .map_err(err)?
        .ok_or_else(|| RunError::new("the meeting no longer exists"))?;
    // A meeting is matched against the voices of its own cycle only.
    let cycle_id = store
        .get_report_settings(meeting_id)
        .map_err(err)?
        .and_then(|s| s.cycle_id);
    let profiles = load_profiles_for(store, cycle_id.as_deref()).map_err(err)?;

    let mut rows: Vec<VoiceRow> = Vec::new();
    let mut candidates: Vec<Candidate> = Vec::new();
    for track in details.tracks.iter().filter(|t| t.source != "microphone") {
        let Some(doc) = read_diarization(meeting_dir, &track.name)? else {
            continue;
        };
        let mut speech: BTreeMap<&str, u64> = BTreeMap::new();
        for t in &doc.turns {
            *speech.entry(t.speaker.as_str()).or_default() += t.end_ms.saturating_sub(t.start_ms);
        }
        for (i, label) in doc.speakers.iter().enumerate() {
            let Some(&speech_ms) = speech.get(label.as_str()) else {
                continue;
            };
            let embedding = doc.embedding(i).unwrap_or_default();
            let id = voice_id(&track.name, label);
            if !embedding.is_empty() {
                candidates.push(Candidate {
                    voice_id: id.clone(),
                    embedding: embedding.clone(),
                });
            }
            rows.push(VoiceRow {
                voice_id: id,
                track: track.name.clone(),
                label: label.clone(),
                embedding,
                speech_ms,
                decision: VoiceDecision::Unknown,
                score: None,
                matched_participant: None,
                assigned_participant: None,
                confirmed: false,
                merged_into: None,
            });
        }
    }

    let decisions: HashMap<String, Decision> =
        assign_unique(&candidates, &profiles, &settings.thresholds)
            .into_iter()
            .collect();
    for row in &mut rows {
        match decisions.get(&row.voice_id) {
            Some(Decision::Auto {
                participant_id,
                score,
            }) => {
                row.decision = VoiceDecision::Auto;
                row.score = Some(*score);
                row.matched_participant = Some(participant_id.clone());
                row.assigned_participant = Some(participant_id.clone());
            }
            Some(Decision::Suggest {
                participant_id,
                score,
            }) => {
                row.decision = VoiceDecision::Suggested;
                row.score = Some(*score);
                row.matched_participant = Some(participant_id.clone());
            }
            _ => {}
        }
    }
    // Keep earlier merges: they are the user's decisions.
    let earlier: HashMap<String, Option<String>> = store
        .list_meeting_voices(meeting_id)
        .map_err(err)?
        .into_iter()
        .map(|v| (v.voice_id, v.merged_into))
        .collect();
    for row in &mut rows {
        row.merged_into = earlier.get(&row.voice_id).cloned().flatten();
    }
    store.put_meeting_voices(meeting_id, &rows).map_err(err)?;
    Ok(rows.len())
}

// ----- clips -----

/// Mono 16-bit PCM of a WAV file: the `data` chunk, whatever the header size.
fn read_pcm(path: &Path) -> std::io::Result<Vec<i16>> {
    let bytes = fs::read(path)?;
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body = pos + 8;
        if id == b"data" {
            let end = (body + size).min(bytes.len());
            return Ok(bytes[body..end]
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect());
        }
        pos = body + size + (size & 1);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "no data chunk",
    ))
}

fn write_wav(path: &Path, samples: &[i16]) -> std::io::Result<()> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE as u32).to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE as u32 * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    fs::write(path, out)
}

/// Share of 20 ms frames that carry sound (louder than a tenth of the clip's loud level).
/// Synthesized and real turns alike contain pauses; a clip that is mostly pause is useless.
pub fn voiced_ratio(samples: &[i16]) -> f32 {
    let frame = (SAMPLE_RATE as usize) / 50;
    let levels: Vec<f64> = samples
        .chunks_exact(frame)
        .map(|f| {
            (f.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>() / frame as f64).sqrt()
        })
        .collect();
    if levels.is_empty() {
        return 0.0;
    }
    let mut sorted = levels.clone();
    sorted.sort_by(f64::total_cmp);
    let loud = sorted[(sorted.len() * 95 / 100).min(sorted.len() - 1)];
    let floor = (loud * 0.1).max(30.0);
    levels.iter().filter(|l| **l > floor).count() as f32 / levels.len() as f32
}

/// Minimum voiced share for a clip to be offered.
pub const MIN_VOICED_RATIO: f32 = 0.6;

/// Reads time ranges of one track from its segment files.
struct TrackAudio {
    offset_ms: f64,
    /// `(first sample index, path)` of each segment in order.
    segments: Vec<(u64, PathBuf)>,
    total: u64,
}

impl TrackAudio {
    fn new(meeting_dir: &Path, track: &store::Track, segments: &[store::Segment]) -> Self {
        let mut sorted: Vec<&store::Segment> =
            segments.iter().filter(|s| s.track == track.name).collect();
        sorted.sort_by_key(|s| s.segment);
        let mut first = 0u64;
        let mut list = Vec::new();
        for s in sorted {
            list.push((first, meeting_dir.join(&s.file)));
            first += s.samples;
        }
        Self {
            offset_ms: track.offset_ms,
            segments: list,
            total: first,
        }
    }

    /// Samples between two times on the session clock; `None` when a needed file is missing.
    fn read(&self, start_ms: u64, end_ms: u64) -> Option<Vec<i16>> {
        let to_index = |ms: u64| -> u64 {
            (((ms as f64 - self.offset_ms).max(0.0)) * SAMPLE_RATE as f64 / 1000.0) as u64
        };
        let (from, to) = (to_index(start_ms), to_index(end_ms).min(self.total));
        if to <= from {
            return None;
        }
        let mut out = Vec::with_capacity((to - from) as usize);
        for (i, (first, path)) in self.segments.iter().enumerate() {
            let next = self.segments.get(i + 1).map_or(self.total, |s| s.0);
            if next <= from || *first >= to {
                continue;
            }
            let pcm = read_pcm(path).ok()?;
            let a = from.saturating_sub(*first) as usize;
            let b = ((to - first) as usize).min(pcm.len());
            if a < b {
                out.extend_from_slice(&pcm[a..b]);
            }
        }
        (!out.is_empty()).then_some(out)
    }
}

fn words_between(raw: &RawTranscript, track: &str, start_ms: u64, end_ms: u64) -> String {
    let words: Vec<&str> = raw
        .segments
        .iter()
        .filter(|s| s.track == track && s.end_ms >= start_ms && s.start_ms <= end_ms)
        .flat_map(|s| s.words.iter())
        .filter(|w| w.start_ms >= start_ms && w.end_ms <= end_ms)
        .map(|w| w.text.trim())
        .collect();
    words.join(" ")
}

fn file_stem(voice_id: &str) -> String {
    voice_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Folder of a meeting's clips.
pub fn clips_dir(meeting_dir: &Path) -> PathBuf {
    meeting_dir.join("clips")
}

/// Stage `clips`: a few clear, non-overlapping samples of each detected voice, cut from the
/// full audio. When the audio is already gone the stage does nothing: the user then names
/// voices without listening, or by their transcript text.
pub fn run_clips(
    store: &Store,
    meeting_id: &str,
    meeting_root: &Path,
    settings: &VoiceSettings,
    now_ms: i64,
) -> Result<usize, RunError> {
    let details = store
        .get_meeting(meeting_id)
        .map_err(err)?
        .ok_or_else(|| RunError::new("the meeting no longer exists"))?;
    let meeting_dir = meeting_root.join(&details.meeting.dir);
    let raw = TranscriptFiles::new(&meeting_dir).load_raw().ok();
    let _ = remove_clip_files(store, meeting_id, meeting_root);

    let mut total = 0;
    for track in details.tracks.iter().filter(|t| t.source != "microphone") {
        let Some(doc) = read_diarization(&meeting_dir, &track.name)? else {
            continue;
        };
        let turns: Vec<Turn> = doc
            .turns
            .iter()
            .map(|t| Turn {
                start_ms: t.start_ms,
                end_ms: t.end_ms,
                speaker: t.speaker.clone(),
            })
            .collect();
        let audio = TrackAudio::new(&meeting_dir, track, &details.segments);
        let speakers: Vec<String> = doc.speakers.clone();
        let mut cache: HashMap<(u64, u64), Vec<i16>> = HashMap::new();
        let spans =
            select_clips_with(
                &turns,
                &speakers,
                &settings.clips,
                |span: &ClipSpan| match audio.read(span.start_ms, span.end_ms) {
                    Some(pcm) if voiced_ratio(&pcm) >= MIN_VOICED_RATIO => {
                        cache.insert((span.start_ms, span.end_ms), pcm);
                        true
                    }
                    _ => false,
                },
            );
        let mut by_voice: BTreeMap<String, Vec<ClipRow>> = BTreeMap::new();
        let dir = clips_dir(&meeting_dir);
        for span in spans {
            let Some(pcm) = cache.remove(&(span.start_ms, span.end_ms)) else {
                continue;
            };
            let id = voice_id(&track.name, &span.speaker);
            let rows = by_voice.entry(id.clone()).or_default();
            let idx = rows.len() as u32;
            let name = format!("{}-{idx}.wav", file_stem(&id));
            fs::create_dir_all(&dir).map_err(err)?;
            write_wav(&dir.join(&name), &pcm).map_err(err)?;
            rows.push(ClipRow {
                voice_id: id,
                idx,
                file: format!("{}/clips/{name}", details.meeting.dir),
                start_ms: span.start_ms,
                end_ms: span.end_ms,
                text: raw
                    .as_ref()
                    .map(|r| words_between(r, &track.name, span.start_ms, span.end_ms))
                    .unwrap_or_default(),
            });
        }
        for (id, rows) in by_voice {
            total += rows.len();
            store
                .replace_clips(meeting_id, &id, &rows, now_ms)
                .map_err(err)?;
        }
    }
    Ok(total)
}

/// Delete the clip rows and files of a meeting. Returns the number of files removed.
pub fn remove_clip_files(
    store: &Store,
    meeting_id: &str,
    meeting_root: &Path,
) -> Result<usize, store::StoreError> {
    let files = store.clear_clips(meeting_id)?;
    Ok(remove_files(meeting_root, &files))
}

/// Delete files given relative to the recordings root, and the emptied `clips` folders.
pub fn remove_files(meeting_root: &Path, files: &[String]) -> usize {
    let mut removed = 0;
    let mut folders = HashSet::new();
    for f in files {
        let path = meeting_root.join(f);
        if fs::remove_file(&path).is_ok() {
            removed += 1;
        }
        if let Some(parent) = path.parent() {
            folders.insert(parent.to_path_buf());
        }
    }
    for folder in folders {
        let _ = fs::remove_dir(folder);
    }
    removed
}

/// Whether the user has decided about every voice of the meeting.
pub fn all_voices_confirmed(voices: &[VoiceRow]) -> bool {
    !voices.is_empty()
        && voices
            .iter()
            .all(|v| v.confirmed || v.merged_into.is_some())
}

/// Delete the clips of meetings whose voices are all confirmed, or whose clips are older than
/// the limit. Returns the number of meetings cleaned.
pub fn sweep_clips(
    store: &Store,
    meeting_root: &Path,
    clip_max_days: u32,
    now_ms: i64,
) -> Result<usize, store::StoreError> {
    let mut cleaned = 0;
    for (meeting_id, oldest) in store.meetings_with_clips()? {
        let confirmed = all_voices_confirmed(&store.list_meeting_voices(&meeting_id)?);
        let expired = now_ms - oldest >= i64::from(clip_max_days) * 86_400_000;
        if confirmed || expired {
            remove_clip_files(store, &meeting_id, meeting_root)?;
            cleaned += 1;
        }
    }
    Ok(cleaned)
}

// ----- naming voices and learning -----

/// The participant a voice is named after, following merges.
pub fn effective_participant(voices: &[VoiceRow], voice_id: &str) -> Option<String> {
    let by_id: HashMap<&str, &VoiceRow> = voices.iter().map(|v| (v.voice_id.as_str(), v)).collect();
    let mut current = by_id.get(voice_id)?;
    for _ in 0..voices.len() {
        match current.merged_into.as_deref().and_then(|id| by_id.get(id)) {
            Some(next) => current = next,
            None => break,
        }
    }
    current.assigned_participant.clone()
}

fn source_key(meeting_id: &str, voice_id: &str) -> String {
    format!("{meeting_id}/{voice_id}")
}

/// Teach the profile of `participant_id` the voice from this meeting.
fn learn(
    store: &Store,
    participant_id: &str,
    meeting_id: &str,
    voice: &VoiceRow,
    now_ms: i64,
) -> Result<AddOutcome, store::StoreError> {
    let mut set = SampleSet::new(
        store
            .samples_of(participant_id)?
            .into_iter()
            .map(to_sample)
            .collect(),
    );
    let before: HashSet<String> = set.samples().iter().map(|s| s.key.clone()).collect();
    let outcome = set.add(Sample {
        key: source_key(meeting_id, &voice.voice_id),
        embedding: voice.embedding.clone(),
        speech_ms: voice.speech_ms,
    });
    if matches!(outcome, AddOutcome::Added | AddOutcome::Replaced) {
        let after: HashSet<String> = set.samples().iter().map(|s| s.key.clone()).collect();
        for gone in before.difference(&after) {
            store.remove_sample(participant_id, gone)?;
        }
        if let Some(s) = set
            .samples()
            .iter()
            .find(|s| s.key == source_key(meeting_id, &voice.voice_id))
        {
            store.put_sample(
                participant_id,
                &SampleRow {
                    source_key: s.key.clone(),
                    embedding: s.embedding.clone(),
                    speech_ms: s.speech_ms,
                },
                now_ms,
            )?;
        }
    }
    debug_assert!(set.len() <= MAX_SAMPLES);
    Ok(outcome)
}

/// Re-learn the voices of one meeting: every voice that is named (directly or through a
/// merge) and confirmed teaches its participant's profile; any earlier sample from the same
/// source under another name is removed first, so a correction replaces a wrong label.
pub fn refresh_learning(
    store: &Store,
    meeting_id: &str,
    now_ms: i64,
) -> Result<(), store::StoreError> {
    let voices = store.list_meeting_voices(meeting_id)?;
    for v in &voices {
        store.remove_samples_of_source(&source_key(meeting_id, &v.voice_id))?;
        let group_confirmed = v.confirmed || is_merged_with_confirmed(&voices, &v.voice_id);
        if !group_confirmed {
            continue;
        }
        if let Some(p) = effective_participant(&voices, &v.voice_id) {
            learn(store, &p, meeting_id, v, now_ms)?;
        }
    }
    Ok(())
}

fn is_merged_with_confirmed(voices: &[VoiceRow], voice_id: &str) -> bool {
    // A merged voice carries the confirmation of the voice it was merged into.
    let by_id: HashMap<&str, &VoiceRow> = voices.iter().map(|v| (v.voice_id.as_str(), v)).collect();
    let mut current = voice_id;
    for _ in 0..voices.len() {
        let Some(v) = by_id.get(current) else {
            return false;
        };
        match v.merged_into.as_deref() {
            Some(next) => current = next,
            None => return v.confirmed,
        }
    }
    false
}

/// The participants a meeting can name its voices after: those of its cycle and no others,
/// or the legacy local list when the meeting has no cycle.
pub fn participants_for_meeting(
    store: &Store,
    meeting_id: &str,
) -> Result<Vec<ParticipantInfo>, store::StoreError> {
    let cycle = store
        .get_report_settings(meeting_id)?
        .and_then(|s| s.cycle_id);
    store.list_participants_for(cycle.as_deref())
}

/// The participant to name a voice after when the user types `name`: the one of the
/// meeting's cycle (or of the legacy list for a meeting without a cycle) with that name,
/// ignoring case, created with `new_id` when there is none. Participants of other cycles
/// are never found. Returns the participant's id.
pub fn find_or_create_participant(
    store: &Store,
    meeting_id: &str,
    name: &str,
    new_id: &str,
    now_ms: i64,
) -> Result<String, store::StoreError> {
    let cycle = store
        .get_report_settings(meeting_id)?
        .and_then(|s| s.cycle_id);
    if let Some(p) = store.find_participant_by_name_in(cycle.as_deref(), name)? {
        return Ok(p.id);
    }
    store.create_participant_in(cycle.as_deref(), new_id, name, now_ms)?;
    Ok(new_id.to_string())
}

/// Name a voice as the user's decision (`None` leaves it deliberately unnamed), then update
/// the profiles.
pub fn assign_voice(
    store: &Store,
    meeting_id: &str,
    voice_id: &str,
    participant_id: Option<&str>,
    now_ms: i64,
) -> Result<bool, store::StoreError> {
    if !store.set_voice_assignment(meeting_id, voice_id, participant_id)? {
        return Ok(false);
    }
    refresh_learning(store, meeting_id, now_ms)?;
    Ok(true)
}

/// Merge a voice into another one of the same meeting (or undo with `None`). A voice cannot
/// be merged into itself or into a voice that is merged into it.
pub fn merge_voice(
    store: &Store,
    meeting_id: &str,
    voice_id: &str,
    into: Option<&str>,
    now_ms: i64,
) -> Result<bool, store::StoreError> {
    if let Some(target) = into {
        let voices = store.list_meeting_voices(meeting_id)?;
        if target == voice_id || !voices.iter().any(|v| v.voice_id == target) {
            return Ok(false);
        }
        // Walk up from the target: reaching `voice_id` would make a cycle.
        let mut current = target;
        for _ in 0..voices.len() {
            if current == voice_id {
                return Ok(false);
            }
            match voices
                .iter()
                .find(|v| v.voice_id == current)
                .and_then(|v| v.merged_into.as_deref())
            {
                Some(next) => current = next,
                None => break,
            }
        }
    }
    if !store.set_voice_merge(meeting_id, voice_id, into)? {
        return Ok(false);
    }
    refresh_learning(store, meeting_id, now_ms)?;
    Ok(true)
}

/// Accept every automatic match of a meeting as correct (one click for the common case).
pub fn confirm_automatic(
    store: &Store,
    meeting_id: &str,
    now_ms: i64,
) -> Result<usize, store::StoreError> {
    let mut n = 0;
    for v in store.list_meeting_voices(meeting_id)? {
        if v.decision == VoiceDecision::Auto && !v.confirmed {
            if let Some(p) = v.assigned_participant.as_deref() {
                store.set_voice_assignment(meeting_id, &v.voice_id, Some(p))?;
                n += 1;
            }
        }
    }
    refresh_learning(store, meeting_id, now_ms)?;
    Ok(n)
}

/// Display names for transcript speakers: voice id to the participant's name.
pub fn speaker_names(
    voices: &[VoiceRow],
    participants: &[ParticipantInfo],
) -> HashMap<String, String> {
    let names: HashMap<&str, &str> = participants
        .iter()
        .map(|p| (p.id.as_str(), p.name.as_str()))
        .collect();
    voices
        .iter()
        .filter_map(|v| {
            let p = effective_participant(voices, &v.voice_id)?;
            Some((v.voice_id.clone(), (*names.get(p.as_str())?).to_string()))
        })
        .collect()
}

/// Put participant names into the transcript's speaker list. A voice merged into another one
/// without a name shows the label of the voice it was merged into.
pub fn apply_names(
    view: &mut TranscriptView,
    voices: &[VoiceRow],
    names: &HashMap<String, String>,
) {
    let labels: HashMap<String, String> = view
        .speakers
        .iter()
        .map(|s| (s.id.clone(), s.label.clone()))
        .collect();
    for sp in &mut view.speakers {
        if let Some(name) = names.get(&sp.id) {
            sp.label = name.clone();
            continue;
        }
        if let Some(v) = voices.iter().find(|v| v.voice_id == sp.id) {
            // Follow the merge chain to the voice that carries the label.
            let mut target = v;
            for _ in 0..voices.len() {
                match target
                    .merged_into
                    .as_deref()
                    .and_then(|id| voices.iter().find(|x| x.voice_id == id))
                {
                    Some(next) => target = next,
                    None => break,
                }
            }
            if let Some(label) = labels.get(&target.voice_id) {
                sp.label = label.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use store::{Meeting, MeetingState, Segment, Track};

    fn tone(len: usize, amp: i16) -> Vec<i16> {
        (0..len)
            .map(|i| ((i as f32 * 0.3).sin() * f32::from(amp)) as i16)
            .collect()
    }

    #[test]
    fn silence_and_pauses_are_not_voiced() {
        assert_eq!(voiced_ratio(&vec![0i16; 16_000]), 0.0);
        let mut mostly_pause = tone(3_200, 8_000);
        mostly_pause.extend(vec![0i16; 16_000]);
        assert!(voiced_ratio(&mostly_pause) < MIN_VOICED_RATIO);
        assert!(voiced_ratio(&tone(32_000, 8_000)) > 0.95);
    }

    #[test]
    fn wav_round_trip() {
        let dir = tempdir("wav");
        let path = dir.join("a.wav");
        let pcm = tone(1_000, 5_000);
        write_wav(&path, &pcm).unwrap();
        assert_eq!(read_pcm(&path).unwrap(), pcm);
    }

    fn tempdir(name: &str) -> crate::test_util::TestDir {
        crate::test_util::TestDir::new(&format!("voices-{name}"))
    }

    fn voice(id: &str, embedding: Vec<f32>, ms: u64) -> VoiceRow {
        VoiceRow {
            voice_id: id.into(),
            track: "t".into(),
            label: id.into(),
            embedding,
            speech_ms: ms,
            decision: VoiceDecision::Unknown,
            score: None,
            matched_participant: None,
            assigned_participant: None,
            confirmed: false,
            merged_into: None,
        }
    }

    fn setup() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.create_meeting(&Meeting {
            id: "m".into(),
            title: "t".into(),
            started_unix_ms: 0,
            duration_ms: None,
            state: MeetingState::Stopped,
            dir: "m".into(),
        })
        .unwrap();
        s.create_meeting(&Meeting {
            id: "m2".into(),
            title: "t".into(),
            started_unix_ms: 1,
            duration_ms: None,
            state: MeetingState::Stopped,
            dir: "m2".into(),
        })
        .unwrap();
        s.create_participant("ola", "Ola", 1).unwrap();
        s.create_participant("jan", "Jan", 1).unwrap();
        s
    }

    #[test]
    fn naming_a_voice_teaches_the_profile_and_a_correction_moves_it() {
        let s = setup();
        s.put_meeting_voices("m", &[voice("t:A", vec![1.0, 0.0], 20_000)])
            .unwrap();
        assign_voice(&s, "m", "t:A", Some("ola"), 1).unwrap();
        assert_eq!(s.samples_of("ola").unwrap().len(), 1);
        assign_voice(&s, "m", "t:A", Some("jan"), 2).unwrap();
        assert!(s.samples_of("ola").unwrap().is_empty());
        assert_eq!(s.samples_of("jan").unwrap().len(), 1);
        assign_voice(&s, "m", "t:A", None, 3).unwrap();
        assert!(s.samples_of("jan").unwrap().is_empty());
    }

    #[test]
    fn short_voices_are_named_but_not_learned() {
        let s = setup();
        s.put_meeting_voices("m", &[voice("t:A", vec![1.0, 0.0], 2_000)])
            .unwrap();
        assign_voice(&s, "m", "t:A", Some("ola"), 1).unwrap();
        assert!(s.samples_of("ola").unwrap().is_empty());
    }

    #[test]
    fn an_automatic_match_teaches_nothing_until_it_is_confirmed() {
        let s = setup();
        let mut v = voice("t:A", vec![1.0, 0.0], 20_000);
        v.decision = VoiceDecision::Auto;
        v.assigned_participant = Some("ola".into());
        s.put_meeting_voices("m", &[v]).unwrap();
        refresh_learning(&s, "m", 1).unwrap();
        assert!(s.samples_of("ola").unwrap().is_empty());
        assert_eq!(confirm_automatic(&s, "m", 2).unwrap(), 1);
        assert_eq!(s.samples_of("ola").unwrap().len(), 1);
    }

    #[test]
    fn merged_voices_share_the_name_and_both_teach_the_profile() {
        let s = setup();
        s.put_meeting_voices(
            "m",
            &[
                voice("t:A", vec![1.0, 0.0], 20_000),
                voice("t:B", vec![0.9, 0.1], 10_000),
            ],
        )
        .unwrap();
        assert!(merge_voice(&s, "m", "t:B", Some("t:A"), 1).unwrap());
        assign_voice(&s, "m", "t:A", Some("ola"), 2).unwrap();
        let voices = s.list_meeting_voices("m").unwrap();
        assert_eq!(
            effective_participant(&voices, "t:B").as_deref(),
            Some("ola")
        );
        assert_eq!(s.samples_of("ola").unwrap().len(), 2);
        // Undoing the merge removes the second sample.
        merge_voice(&s, "m", "t:B", None, 3).unwrap();
        assert_eq!(s.samples_of("ola").unwrap().len(), 1);
    }

    #[test]
    fn merges_cannot_form_a_cycle() {
        let s = setup();
        s.put_meeting_voices(
            "m",
            &[
                voice("t:A", vec![1.0], 9_000),
                voice("t:B", vec![1.0], 9_000),
            ],
        )
        .unwrap();
        assert!(merge_voice(&s, "m", "t:A", Some("t:B"), 1).unwrap());
        assert!(!merge_voice(&s, "m", "t:B", Some("t:A"), 1).unwrap());
        assert!(!merge_voice(&s, "m", "t:A", Some("t:A"), 1).unwrap());
        assert!(!merge_voice(&s, "m", "t:A", Some("t:missing"), 1).unwrap());
    }

    #[test]
    fn speaker_stage_matches_a_known_voice_and_leaves_an_unknown_one_unknown() {
        let s = setup();
        // Enroll Ola from meeting m2.
        s.put_meeting_voices("m2", &[voice("t:X", vec![1.0, 0.0, 0.0], 30_000)])
            .unwrap();
        assign_voice(&s, "m2", "t:X", Some("ola"), 1).unwrap();

        // Meeting m: one voice like Ola, one different voice.
        let dir = tempdir("stage");
        let work = dir.join("work").join("diarize");
        fs::create_dir_all(&work).unwrap();
        fs::write(
            work.join("t.json"),
            serde_json::json!({
                "track": "t",
                "speakers": ["SPEAKER_00", "SPEAKER_01"],
                "turns": [
                    {"start_ms": 0, "end_ms": 10000, "speaker": "SPEAKER_00"},
                    {"start_ms": 10000, "end_ms": 20000, "speaker": "SPEAKER_01"}
                ],
                "embeddings": [[0.98, 0.1, 0.0], [0.0, 0.0, 1.0]]
            })
            .to_string(),
        )
        .unwrap();
        s.upsert_track(
            "m",
            &Track {
                name: "t".into(),
                source: "system".into(),
                pid: None,
                offset_ms: 0.0,
                ended: true,
                end_offset_ms: None,
                total_samples: None,
            },
        )
        .unwrap();
        run_speaker_id(&s, "m", &dir, &VoiceSettings::default()).unwrap();
        let voices = s.list_meeting_voices("m").unwrap();
        let known = voices.iter().find(|v| v.label == "SPEAKER_00").unwrap();
        let unknown = voices.iter().find(|v| v.label == "SPEAKER_01").unwrap();
        assert_eq!(known.decision, VoiceDecision::Auto);
        assert_eq!(known.assigned_participant.as_deref(), Some("ola"));
        assert!(!known.confirmed);
        assert_eq!(unknown.decision, VoiceDecision::Unknown);
        assert_eq!(unknown.assigned_participant, None);
        assert_eq!(known.speech_ms, 10_000);
    }

    #[test]
    fn a_voice_without_a_usable_embedding_stays_unknown_and_does_not_fail_the_stage() {
        let s = setup();
        let dir = tempdir("nan");
        let work = dir.join("work").join("diarize");
        fs::create_dir_all(&work).unwrap();
        // Written by an older worker: NaN is not valid JSON.
        fs::write(
            work.join("t.json"),
            r#"{"track":"t","speakers":["S0","S1"],
                "turns":[{"start_ms":0,"end_ms":1200,"speaker":"S0"},
                         {"start_ms":2000,"end_ms":9000,"speaker":"S1"}],
                "embeddings":[[NaN,NaN],[1.0,0.0]]}"#,
        )
        .unwrap();
        s.upsert_track(
            "m",
            &Track {
                name: "t".into(),
                source: "system".into(),
                pid: None,
                offset_ms: 0.0,
                ended: true,
                end_offset_ms: None,
                total_samples: None,
            },
        )
        .unwrap();
        assert_eq!(
            run_speaker_id(&s, "m", &dir, &VoiceSettings::default()).unwrap(),
            2
        );
        let voices = s.list_meeting_voices("m").unwrap();
        let broken = voices.iter().find(|v| v.label == "S0").unwrap();
        assert!(broken.embedding.is_empty());
        assert_eq!(broken.decision, VoiceDecision::Unknown);
        let fine = voices.iter().find(|v| v.label == "S1").unwrap();
        assert_eq!(fine.embedding, vec![1.0, 0.0]);
    }

    #[test]
    fn clips_are_cut_from_the_audio_and_skip_silence() {
        let s = setup();
        let root = tempdir("clips");
        let dir = root.join("m");
        fs::create_dir_all(dir.join("t")).unwrap();
        // 60 s of audio: speech from 0-6 s and 20-26 s, silence between.
        let mut pcm = tone(96_000, 8_000);
        pcm.extend(vec![0i16; 224_000]);
        pcm.extend(tone(96_000, 8_000));
        pcm.extend(vec![0i16; 544_000]);
        write_wav(&dir.join("t").join("0.wav"), &pcm).unwrap();
        s.upsert_track(
            "m",
            &Track {
                name: "t".into(),
                source: "system".into(),
                pid: None,
                offset_ms: 0.0,
                ended: true,
                end_offset_ms: None,
                total_samples: None,
            },
        )
        .unwrap();
        s.add_segment(
            "m",
            &Segment {
                track: "t".into(),
                segment: 0,
                file: "t/0.wav".into(),
                offset_ms: 0.0,
                samples: pcm.len() as u64,
            },
        )
        .unwrap();
        let work = dir.join("work").join("diarize");
        fs::create_dir_all(&work).unwrap();
        fs::write(
            work.join("t.json"),
            serde_json::json!({
                "track": "t",
                "speakers": ["S0"],
                "turns": [
                    {"start_ms": 0, "end_ms": 6000, "speaker": "S0"},
                    {"start_ms": 20000, "end_ms": 26000, "speaker": "S0"},
                    // A turn the diarization reports over silence must not become a clip.
                    {"start_ms": 40000, "end_ms": 46000, "speaker": "S0"}
                ],
                "embeddings": [[1.0, 0.0]]
            })
            .to_string(),
        )
        .unwrap();
        let n = run_clips(&s, "m", &root, &VoiceSettings::default(), 100).unwrap();
        assert_eq!(n, 2);
        let clips = s.list_clips("m").unwrap();
        assert_eq!(clips.len(), 2);
        for c in &clips {
            let pcm = read_pcm(&root.join(&c.file)).unwrap();
            assert_eq!(pcm.len() as u64, (c.end_ms - c.start_ms) * 16);
        }
        // Confirming the voices removes the clips.
        s.put_meeting_voices("m", &[voice("t:S0", vec![1.0, 0.0], 12_000)])
            .unwrap();
        assert_eq!(sweep_clips(&s, &root, 30, 200).unwrap(), 0);
        assign_voice(&s, "m", "t:S0", Some("ola"), 201).unwrap();
        assert_eq!(sweep_clips(&s, &root, 30, 202).unwrap(), 1);
        assert!(s.list_clips("m").unwrap().is_empty());
        assert!(!root.join(&clips[0].file).exists());
    }

    #[test]
    fn old_clips_expire_even_when_nobody_named_the_voices() {
        let s = setup();
        s.replace_clips(
            "m",
            "t:A",
            &[ClipRow {
                voice_id: "t:A".into(),
                idx: 0,
                file: "m/clips/a.wav".into(),
                start_ms: 0,
                end_ms: 1,
                text: String::new(),
            }],
            0,
        )
        .unwrap();
        let root = tempdir("expire");
        assert_eq!(sweep_clips(&s, &root, 30, 29 * 86_400_000).unwrap(), 0);
        assert_eq!(sweep_clips(&s, &root, 30, 30 * 86_400_000).unwrap(), 1);
    }

    #[test]
    fn names_reach_the_transcript_speakers() {
        use crate::transcript::{Speaker, TranscriptView};
        let voices = vec![
            {
                let mut v = voice("t:A", vec![1.0], 9_000);
                v.assigned_participant = Some("ola".into());
                v
            },
            {
                let mut v = voice("t:B", vec![1.0], 9_000);
                v.merged_into = Some("t:C".into());
                v
            },
            voice("t:C", vec![1.0], 9_000),
        ];
        let participants = vec![ParticipantInfo {
            id: "ola".into(),
            name: "Ola".into(),
            sample_count: 0,
            speech_ms: 0,
            cycle_id: None,
        }];
        let sp = |id: &str, label: &str| Speaker {
            id: id.into(),
            label: label.into(),
            kind: "voice".into(),
            track: Some("t".into()),
        };
        let mut view = TranscriptView {
            meeting_id: "m".into(),
            language: "en".into(),
            speakers: vec![
                sp("t:A", "Speaker 1"),
                sp("t:B", "Speaker 2"),
                sp("t:C", "Speaker 3"),
            ],
            segments: vec![],
        };
        let names = speaker_names(&voices, &participants);
        apply_names(&mut view, &voices, &names);
        assert_eq!(view.speakers[0].label, "Ola");
        assert_eq!(view.speakers[1].label, "Speaker 3");
        assert_eq!(view.speakers[2].label, "Speaker 3");
    }

    #[test]
    fn a_meeting_is_matched_against_and_names_voices_after_its_own_cycle_only() {
        let s = Store::open_in_memory().unwrap();
        for id in ["with-cycle", "without"] {
            s.create_meeting(&Meeting {
                id: id.into(),
                title: id.into(),
                started_unix_ms: 1,
                duration_ms: None,
                state: MeetingState::Stopped,
                dir: id.into(),
            })
            .unwrap();
        }
        s.set_report_settings(
            "with-cycle",
            &store::ReportSettings {
                cycle_id: Some("TEAM".into()),
                ..Default::default()
            },
        )
        .unwrap();
        s.sync_cycle_participants("TEAM", &[("ola", "Ola")])
            .unwrap();
        s.sync_cycle_participants("OTHER", &[("jan", "Jan")])
            .unwrap();
        s.create_participant("legacy", "Ewa", 1).unwrap();

        let names = |meeting: &str| -> Vec<String> {
            participants_for_meeting(&s, meeting)
                .unwrap()
                .into_iter()
                .map(|p| p.name)
                .collect()
        };
        assert_eq!(names("with-cycle"), ["Ola"]);
        assert_eq!(names("without"), ["Ewa"]);

        // Typing a name finds the participant of the meeting's cycle or creates one there.
        let ola = find_or_create_participant(&s, "with-cycle", "OLA", "new-1", 5).unwrap();
        assert_eq!(ola, "TEAM:ola");
        let jan = find_or_create_participant(&s, "with-cycle", "Jan", "new-2", 5).unwrap();
        assert_eq!(jan, "new-2", "Jan of the other cycle is not found");
        assert_eq!(names("with-cycle"), ["Jan", "Ola"]);
        let ewa = find_or_create_participant(&s, "without", "ewa", "new-3", 5).unwrap();
        assert_eq!(ewa, "legacy");

        // Matching only looks at the cycle's own voice profiles.
        let sample = SampleRow {
            source_key: "k".into(),
            embedding: vec![1.0, 0.0],
            speech_ms: 12_000,
        };
        for id in ["TEAM:ola", "legacy"] {
            s.put_sample(id, &sample, 1).unwrap();
        }
        s.put_sample(
            &s.participants_for_cycle("OTHER").unwrap()[0].id,
            &sample,
            1,
        )
        .unwrap();
        let ids = |cycle: Option<&str>| -> Vec<String> {
            load_profiles_for(&s, cycle)
                .unwrap()
                .into_iter()
                .map(|p| p.participant_id)
                .collect()
        };
        assert_eq!(ids(Some("TEAM")), ["TEAM:ola"]);
        assert_eq!(ids(None), ["legacy"]);
        assert_eq!(load_profiles(&s).unwrap().len(), 3);
    }
}
