//! Corrections to a published meeting: the notes the user writes after a meeting was published
//! (the draft stays locked), their local file, and their publication through the outbox.
//!
//! The local file is `work/corrections.json` of the meeting. Publishing a correction goes
//! through the same plan, rules and checks as the meeting (see `publishers::correction`): the
//! rule of the `github` target decides when, the public-repository check applies, and a note
//! about an item or an issue is also commented on that issue. A correction that is queued or
//! published is part of the record and cannot be changed or deleted here.

use cycle_store::corrections::CorrectionsFile;
use cycle_store::time::{format_utc_date, now_unix_ms};
use cycle_store::Correction;
use publishers::correction::{comment_key, file_key};
use publishers::flow::{self, PreparedCorrection};
use publishers::outbox::Env;
use publishers::rules::{PublishTarget, TargetRules, Trigger};
use serde::Serialize;
use serde_json::Value;
use store::{OpState, OutboxOp};

use super::{mode_name, start_error, Loaded, Publishing};
use crate::ipc::{IpcError, IpcResult};

/// The state of one operation of a correction, for a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CorrectionStepDto {
    pub op_id: i64,
    /// `queued`, `running`, `done` or `failed`.
    pub state: &'static str,
    pub attempts: u32,
    pub next_attempt_unix_ms: Option<i64>,
    pub last_error: Option<String>,
    pub result: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CorrectionDto {
    pub id: String,
    /// `YYYY-MM-DD`, the day it was written.
    pub date: String,
    pub text: String,
    pub item: Option<String>,
    pub issue: Option<String>,
    /// `local` (not queued), `queued`, `running`, `published` or `failed`: where the file in
    /// the repository stands.
    pub state: &'static str,
    /// The file operation, when one is queued or done.
    pub file: Option<CorrectionStepDto>,
    /// The comment on the issue, when one is planned.
    pub comment: Option<CorrectionStepDto>,
    /// The name of the file in the meeting folder once published (`corrections-2.md`).
    pub file_name: Option<String>,
    pub url: Option<String>,
    /// Only a correction that was never queued can be deleted.
    pub can_delete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CorrectionsDto {
    pub meeting_id: String,
    pub cycle_id: String,
    /// The draft is locked (the meeting is published or queued): corrections can be added.
    pub locked: bool,
    /// The meeting is in the repository, or its commit is queued: corrections can be published.
    pub in_repository: bool,
    /// The rule of the `github` target, which decides when a correction is published.
    pub rule: &'static str,
    pub corrections: Vec<CorrectionDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishCorrectionsDto {
    pub corrections: CorrectionsDto,
    /// Operations queued by this call.
    pub queued: usize,
    /// Things worth telling the user (an issue that gets no comment, for instance).
    pub notes: Vec<String>,
}

fn step(op: &OutboxOp) -> CorrectionStepDto {
    CorrectionStepDto {
        op_id: op.id,
        state: match op.state {
            OpState::Queued => "queued",
            OpState::Running => "running",
            OpState::Done => "done",
            OpState::Failed => "failed",
            // A cancelled operation is not shown: the correction is local again.
            OpState::Cancelled => "queued",
        },
        attempts: op.attempts,
        next_attempt_unix_ms: op.next_attempt_unix_ms,
        last_error: op.last_error.clone(),
        result: op
            .result
            .as_deref()
            .and_then(|r| serde_json::from_str(r).ok()),
    }
}

/// `12` and `#12` are both the issue `#12`.
fn normalise_issue(text: &str) -> Option<String> {
    let n = text.trim().trim_start_matches('#');
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) && n != "0" && !n.starts_with('0'))
        .then(|| format!("#{n}"))
}

fn problems(c: &Correction) -> IpcError {
    let text: Vec<String> = c
        .problems()
        .iter()
        .map(|p| format!("{}: {}", p.path, p.message))
        .collect();
    IpcError::new("invalid", text.join("; "))
}

impl Publishing {
    fn corrections_path(&self, loaded: &Loaded) -> std::path::PathBuf {
        loaded
            .dir
            .join("work")
            .join(cycle_store::corrections::CORRECTIONS_FILE)
    }

    fn read_corrections(&self, loaded: &Loaded) -> IpcResult<CorrectionsFile> {
        let path = self.corrections_path(loaded);
        match CorrectionsFile::read(&path) {
            Ok(Some(file)) if file.meeting_id == loaded.draft.meeting_id => Ok(file),
            Ok(_) => Ok(CorrectionsFile::new(&loaded.draft.meeting_id)),
            Err(e) => Err(IpcError::new("corrections_invalid", e.to_string())),
        }
    }

    fn write_corrections(&self, loaded: &Loaded, file: &CorrectionsFile) -> IpcResult<()> {
        file.write(&self.corrections_path(loaded))
            .map_err(|e| IpcError::new("io", e.to_string()))
    }

    fn corrections_view(
        &self,
        loaded: &Loaded,
        file: &CorrectionsFile,
    ) -> IpcResult<CorrectionsDto> {
        let meeting_id = loaded.draft.meeting_id.clone();
        let ops = self.with_store(|s| s.list_ops(Some(&meeting_id)))?;
        let publication = self.with_store(|s| s.get_publication(&meeting_id))?;
        let in_repository = publication.as_ref().is_some_and(|p| p.folder.is_some())
            || ops
                .iter()
                .any(|o| o.op_key == publishers::plan::COMMIT_KEY && o.state != OpState::Cancelled);
        let find = |key: String| {
            ops.iter()
                .find(|o| o.op_key == key && o.state != OpState::Cancelled)
        };
        let corrections = file
            .corrections
            .iter()
            .map(|c| {
                let file_op = find(file_key(&c.id));
                let comment_op = find(comment_key(&c.id));
                let file_step = file_op.map(step);
                let state = match file_op.map(|o| o.state) {
                    None | Some(OpState::Cancelled) => "local",
                    Some(OpState::Queued) => "queued",
                    Some(OpState::Running) => "running",
                    Some(OpState::Done) => "published",
                    Some(OpState::Failed) => "failed",
                };
                let result = file_step.as_ref().and_then(|s| s.result.as_ref());
                CorrectionDto {
                    id: c.id.clone(),
                    date: c.date.clone(),
                    text: c.text.clone(),
                    item: c.item.clone(),
                    issue: c.issue.clone(),
                    state,
                    file_name: (state == "published")
                        .then(|| result.and_then(|r| r.get("file")).and_then(Value::as_str))
                        .flatten()
                        .map(String::from),
                    url: (state == "published")
                        .then(|| result.and_then(|r| r.get("url")).and_then(Value::as_str))
                        .flatten()
                        .map(String::from),
                    file: file_step,
                    comment: comment_op.map(step),
                    can_delete: file_op.is_none() && comment_op.is_none(),
                }
            })
            .collect();
        Ok(CorrectionsDto {
            meeting_id,
            cycle_id: loaded.ctx.cycle.id.clone(),
            locked: ops.iter().any(|o| o.state != OpState::Cancelled),
            in_repository,
            rule: mode_name(TargetRules::of(&loaded.ctx.cycle).github),
            corrections,
        })
    }

    /// The corrections of a meeting with where each stands. Reads files and the outbox only.
    pub fn list_corrections(&self, meeting_id: &str) -> IpcResult<CorrectionsDto> {
        let loaded = self.load(meeting_id)?;
        let _guard = super::lock(&self.corrections_lock);
        let file = self.read_corrections(&loaded)?;
        self.corrections_view(&loaded, &file)
    }

    /// Writes a correction note to the meeting. Only a published (locked) meeting takes one:
    /// the draft of any other meeting can simply be edited. Under an `auto` rule of the
    /// `github` target it is queued at once (a public repository that was not confirmed, or a
    /// meeting not yet in the repository, leaves it local).
    pub fn add_correction(
        &self,
        meeting_id: &str,
        text: &str,
        item: Option<&str>,
        issue: Option<&str>,
        date: Option<&str>,
    ) -> IpcResult<CorrectionsDto> {
        let loaded = self.load(meeting_id)?;
        if !self.with_store(|s| flow::is_locked(s, meeting_id))? {
            return Err(IpcError::new(
                "not_published",
                "only a published meeting has corrections: edit the draft of this one instead",
            ));
        }
        let item = item.map(str::trim).filter(|i| !i.is_empty());
        if let Some(id) = item {
            if !loaded.draft.items.iter().any(|i| i.id == id) {
                return Err(IpcError::new(
                    "not_found",
                    format!("the meeting has no item {id}"),
                ));
            }
        }
        let issue = match issue.map(str::trim).filter(|i| !i.is_empty()) {
            None => None,
            Some(text) => Some(normalise_issue(text).ok_or_else(|| {
                IpcError::new(
                    "invalid",
                    format!("{text} is not an issue number (use #12)"),
                )
            })?),
        };
        let guard = super::lock(&self.corrections_lock);
        let mut file = self.read_corrections(&loaded)?;
        let correction = Correction {
            id: file.next_id(),
            date: date
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map_or_else(|| format_utc_date(now_unix_ms()), String::from),
            text: text.trim().to_string(),
            item: item.map(String::from),
            issue,
        };
        if !correction.problems().is_empty() {
            return Err(problems(&correction));
        }
        file.corrections.push(correction.clone());
        self.write_corrections(&loaded, &file)?;
        drop(guard);

        if TargetRules::of(&loaded.ctx.cycle).allows(PublishTarget::Github, Trigger::Auto) {
            if let Err(e) = self.queue_correction(&loaded, &correction, Trigger::Auto) {
                // A write that cannot go out yet is not an error of writing it.
                tracing::info!(meeting = meeting_id, code = %e.code, "an automatic correction is waiting for the user");
            }
        }
        self.list_corrections(meeting_id)
    }

    /// Plans and queues one correction; returns the notes of the plan and how many operations
    /// are new.
    fn queue_correction(
        &self,
        loaded: &Loaded,
        correction: &Correction,
        trigger: Trigger,
    ) -> IpcResult<(usize, Vec<String>)> {
        let prepared = PreparedCorrection {
            meeting_id: &loaded.draft.meeting_id,
            meeting_rel: &loaded.meeting_rel,
            meeting_date: &loaded.draft.date,
            ctx: &loaded.ctx,
            correction,
            trigger,
        };
        // The command's own check (the same rule as the plan and the dispatcher).
        let preview =
            self.with_store(|s| flow::preview_correction(self.env.as_ref(), s, &prepared))?;
        if let Some(block) = preview.blocks.first() {
            return Err(start_error(flow::StartError::Blocked(vec![block.clone()])));
        }
        let outcome = {
            let store = super::lock(&self.store);
            flow::publish_correction(self.env.as_ref(), &store, &prepared).map_err(start_error)?
        };
        let queued = outcome.ops.iter().filter(|(_, created)| *created).count();
        if queued > 0 {
            self.wake();
            self.env.outbox_changed();
        }
        let mut notes = outcome.plan.notes;
        notes.sort();
        notes.dedup();
        Ok((queued, notes))
    }

    /// Queues the corrections `ids` (all that are not queued yet when empty). A public or
    /// unknown repository is refused with `public_repo_unconfirmed`, as for the meeting; a
    /// meeting not in the repository with `not_in_repository`; `off` with
    /// `nothing_to_publish`.
    pub fn publish_corrections(
        &self,
        meeting_id: &str,
        ids: &[String],
    ) -> IpcResult<PublishCorrectionsDto> {
        let loaded = self.load(meeting_id)?;
        let file = {
            let _guard = super::lock(&self.corrections_lock);
            self.read_corrections(&loaded)?
        };
        let view = self.corrections_view(&loaded, &file)?;
        let wanted: Vec<&Correction> = file
            .corrections
            .iter()
            .filter(|c| {
                if ids.is_empty() {
                    view.corrections
                        .iter()
                        .any(|v| v.id == c.id && v.state == "local")
                } else {
                    ids.contains(&c.id)
                }
            })
            .collect();
        if wanted.is_empty() {
            return Err(IpcError::new(
                if ids.is_empty() {
                    "nothing_to_publish"
                } else {
                    "not_found"
                },
                if ids.is_empty() {
                    "every correction is queued or published already"
                } else {
                    "no such correction"
                },
            ));
        }
        let mut queued = 0;
        let mut notes = Vec::new();
        for correction in wanted {
            let (n, more) = self.queue_correction(&loaded, correction, Trigger::Explicit)?;
            queued += n;
            notes.extend(more);
        }
        notes.sort();
        notes.dedup();
        let file = {
            let _guard = super::lock(&self.corrections_lock);
            self.read_corrections(&loaded)?
        };
        Ok(PublishCorrectionsDto {
            corrections: self.corrections_view(&loaded, &file)?,
            queued,
            notes,
        })
    }

    /// Deletes a correction that was never queued.
    pub fn delete_correction(&self, meeting_id: &str, id: &str) -> IpcResult<CorrectionsDto> {
        let loaded = self.load(meeting_id)?;
        let _guard = super::lock(&self.corrections_lock);
        let mut file = self.read_corrections(&loaded)?;
        let view = self.corrections_view(&loaded, &file)?;
        let Some(shown) = view.corrections.iter().find(|c| c.id == id) else {
            return Err(IpcError::new("not_found", "no such correction"));
        };
        if !shown.can_delete {
            return Err(IpcError::new(
                "correction_queued",
                "this correction was queued for publishing: it is part of the record and stays",
            ));
        }
        file.corrections.retain(|c| c.id != id);
        self.write_corrections(&loaded, &file)?;
        self.corrections_view(&loaded, &file)
    }
}
