//! Cycle logic of the application, free of Tauri: the registered repositories, the payloads
//! of the cycle screens, the report mode gate, the participants of a cycle and the source
//! that gives the extraction stages their cycle.
//!
//! Functions take the list of registered repositories (`&[CycleRepoSetting]`), a `&Store`
//! and plain values, so every rule is tested against a temporary repository. The commands in
//! `cycle_commands.rs` are thin wrappers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cycle_store::schema::is_identifier;
use cycle_store::{
    Cycle, CycleEntry, DiscordBot, DiscordRef, DiscordTarget, GithubTarget, GlossaryEntry,
    IssueCache, LanguageSetting, Languages, NotionProperties, NotionTarget, Participant,
    PublishMode, Repo, ReportMode, Template, TemplateSection, Topic, TopicStatus, ValidationIssue,
};
use queue::context::{CycleData, CycleSource};
use queue::RunError;
use serde::{Deserialize, Serialize};
use store::{ParticipantInfo, ReportSettings, Store, SyncOutcome};

use crate::ipc::{IpcError, IpcResult};
use crate::settings::{CycleRepoSetting, LastReport, RepoKind, Settings};

/// Id of the repository the application creates itself.
pub const DEFAULT_REPO_ID: &str = "local";
/// Folder of that repository under the application data folder.
pub const DEFAULT_REPO_DIR: &str = "cycles-repo";
/// Folder under the data folder that holds clones made by the application.
pub const CLONES_DIR: &str = "cycle-repos";

// ----- payloads ----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueDto {
    pub path: String,
    pub message: String,
}

impl From<&ValidationIssue> for IssueDto {
    fn from(i: &ValidationIssue) -> Self {
        Self {
            path: i.path.clone(),
            message: i.message.clone(),
        }
    }
}

fn issue_dtos(issues: &[ValidationIssue]) -> Vec<IssueDto> {
    issues.iter().map(Into::into).collect()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LanguagesDto {
    pub asr: LanguageSetting,
    pub output: LanguageSetting,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DiscordRefDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CycleParticipantDto {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discord: Option<DiscordRefDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlossaryDto {
    pub term: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReportDto {
    pub default_mode: ReportMode,
    pub default_template: String,
    pub instruction: String,
    /// Meeting type to template name.
    pub by_type: BTreeMap<String, String>,
}

impl Default for ReportDto {
    fn default() -> Self {
        let r = cycle_store::Report::default();
        Self {
            default_mode: r.default_mode,
            default_template: r.default_template,
            instruction: r.instruction,
            by_type: r.by_type,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubTargetDto {
    pub path: String,
    pub publish: PublishMode,
    /// Create issues for task items and write back status changes.
    #[serde(default = "default_true")]
    pub issues: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscordTargetDto {
    /// A `cred:` reference, never the webhook URL. Exactly one of `webhook` and `bot`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot: Option<DiscordBotDto>,
    pub publish: PublishMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscordBotDto {
    /// A `cred:` reference, never the bot token.
    pub token: String,
    pub guild: String,
    pub channel: String,
    #[serde(default = "default_true")]
    pub thread: bool,
    #[serde(default = "default_true")]
    pub edit_on_correction: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotionTargetDto {
    /// A `cred:` reference, never the integration token.
    pub token: String,
    pub database: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_source: Option<String>,
    pub publish: PublishMode,
    #[serde(default)]
    pub properties: NotionPropertiesDto,
}

/// The Notion property names of the field mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NotionPropertiesDto {
    pub title: String,
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meeting: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cycle: Option<String>,
}

impl Default for NotionPropertiesDto {
    fn default() -> Self {
        NotionProperties::default().into()
    }
}

impl From<NotionProperties> for NotionPropertiesDto {
    fn from(p: NotionProperties) -> Self {
        Self {
            title: p.title,
            key: p.key,
            assignee: p.assignee,
            due: p.due,
            status: p.status,
            status_value: p.status_value,
            meeting: p.meeting,
            cycle: p.cycle,
        }
    }
}

impl From<NotionPropertiesDto> for NotionProperties {
    /// A name the user left empty is no name: the defaults for `title` and `key`, no property
    /// for the optional fields.
    fn from(d: NotionPropertiesDto) -> Self {
        let defaults = NotionProperties::default();
        let name = |value: String, default: String| {
            if value.trim().is_empty() {
                default
            } else {
                value
            }
        };
        let optional = |value: Option<String>| value.filter(|v| !v.trim().is_empty());
        Self {
            title: name(d.title, defaults.title),
            key: name(d.key, defaults.key),
            assignee: optional(d.assignee),
            due: optional(d.due),
            status: optional(d.status),
            status_value: optional(d.status_value),
            meeting: optional(d.meeting),
            cycle: optional(d.cycle),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TargetsDto {
    pub github: Option<GithubTargetDto>,
    pub discord: Option<DiscordTargetDto>,
    pub notion: Option<NotionTargetDto>,
}

/// `cycle.yaml` as the UI edits it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CycleDto {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub languages: LanguagesDto,
    #[serde(default)]
    pub participants: Vec<CycleParticipantDto>,
    #[serde(default)]
    pub glossary: Vec<GlossaryDto>,
    #[serde(default)]
    pub report: ReportDto,
    #[serde(default)]
    pub targets: TargetsDto,
}

impl From<&Cycle> for CycleDto {
    fn from(c: &Cycle) -> Self {
        Self {
            id: c.id.clone(),
            name: c.name.clone(),
            languages: LanguagesDto {
                asr: c.languages.asr,
                output: c.languages.output,
            },
            participants: c
                .participants
                .iter()
                .map(|p| CycleParticipantDto {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    github: p.github.clone(),
                    discord: p.discord.as_ref().map(|d| DiscordRefDto {
                        user_id: d.user_id.clone(),
                        role_id: d.role_id.clone(),
                    }),
                })
                .collect(),
            glossary: c
                .glossary
                .iter()
                .map(|g| GlossaryDto {
                    term: g.term.clone(),
                    hint: g.hint.clone(),
                })
                .collect(),
            report: ReportDto {
                default_mode: c.report.default_mode,
                default_template: c.report.default_template.clone(),
                instruction: c.report.instruction.clone(),
                by_type: c.report.by_type.clone(),
            },
            targets: TargetsDto {
                github: c.targets.github.as_ref().map(|t| GithubTargetDto {
                    path: t.path.clone(),
                    publish: t.publish,
                    issues: t.issues,
                }),
                discord: c.targets.discord.as_ref().map(|t| DiscordTargetDto {
                    webhook: t.webhook.clone(),
                    bot: t.bot.as_ref().map(|b| DiscordBotDto {
                        token: b.token.clone(),
                        guild: b.guild.clone(),
                        channel: b.channel.clone(),
                        thread: b.thread,
                        edit_on_correction: b.edit_on_correction,
                    }),
                    publish: t.publish,
                    template: t.template.clone(),
                }),
                notion: c.targets.notion.as_ref().map(|t| NotionTargetDto {
                    token: t.token.clone(),
                    database: t.database.clone(),
                    data_source: t.data_source.clone(),
                    publish: t.publish,
                    properties: t.properties.clone().into(),
                }),
            },
        }
    }
}

impl From<CycleDto> for Cycle {
    fn from(d: CycleDto) -> Self {
        Cycle {
            id: d.id,
            name: d.name,
            languages: Languages {
                asr: d.languages.asr,
                output: d.languages.output,
            },
            participants: d
                .participants
                .into_iter()
                .map(|p| Participant {
                    id: p.id,
                    name: p.name,
                    github: p.github.filter(|g| !g.is_empty()),
                    discord: p.discord.map(|d| DiscordRef {
                        user_id: d.user_id,
                        role_id: d.role_id,
                    }),
                })
                .collect(),
            glossary: d
                .glossary
                .into_iter()
                .map(|g| GlossaryEntry {
                    term: g.term,
                    hint: g.hint.filter(|h| !h.is_empty()),
                })
                .collect(),
            report: cycle_store::Report {
                default_mode: d.report.default_mode,
                default_template: d.report.default_template,
                instruction: d.report.instruction,
                by_type: d.report.by_type,
            },
            targets: cycle_store::Targets {
                github: d.targets.github.map(|t| GithubTarget {
                    path: t.path,
                    publish: t.publish,
                    issues: t.issues,
                }),
                discord: d.targets.discord.map(|t| DiscordTarget {
                    webhook: t.webhook.filter(|w| !w.is_empty()),
                    bot: t.bot.map(|b| DiscordBot {
                        token: b.token,
                        guild: b.guild,
                        channel: b.channel,
                        thread: b.thread,
                        edit_on_correction: b.edit_on_correction,
                    }),
                    publish: t.publish,
                    template: t.template.filter(|t| !t.is_empty()),
                }),
                notion: d.targets.notion.map(|t| NotionTarget {
                    token: t.token,
                    database: t.database,
                    data_source: t.data_source.filter(|s| !s.trim().is_empty()),
                    publish: t.publish,
                    properties: t.properties.into(),
                }),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateSectionDto {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
}

impl From<&TemplateSection> for TemplateSectionDto {
    fn from(s: &TemplateSection) -> Self {
        Self {
            id: s.id.clone(),
            title: s.title.clone(),
            description: s.description.clone(),
        }
    }
}

/// A template in the list: its name and the sections it declares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateSummaryDto {
    pub name: String,
    pub sections: Vec<TemplateSectionDto>,
}

/// A whole template, to edit and save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateDto {
    pub name: String,
    #[serde(default)]
    pub sections: Vec<TemplateSectionDto>,
    #[serde(default)]
    pub body: String,
}

impl From<&TemplateDto> for Template {
    fn from(t: &TemplateDto) -> Self {
        Template {
            name: t.name.clone(),
            sections: t
                .sections
                .iter()
                .map(|s| TemplateSection {
                    id: s.id.clone(),
                    title: s.title.clone(),
                    description: s.description.clone(),
                })
                .collect(),
            body: t.body.clone(),
        }
    }
}

impl From<&Template> for TemplateDto {
    fn from(t: &Template) -> Self {
        Self {
            name: t.name.clone(),
            sections: t.sections.iter().map(Into::into).collect(),
            body: t.body.clone(),
        }
    }
}

/// Result of checking or saving a template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateCheck {
    pub valid: bool,
    /// Whether the file was written (only `save_template`).
    pub saved: bool,
    pub issues: Vec<IssueDto>,
    /// The placeholders a body may use, for the editor's help.
    pub placeholders: Vec<String>,
}

fn template_check(issues: Vec<ValidationIssue>, saved: bool) -> TemplateCheck {
    TemplateCheck {
        valid: issues.is_empty(),
        saved,
        issues: issue_dtos(&issues),
        placeholders: cycle_store::PLACEHOLDERS
            .iter()
            .map(ToString::to_string)
            .collect(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicDto {
    pub id: String,
    pub text: String,
    pub status: TopicStatus,
    pub opened_in: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_in: Option<String>,
}

impl From<&Topic> for TopicDto {
    fn from(t: &Topic) -> Self {
        Self {
            id: t.id.clone(),
            text: t.text.clone(),
            status: t.status,
            opened_in: t.opened_in.clone(),
            closed_in: t.closed_in.clone(),
        }
    }
}

/// One line of the cycle list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CycleListItem {
    pub id: String,
    pub name: String,
    pub participants_count: u32,
    /// `None` when `cycle.yaml` could not be read at all.
    pub default_mode: Option<ReportMode>,
    /// Usable for a recording: no issues.
    pub valid: bool,
    pub issues: Vec<IssueDto>,
    pub repo_id: String,
    /// `YYYY-MM-DD` of the last successful pull or clone; `None` for a local repository.
    pub state_as_of: Option<String>,
    /// The last attempt to update the clone failed: what is shown may be stale.
    pub offline: bool,
}

/// A registered repository and what is known about its state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoDto {
    pub id: String,
    pub path: String,
    pub remote: Option<String>,
    pub kind: RepoKind,
    /// `owner/name` set for GitHub, when it is not the remote's.
    pub github_repo: Option<String>,
    pub github_api: Option<String>,
    /// The folder exists.
    pub available: bool,
    pub state_as_of: Option<String>,
    pub offline: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullResult {
    pub repo: RepoDto,
    /// `updated`, `upToDate`, `noRemote` or `failed`.
    pub outcome: &'static str,
    pub error: Option<String>,
    /// Problems after an update: a cycle that no longer parses, a participant that could
    /// not be linked to its voice profile. The update itself stays.
    pub warnings: Vec<String>,
}

/// Everything the cycle editor shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CycleDetails {
    pub cycle: CycleDto,
    pub repo_id: String,
    pub valid: bool,
    pub issues: Vec<IssueDto>,
    pub templates: Vec<TemplateSummaryDto>,
    pub topics: Vec<TopicDto>,
    /// The meeting types of `report.byType`.
    pub meeting_types: Vec<String>,
    pub state_as_of: Option<String>,
    pub offline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveCycleResult {
    /// False when the cycle is invalid: nothing was written and `issues` says why.
    pub saved: bool,
    pub issues: Vec<IssueDto>,
    /// Problems after a successful write (voice profiles that could not be linked).
    pub warnings: Vec<String>,
    pub details: Option<CycleDetails>,
}

// ----- repositories ------------------------------------------------------------------------

fn state_of(repo: &Repo, setting: &CycleRepoSetting) -> (Option<String>, bool, Option<String>) {
    if setting.kind == RepoKind::Local && setting.remote.is_none() {
        return (None, false, None);
    }
    let state = repo.state();
    let error = state.last_pull_error.clone();
    (state.as_of_date(), error.is_some(), error)
}

/// The registered repositories with their state.
pub fn list_repos(repos: &[CycleRepoSetting]) -> Vec<RepoDto> {
    repos.iter().map(repo_dto).collect()
}

pub fn repo_dto(setting: &CycleRepoSetting) -> RepoDto {
    let (state_as_of, offline, last_error, available) = match Repo::open(&setting.path) {
        Ok(repo) => {
            let (a, o, e) = state_of(&repo, setting);
            (a, o, e, true)
        }
        Err(_) => (None, false, None, false),
    };
    RepoDto {
        id: setting.id.clone(),
        path: setting.path.clone(),
        remote: setting.remote.clone(),
        kind: setting.kind,
        github_repo: setting.github_repo.clone(),
        github_api: setting.github_api.clone(),
        available,
        state_as_of,
        offline,
        last_error,
    }
}

fn find_setting<'a>(repos: &'a [CycleRepoSetting], id: &str) -> IpcResult<&'a CycleRepoSetting> {
    repos
        .iter()
        .find(|r| r.id == id)
        .ok_or_else(|| IpcError::new("not_found", "this cycle repository is not registered"))
}

/// A cycle found in a registered repository.
pub struct Located {
    pub setting: CycleRepoSetting,
    pub repo: Repo,
    pub entry: CycleEntry,
}

/// The first registered repository that has a cycle with this id. A repository whose folder
/// is gone is skipped.
pub fn locate(repos: &[CycleRepoSetting], cycle_id: &str) -> IpcResult<Option<Located>> {
    if !is_identifier(cycle_id) {
        return Ok(None);
    }
    for setting in repos {
        let Ok(repo) = Repo::open(&setting.path) else {
            continue;
        };
        if !repo.cycle_dir(cycle_id)?.join("cycle.yaml").is_file() {
            continue;
        }
        let entry = repo
            .list_cycles()?
            .into_iter()
            .find(|e| e.dir_name == cycle_id);
        if let Some(entry) = entry {
            return Ok(Some(Located {
                setting: setting.clone(),
                repo,
                entry,
            }));
        }
    }
    Ok(None)
}

fn locate_or_err(repos: &[CycleRepoSetting], cycle_id: &str) -> IpcResult<Located> {
    locate(repos, cycle_id)?.ok_or_else(|| {
        IpcError::new(
            "not_found",
            format!("the cycle {cycle_id} is not in any registered repository"),
        )
    })
}

/// A loaded, valid cycle, or `cycle_invalid` with the first problem.
fn load_valid(located: &Located) -> IpcResult<Cycle> {
    match (&located.entry.cycle, located.entry.issues.first()) {
        (Some(c), None) => Ok(c.clone()),
        (_, Some(first)) => Err(IpcError::new(
            "cycle_invalid",
            format!("the cycle cannot be used until it is fixed: {first}"),
        )),
        (None, None) => Err(IpcError::new("cycle_invalid", "the cycle cannot be read")),
    }
}

/// Every cycle of every registered repository.
pub fn list_cycles(repos: &[CycleRepoSetting]) -> Vec<CycleListItem> {
    let mut out = Vec::new();
    for setting in repos {
        let Ok(repo) = Repo::open(&setting.path) else {
            continue;
        };
        let Ok(entries) = repo.list_cycles() else {
            continue;
        };
        let (state_as_of, offline, _) = state_of(&repo, setting);
        for e in entries {
            out.push(CycleListItem {
                name: e
                    .cycle
                    .as_ref()
                    .map_or_else(|| e.dir_name.clone(), |c| c.name.clone()),
                participants_count: e.cycle.as_ref().map_or(0, |c| c.participants.len() as u32),
                default_mode: e.cycle.as_ref().map(|c| c.report.default_mode),
                valid: e.is_valid(),
                issues: issue_dtos(&e.issues),
                id: e.dir_name,
                repo_id: setting.id.clone(),
                state_as_of: state_as_of.clone(),
                offline,
            });
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn template_summaries(repo: &Repo, cycle_id: &str) -> IpcResult<Vec<TemplateSummaryDto>> {
    let mut out = Vec::new();
    for name in repo.list_templates(cycle_id)? {
        // A template that cannot be parsed is still listed (so it can be opened and fixed
        // is not possible: its sections are unknown), with no sections.
        let sections = repo
            .read_template(cycle_id, &name)
            .map(|t| t.sections.iter().map(Into::into).collect())
            .unwrap_or_default();
        out.push(TemplateSummaryDto { name, sections });
    }
    Ok(out)
}

fn details_of(located: &Located) -> IpcResult<CycleDetails> {
    let cycle = located.entry.cycle.as_ref().ok_or_else(|| {
        let first = located
            .entry
            .issues
            .first()
            .map(ToString::to_string)
            .unwrap_or_default();
        IpcError::new(
            "cycle_invalid",
            format!("cycle.yaml cannot be read: {first}"),
        )
    })?;
    let id = &located.entry.dir_name;
    let (state_as_of, offline, _) = state_of(&located.repo, &located.setting);
    Ok(CycleDetails {
        cycle: cycle.into(),
        repo_id: located.setting.id.clone(),
        valid: located.entry.is_valid(),
        issues: issue_dtos(&located.entry.issues),
        templates: template_summaries(&located.repo, id)?,
        topics: located
            .repo
            .load_topics(id)
            .unwrap_or_default()
            .iter()
            .map(Into::into)
            .collect(),
        meeting_types: cycle.report.by_type.keys().cloned().collect(),
        state_as_of,
        offline,
    })
}

pub fn get_cycle(repos: &[CycleRepoSetting], cycle_id: &str) -> IpcResult<CycleDetails> {
    details_of(&locate_or_err(repos, cycle_id)?)
}

/// The repository a new cycle goes to. Without an id that is the default local repository,
/// created now when it does not exist yet (and registered in `settings`).
pub fn resolve_target_repo(
    settings: &mut Settings,
    repo_id: Option<&str>,
    data_dir: &Path,
) -> IpcResult<CycleRepoSetting> {
    if let Some(id) = repo_id.filter(|id| !id.is_empty()) {
        return find_setting(&settings.cycle_repos, id).cloned();
    }
    if let Some(existing) = settings
        .cycle_repos
        .iter()
        .find(|r| r.id == DEFAULT_REPO_ID)
    {
        return Ok(existing.clone());
    }
    let path = data_dir.join(DEFAULT_REPO_DIR);
    Repo::init_local(&path)?;
    let setting = CycleRepoSetting {
        id: DEFAULT_REPO_ID.into(),
        path: path.to_string_lossy().into_owned(),
        remote: None,
        kind: RepoKind::Local,
        github_repo: None,
        github_api: None,
    };
    settings.cycle_repos.push(setting.clone());
    Ok(setting)
}

/// Creates a cycle (`cycle.yaml`, an empty `topics.yaml`, `templates/default.md`) in
/// `target`. The id is unique over all registered repositories.
pub fn create_cycle(
    repos: &[CycleRepoSetting],
    target: &CycleRepoSetting,
    id: &str,
    name: &str,
) -> IpcResult<CycleDetails> {
    let id = id.trim();
    let name = name.trim();
    if !is_identifier(id) {
        return Err(IpcError::new(
            "invalid",
            "the cycle id may contain letters, digits, - and _ and must start with a letter or digit",
        ));
    }
    if name.is_empty() || name.chars().count() > 120 {
        return Err(IpcError::new(
            "invalid",
            "enter a name of 1 to 120 characters",
        ));
    }
    if locate(repos, id)?.is_some() {
        return Err(IpcError::new(
            "duplicate",
            format!("a cycle with the id {id} already exists"),
        ));
    }
    let repo = Repo::open(&target.path)?;
    repo.create_cycle(&Cycle::new(id, name))?;
    get_cycle(std::slice::from_ref(target), id)
}

/// Whether two names are the same person's, ignoring case (Unicode-aware) and the space
/// around them. The store refuses two participants of a cycle with such names.
pub fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

fn name_issues(cycle: &Cycle) -> Vec<ValidationIssue> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut issues = Vec::new();
    for (i, p) in cycle.participants.iter().enumerate() {
        if p.name.trim().is_empty() {
            issues.push(ValidationIssue::new(
                format!("participants[{i}].name"),
                "a participant needs a name",
            ));
        } else if let Some(first) = seen.insert(p.name.trim().to_lowercase(), i) {
            issues.push(ValidationIssue::new(
                format!("participants[{i}].name"),
                format!(
                    "the name \"{}\" is used twice (first at participants[{first}])",
                    p.name
                ),
            ));
        }
    }
    issues
}

fn participant_pairs(cycle: &Cycle) -> Vec<(String, String)> {
    cycle
        .participants
        .iter()
        .map(|p| (p.id.clone(), p.name.trim().to_string()))
        .collect()
}

/// Where the participants of a cycle are kept in step with `cycle.yaml`. The database
/// implements it directly; the application state locks the database for each call only, so
/// the repository work around it never holds the lock.
pub trait ParticipantSync {
    fn sync_participants(
        &self,
        cycle_id: &str,
        participants: &[(String, String)],
    ) -> IpcResult<SyncOutcome>;
}

impl ParticipantSync for Store {
    fn sync_participants(
        &self,
        cycle_id: &str,
        participants: &[(String, String)],
    ) -> IpcResult<SyncOutcome> {
        Ok(self.sync_cycle_participants(cycle_id, participants)?)
    }
}

/// What a sync tells the user: participants that could not be stored, and ones that were
/// matched to a changed id by name.
fn sync_notes(outcome: &SyncOutcome) -> Vec<String> {
    let mut notes = outcome.warnings.clone();
    for id in &outcome.rebound {
        notes.push(format!(
            "the participant {id} has a new id in cycle.yaml: the voice profile of the person with the same name was kept"
        ));
    }
    notes
}

/// Validates and writes `cycle.yaml`, then brings the stored participants in line with it.
/// An invalid cycle is not written: the issues are returned instead.
pub fn save_cycle<S: ParticipantSync + ?Sized>(
    repos: &[CycleRepoSetting],
    store: &S,
    dto: CycleDto,
) -> IpcResult<SaveCycleResult> {
    let located = locate_or_err(repos, &dto.id)?;
    let mut cycle: Cycle = dto.into();
    cycle.name = cycle.name.trim().to_string();
    for p in &mut cycle.participants {
        p.name = p.name.trim().to_string();
    }
    let templates = located.repo.list_templates(&cycle.id)?;
    let mut issues = cycle.validate(&templates);
    issues.extend(name_issues(&cycle));
    if issues.is_empty() {
        if let Err(e) = located.repo.save_cycle(&cycle) {
            issues.extend_from_slice(e.issues());
            if issues.is_empty() {
                return Err(e.into());
            }
        }
    }
    if !issues.is_empty() {
        return Ok(SaveCycleResult {
            saved: false,
            issues: issue_dtos(&issues),
            warnings: Vec::new(),
            details: None,
        });
    }
    let mut warnings = Vec::new();
    match store.sync_participants(&cycle.id, &participant_pairs(&cycle)) {
        Ok(outcome) => warnings.extend(sync_notes(&outcome)),
        Err(e) => warnings.push(format!(
            "the participants could not be linked to voice profiles: {}",
            e.message
        )),
    }
    let refreshed = locate_or_err(repos, &cycle.id)?;
    Ok(SaveCycleResult {
        saved: true,
        issues: Vec::new(),
        warnings,
        details: Some(details_of(&refreshed)?),
    })
}

/// Sets the publish rule of one target of a cycle (`targets.<target>.publish` of
/// `cycle.yaml`) and nothing else, and returns the targets as they are written. The rules
/// are the ones of ADR 0019: `github` covers the report, the issues and the status
/// write-back, but no mode lets a status change go out without the user's approval and an
/// explicit publication; the public-repository check applies to every mode.
///
/// A cycle without `targets.github` gets one with the default folder and issues on. Discord
/// needs a webhook or bot reference and Notion a token and a database, which only the cycle
/// editor can add, so a cycle without such a target is refused (`no_target`). Nothing is
/// written when the rule is already set.
pub fn set_target_rule(
    repos: &[CycleRepoSetting],
    cycle_id: &str,
    target: &str,
    mode: PublishMode,
) -> IpcResult<TargetsDto> {
    let located = locate_or_err(repos, cycle_id)?;
    let mut cycle = load_valid(&located)?;
    let changed = match target {
        "github" => {
            let existed = cycle.targets.github.is_some();
            let github = cycle
                .targets
                .github
                .get_or_insert_with(GithubTarget::default);
            let changed = !existed || github.publish != mode;
            github.publish = mode;
            changed
        }
        "discord" => {
            let discord = cycle.targets.discord.as_mut().ok_or_else(|| {
                IpcError::new(
                    "no_target",
                    "this cycle has no Discord target: add one in the cycle settings first",
                )
            })?;
            let changed = discord.publish != mode;
            discord.publish = mode;
            changed
        }
        "notion" => {
            let notion = cycle.targets.notion.as_mut().ok_or_else(|| {
                IpcError::new(
                    "no_target",
                    "this cycle has no Notion target: add one in the cycle settings first",
                )
            })?;
            let changed = notion.publish != mode;
            notion.publish = mode;
            changed
        }
        other => {
            return Err(IpcError::new(
                "invalid",
                format!(
                    "{other} is not a target with a publishing rule (github, discord or notion)"
                ),
            ))
        }
    };
    if changed {
        located.repo.save_cycle(&cycle)?;
    }
    Ok(CycleDto::from(&cycle).targets)
}

/// Brings the stored participants of a cycle in line with its `cycle.yaml`. A cycle that is
/// not available or not valid is left alone.
pub fn sync_cycle<S: ParticipantSync + ?Sized>(
    repos: &[CycleRepoSetting],
    store: &S,
    cycle_id: &str,
) -> IpcResult<()> {
    let Some(located) = locate(repos, cycle_id)? else {
        return Ok(());
    };
    let cycle = load_valid(&located)?;
    for note in sync_notes(&store.sync_participants(cycle_id, &participant_pairs(&cycle))?) {
        tracing::warn!(cycle = cycle_id, "{note}");
    }
    Ok(())
}

/// [`sync_cycle`] for every valid cycle of one repository (after a pull). One cycle that
/// cannot be synced never stops the others; what went wrong is returned as notes.
pub fn sync_repo<S: ParticipantSync + ?Sized>(
    setting: &CycleRepoSetting,
    store: &S,
) -> Vec<String> {
    let mut notes = Vec::new();
    let Ok(repo) = Repo::open(&setting.path) else {
        return notes;
    };
    for entry in repo.list_cycles().unwrap_or_default() {
        if let (true, Some(cycle)) = (entry.is_valid(), &entry.cycle) {
            match store.sync_participants(&cycle.id, &participant_pairs(cycle)) {
                Ok(outcome) => {
                    notes.extend(sync_notes(&outcome).into_iter().map(|n| {
                        tracing::warn!(cycle = %cycle.id, "{n}");
                        format!("{}: {n}", cycle.id)
                    }));
                }
                Err(e) => {
                    tracing::warn!(cycle = %cycle.id, error = %e.message, "participant sync failed");
                    notes.push(format!(
                        "{}: the participants could not be linked to voice profiles: {}",
                        cycle.id, e.message
                    ));
                }
            }
        }
    }
    notes
}

/// The folders of earlier meetings of a cycle inside one repository that exist: the folder
/// the cycle publishes to (`targets.github.path`) and, when that is another one, the default
/// `meetings/<cycle>` where meetings published before the change are.
fn meeting_dirs_in(repo: &Repo, cycle: Option<&Cycle>, cycle_id: &str) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let configured = cycle.map(Cycle::meetings_path);
    for base in configured
        .into_iter()
        .chain([cycle_store::DEFAULT_MEETINGS_PATH])
    {
        if let Ok(dir) = repo.meetings_dir_at(base, cycle_id) {
            if dir.is_dir() && !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    }
    dirs
}

/// The meeting folders of a cycle in the registered repositories that exist: where earlier
/// meetings of a cycle are kept besides the application's own drafts.
pub fn previous_meeting_dirs(repos: &[CycleRepoSetting], cycle_id: &str) -> Vec<PathBuf> {
    repos
        .iter()
        .filter_map(|setting| Repo::open(&setting.path).ok())
        .flat_map(|repo| {
            let cycle = repo.load_cycle(cycle_id).ok();
            meeting_dirs_in(&repo, cycle.as_ref(), cycle_id)
        })
        .collect()
}

// ----- templates ---------------------------------------------------------------------------

pub fn list_templates(
    repos: &[CycleRepoSetting],
    cycle_id: &str,
) -> IpcResult<Vec<TemplateSummaryDto>> {
    let located = locate_or_err(repos, cycle_id)?;
    template_summaries(&located.repo, cycle_id)
}

pub fn get_template(
    repos: &[CycleRepoSetting],
    cycle_id: &str,
    name: &str,
) -> IpcResult<TemplateDto> {
    let located = locate_or_err(repos, cycle_id)?;
    Ok((&located.repo.read_template(cycle_id, name)?).into())
}

/// Checks the sections and the placeholders of a template without writing anything.
pub fn validate_template(t: &TemplateDto) -> TemplateCheck {
    template_check(cycle_store::validate_template(&t.into()), false)
}

/// Writes a template when it is valid; otherwise returns the issues and writes nothing.
pub fn save_template(
    repos: &[CycleRepoSetting],
    cycle_id: &str,
    t: &TemplateDto,
) -> IpcResult<TemplateCheck> {
    let located = locate_or_err(repos, cycle_id)?;
    let template: Template = t.into();
    let issues = cycle_store::validate_template(&template);
    if !issues.is_empty() {
        return Ok(template_check(issues, false));
    }
    located.repo.write_template(cycle_id, &template)?;
    Ok(template_check(Vec::new(), true))
}

// ----- report mode gate --------------------------------------------------------------------

/// The report settings the UI sends when a recording starts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReportChoice {
    pub cycle_id: String,
    /// `fixed`, `template`, `auto` or `by-type`.
    pub mode: String,
    pub template: Option<String>,
    pub meeting_type: Option<String>,
    /// `auto`, `pl` or `en`; absent means the cycle's setting.
    pub output_language: Option<String>,
}

fn gate(message: impl Into<String>) -> IpcError {
    IpcError::new("report_mode_required", message)
}

/// The choice to validate: the one sent, or (when the caller allows it) the last one used.
/// Without either the recording must not start.
pub fn resolve_choice(
    sent: Option<ReportChoice>,
    use_last: bool,
    last: Option<&LastReport>,
) -> IpcResult<ReportChoice> {
    if let Some(choice) = sent {
        return Ok(choice);
    }
    match (use_last, last) {
        (true, Some(l)) => Ok(ReportChoice {
            cycle_id: l.cycle_id.clone(),
            mode: l.mode.clone(),
            template: l.template.clone(),
            meeting_type: l.meeting_type.clone(),
            output_language: None,
        }),
        _ => Err(gate("choose a cycle and a report mode before recording")),
    }
}

fn parse_mode(mode: &str) -> IpcResult<ReportMode> {
    match mode {
        "fixed" => Ok(ReportMode::Fixed),
        "template" => Ok(ReportMode::Template),
        "auto" => Ok(ReportMode::Auto),
        "by-type" => Ok(ReportMode::ByType),
        "" => Err(gate("choose a report mode")),
        other => Err(gate(format!("unknown report mode {other:?}"))),
    }
}

fn mode_name(mode: ReportMode) -> &'static str {
    match mode {
        ReportMode::Fixed => "fixed",
        ReportMode::Template => "template",
        ReportMode::Auto => "auto",
        ReportMode::ByType => "by-type",
    }
}

/// Checks a report choice against the cycle and returns what to store on the meeting.
///
/// Refuses (`report_mode_required`) when the cycle is missing, unknown or not valid, the
/// mode or language is not one of the known values, `template` mode names a template the
/// cycle does not have, or `by-type` mode has no meeting type or one that `report.byType`
/// does not define.
pub fn validate_report(
    repos: &[CycleRepoSetting],
    choice: &ReportChoice,
) -> IpcResult<ReportSettings> {
    if choice.cycle_id.trim().is_empty() {
        return Err(gate("choose a cycle before recording"));
    }
    let mode = parse_mode(&choice.mode)?;
    let language = match choice.output_language.as_deref() {
        None => None,
        Some(l @ ("auto" | "pl" | "en")) => Some(l.to_string()),
        Some(other) => return Err(gate(format!("unknown output language {other:?}"))),
    };
    let located = locate(repos, &choice.cycle_id)?
        .ok_or_else(|| gate(format!("the cycle {} does not exist", choice.cycle_id)))?;
    let cycle = load_valid(&located).map_err(|e| gate(e.message))?;
    let templates = located.repo.list_templates(&cycle.id)?;
    let has = |name: &str| templates.iter().any(|t| t == name);

    let mut template = choice.template.clone().filter(|t| !t.is_empty());
    if let Some(t) = &template {
        if !has(t) {
            return Err(gate(format!("the cycle has no template named {t:?}")));
        }
    }
    let mut meeting_type = None;
    match mode {
        ReportMode::Template => {
            let name = template
                .clone()
                .unwrap_or_else(|| cycle.report.default_template.clone());
            if !has(&name) {
                return Err(gate(format!("the cycle has no template named {name:?}")));
            }
            template = Some(name);
        }
        ReportMode::ByType => {
            let kind = choice
                .meeting_type
                .clone()
                .filter(|k| !k.is_empty())
                .ok_or_else(|| gate("choose the meeting type for the by-type report"))?;
            if !cycle.report.by_type.contains_key(&kind) {
                return Err(gate(format!(
                    "the cycle does not define the meeting type {kind:?}"
                )));
            }
            // The meeting type picks the template.
            template = None;
            meeting_type = Some(kind);
        }
        ReportMode::Fixed | ReportMode::Auto => {}
    }
    Ok(ReportSettings {
        cycle_id: Some(cycle.id),
        report_mode: Some(mode_name(mode).to_string()),
        report_template: template,
        meeting_type,
        output_language: language,
    })
}

/// The report settings a recording starts with: the choice sent (or, when the caller allows
/// it, the last one), validated against the cycle. Refuses with `report_mode_required`.
pub fn prepare_recording(
    repos: &[CycleRepoSetting],
    sent: Option<ReportChoice>,
    use_last_report: bool,
    last: Option<&LastReport>,
) -> IpcResult<ReportSettings> {
    let choice = resolve_choice(sent, use_last_report, last)?;
    validate_report(repos, &choice)
}

/// What is remembered of a started recording's report settings.
pub fn last_report_of(settings: &ReportSettings) -> Option<LastReport> {
    Some(LastReport {
        cycle_id: settings.cycle_id.clone()?,
        mode: settings.report_mode.clone()?,
        template: settings.report_template.clone(),
        meeting_type: settings.meeting_type.clone(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingTypeDto {
    pub name: String,
    pub template: String,
}

/// What the report mode gate offers for a cycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportDefaults {
    pub cycle_id: String,
    pub default_mode: ReportMode,
    pub default_template: String,
    pub default_meeting_type: Option<String>,
    pub output_language: LanguageSetting,
    pub templates: Vec<TemplateSummaryDto>,
    pub meeting_types: Vec<MeetingTypeDto>,
    /// The last choice, when it was made for this cycle.
    pub last: Option<LastReport>,
}

pub fn report_defaults(
    repos: &[CycleRepoSetting],
    cycle_id: &str,
    last: Option<&LastReport>,
) -> IpcResult<ReportDefaults> {
    let located = locate_or_err(repos, cycle_id)?;
    let cycle = load_valid(&located)?;
    Ok(ReportDefaults {
        cycle_id: cycle.id.clone(),
        default_mode: cycle.report.default_mode,
        default_template: cycle.report.default_template.clone(),
        default_meeting_type: cycle.report.by_type.keys().next().cloned(),
        output_language: cycle.languages.output,
        templates: template_summaries(&located.repo, &cycle.id)?,
        meeting_types: cycle
            .report
            .by_type
            .iter()
            .map(|(k, v)| MeetingTypeDto {
                name: k.clone(),
                template: v.clone(),
            })
            .collect(),
        last: last.filter(|l| l.cycle_id == cycle.id).cloned(),
    })
}

// ----- the cycle's source for the queue ---------------------------------------------------

/// Everything the context of `extract` needs about a cycle, read from the registered
/// repositories: the cycle, its templates, topics, the cached open issues and the folder of
/// earlier meetings. With `pull_first`, a cloned repository is updated first (best effort:
/// offline use keeps what is there).
pub fn load_cycle_data(
    repos: &[CycleRepoSetting],
    cycle_id: &str,
    pull_first: bool,
) -> Result<Option<CycleData>, RunError> {
    let err = |e: &dyn std::fmt::Display| RunError::new(e.to_string());
    let Some(located) = locate(repos, cycle_id).map_err(|e| err(&e))? else {
        return Ok(None);
    };
    if pull_first && located.setting.kind == RepoKind::Clone {
        log_pull(cycle_id, located.repo.pull_rebase());
    }
    // The pull may have changed the files: read them afresh.
    let repo = &located.repo;
    let cycle = repo.load_cycle(cycle_id).map_err(|e| err(&e))?;
    let mut templates = Vec::new();
    for name in repo.list_templates(cycle_id).map_err(|e| err(&e))? {
        match repo.read_template(cycle_id, &name) {
            Ok(t) => templates.push(t),
            Err(e) => {
                tracing::warn!(cycle = cycle_id, template = %name, error = %e, "template skipped")
            }
        }
    }
    let topics = repo.load_topics(cycle_id).map_err(|e| err(&e))?;
    // The cache of the last fetch of the cycle's issues: open ones, closed numbers and the
    // issues made from meeting items.
    let cache = repo
        .issue_cache_path(cycle_id)
        .map(|p| IssueCache::load(&p))
        .unwrap_or_default();
    let closed_issue_ids = cache.closed.iter().map(u64::to_string).collect();
    let previous_dirs = meeting_dirs_in(repo, Some(&cycle), cycle_id);
    Ok(Some(CycleData {
        cycle,
        templates,
        topics,
        issues: cache.issues,
        closed_issue_ids,
        issue_links: cache.links,
        previous_dirs,
    }))
}

/// [`CycleSource`] over the repositories registered in the settings.
pub struct RepoCycleSource {
    repos: Box<dyn Fn() -> Vec<CycleRepoSetting> + Send + Sync>,
}

impl RepoCycleSource {
    pub fn new(repos: impl Fn() -> Vec<CycleRepoSetting> + Send + Sync + 'static) -> Self {
        Self {
            repos: Box::new(repos),
        }
    }
}

impl CycleSource for RepoCycleSource {
    fn load(&self, cycle_id: &str) -> Result<Option<CycleData>, RunError> {
        // A clone is brought up to date before the stage starts (see `Processing`), not here:
        // this runs while the store is locked.
        load_cycle_data(&(self.repos)(), cycle_id, false)
    }
}

// ----- adding repositories -----------------------------------------------------------------

/// A remote address that is fit to store: not empty, not an option for `git`, not a
/// remote-helper transport such as `ext::`, and without credentials in it (a user name of an
/// `https://` address counts: tokens are put there). Credentials belong to the Git
/// credential helper, not to the settings.
pub fn validate_remote(url: &str) -> IpcResult<String> {
    let url = url.trim();
    cycle_store::check_remote_url(url).map_err(|why| IpcError::new("invalid", why))?;
    Ok(url.to_string())
}

/// A readable, unique, file-name-safe id from a name or a URL.
pub fn slug(source: &str, taken: &[String], fallback: &str) -> String {
    let last = source
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\', ':'])
        .next()
        .unwrap_or(source);
    let last = last.strip_suffix(".git").unwrap_or(last);
    let mut base = String::new();
    for c in last.chars() {
        let c = match c {
            'ą' | 'Ą' => 'a',
            'ć' | 'Ć' => 'c',
            'ę' | 'Ę' => 'e',
            'ł' | 'Ł' => 'l',
            'ń' | 'Ń' => 'n',
            'ó' | 'Ó' => 'o',
            'ś' | 'Ś' => 's',
            'ź' | 'Ź' | 'ż' | 'Ż' => 'z',
            c => c,
        };
        if c.is_ascii_alphanumeric() {
            base.push(c.to_ascii_lowercase());
        } else if !base.ends_with('-') && !base.is_empty() {
            base.push('-');
        }
    }
    let base = base.trim_matches('-').to_string();
    let base = if base.is_empty() {
        fallback.to_string()
    } else {
        base
    };
    let mut id = base.clone();
    let mut n = 2;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&id)) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

/// Where a clone of repository `id` goes.
pub fn clone_destination(data_dir: &Path, id: &str) -> PathBuf {
    data_dir.join(CLONES_DIR).join(id)
}

fn repo_ids(settings: &Settings) -> Vec<String> {
    settings.cycle_repos.iter().map(|r| r.id.clone()).collect()
}

/// Registers an existing folder as a local repository.
pub fn add_local_repo(settings: &mut Settings, path: &str) -> IpcResult<CycleRepoSetting> {
    let path = path.trim();
    if path.is_empty() {
        return Err(IpcError::new(
            "invalid",
            "choose the folder of the repository",
        ));
    }
    let repo =
        Repo::open(path).map_err(|_| IpcError::new("invalid", "this folder does not exist"))?;
    let canonical = repo.root().to_string_lossy().into_owned();
    if settings
        .cycle_repos
        .iter()
        .any(|r| Path::new(&r.path) == repo.root())
    {
        return Err(IpcError::new(
            "duplicate",
            "this folder is already registered",
        ));
    }
    let id = slug(&canonical, &repo_ids(settings), "repository");
    let setting = CycleRepoSetting {
        id,
        path: canonical,
        remote: None,
        kind: RepoKind::Local,
        github_repo: None,
        github_api: None,
    };
    settings.cycle_repos.push(setting.clone());
    Ok(setting)
}

/// Registers a finished clone.
pub fn register_clone(
    settings: &mut Settings,
    id: &str,
    dest: &Path,
    url: &str,
) -> CycleRepoSetting {
    let setting = CycleRepoSetting {
        id: id.to_string(),
        path: dest.to_string_lossy().into_owned(),
        remote: Some(url.to_string()),
        kind: RepoKind::Clone,
        github_repo: None,
        github_api: None,
    };
    settings.cycle_repos.push(setting.clone());
    setting
}

/// The id and the destination for cloning `url`, chosen so they collide with nothing that
/// is registered or on disk.
pub fn plan_clone(settings: &Settings, data_dir: &Path, url: &str) -> (String, PathBuf) {
    let mut taken = repo_ids(settings);
    loop {
        let id = slug(url, &taken, "repository");
        let dest = clone_destination(data_dir, &id);
        if !dest.exists() {
            return (id, dest);
        }
        taken.push(id);
    }
}

/// Logs what a pull before the briefing did; it never stops the briefing.
fn log_pull(cycle_id: &str, result: cycle_store::Result<cycle_store::PullReport>) {
    match result {
        Ok(report) => match report.outcome {
            cycle_store::PullOutcome::Failed(why) => {
                tracing::warn!(cycle = cycle_id, %why, "pull failed, using the local state");
            }
            cycle_store::PullOutcome::SkippedDirty => {
                tracing::info!(
                    cycle = cycle_id,
                    "cycle edits are not committed: not pulling, using the local state"
                );
            }
            other => {
                tracing::info!(cycle = cycle_id, outcome = ?other, "pulled before the briefing");
                for w in &report.warnings {
                    tracing::warn!(cycle = cycle_id, "{w}");
                }
            }
        },
        Err(e) => tracing::warn!(cycle = cycle_id, error = %e, "pull failed"),
    }
}

/// Remembers for which attempt of which job a repository was already updated, so the
/// briefing of a job pulls once however often the queue looks at it (it looks every few
/// seconds while the job waits for the GPU).
#[derive(Debug, Default)]
pub struct PullLedger {
    done: std::sync::Mutex<std::collections::HashMap<String, u32>>,
}

impl PullLedger {
    /// `true` the first time for this meeting and `attempt`; `false` afterwards. A retried
    /// stage has a new attempt number and pulls again.
    pub fn first_time(&self, meeting_id: &str, attempt: u32) -> bool {
        let Ok(mut done) = self.done.lock() else {
            return false;
        };
        match done.get(meeting_id) {
            Some(seen) if *seen == attempt => false,
            _ => {
                done.insert(meeting_id.to_string(), attempt);
                true
            }
        }
    }
}

/// Brings the clone that holds `cycle_id` up to date before the briefing, once per
/// `(meeting, attempt)`. Only clones are pulled; a failure, a conflict or uncommitted cycle
/// edits only mean the local state is used. Takes no store lock.
pub fn pull_for_briefing(
    ledger: &PullLedger,
    repos: &[CycleRepoSetting],
    meeting_id: &str,
    attempt: u32,
    cycle_id: &str,
) {
    if !ledger.first_time(meeting_id, attempt) {
        return;
    }
    if let Ok(Some(found)) = locate(repos, cycle_id) {
        if found.setting.kind == RepoKind::Clone {
            log_pull(cycle_id, found.repo.pull_rebase());
        }
        // The state of the cycle's issues: what was closed since the last meeting is done.
        if let Some(list) = crate::issue_sync::refresh_cycle_issues(
            &crate::secrets::Credentials,
            &found.setting,
            &found.repo,
            cycle_id,
            crate::issue_sync::quick_client(),
        ) {
            match list.error {
                Some(why) => {
                    tracing::warn!(cycle = cycle_id, %why, "issues not refreshed, using the last known state");
                }
                None => tracing::info!(
                    cycle = cycle_id,
                    closed = list.closed.len(),
                    "issues refreshed"
                ),
            }
        }
    }
}

/// Pulls a repository (a failure is a result, not an error). The caller refreshes the
/// stored participants with [`sync_repo`] afterwards and adds its notes to the result.
///
/// Uncommitted cycle edits are never stashed: with such edits the pull is refused with
/// `repo_dirty` (commit or discard them first).
pub fn pull_repo(setting: &CycleRepoSetting) -> IpcResult<PullResult> {
    let repo = Repo::open(&setting.path)
        .map_err(|_| IpcError::new("not_found", "the folder of this repository is missing"))?;
    let report = repo.pull_rebase()?;
    let (outcome, error) = match report.outcome {
        cycle_store::PullOutcome::Updated => ("updated", None),
        cycle_store::PullOutcome::UpToDate => ("upToDate", None),
        cycle_store::PullOutcome::NoRemote => ("noRemote", None),
        cycle_store::PullOutcome::Failed(why) => ("failed", Some(why)),
        cycle_store::PullOutcome::SkippedDirty => {
            return Err(IpcError::new(
                "repo_dirty",
                "the cycle edits in this repository are not committed yet, so it was not updated; commit or discard them (for example in your Git client) and pull again",
            ));
        }
    };
    Ok(PullResult {
        repo: repo_dto(setting),
        outcome,
        error,
        warnings: report.warnings,
    })
}

// ----- participants of a cycle -------------------------------------------------------------

/// An id for a new participant, made from the name and different from the taken ones.
pub fn participant_slug(name: &str, taken: &[String]) -> String {
    slug(name, taken, "participant")
}

/// Finds a stored participant by its row id.
pub fn find_participant(store: &Store, id: &str) -> IpcResult<Option<ParticipantInfo>> {
    Ok(store.list_participants()?.into_iter().find(|p| p.id == id))
}

/// Adds a person to a cycle: into `cycle.yaml` (through `cycle-store`, validated) and into
/// the store. A person with the same name (ignoring case) is reused. Returns the stored row
/// whose `id` is the key voices are assigned to.
pub fn add_cycle_participant(
    repos: &[CycleRepoSetting],
    store: &Store,
    cycle_id: &str,
    name: &str,
) -> IpcResult<ParticipantInfo> {
    let name = name.trim();
    let located = locate_or_err(repos, cycle_id)?;
    let mut cycle = load_valid(&located)?;
    let existing = cycle
        .participants
        .iter()
        .find(|p| same_name(&p.name, name))
        .map(|p| p.id.clone());
    let id = match existing {
        Some(id) => id,
        None => {
            let taken: Vec<String> = cycle.participants.iter().map(|p| p.id.clone()).collect();
            let id = participant_slug(name, &taken);
            cycle.participants.push(Participant {
                id: id.clone(),
                name: name.to_string(),
                github: None,
                discord: None,
            });
            located.repo.save_cycle(&cycle)?;
            id
        }
    };
    store.sync_cycle_participants(cycle_id, &participant_pairs(&cycle))?;
    let key = store::cycle_participant_key(cycle_id, &id);
    find_participant(store, &key)?
        .ok_or_else(|| IpcError::new("internal", "the participant was not stored"))
}

/// Renames a person of a cycle in `cycle.yaml` and in the store. `duplicate` when another
/// participant of the cycle already has the name.
pub fn rename_cycle_participant(
    repos: &[CycleRepoSetting],
    store: &Store,
    info: &ParticipantInfo,
    name: &str,
) -> IpcResult<()> {
    let (cycle_id, id) = cycle_and_id(info)?;
    let name = name.trim();
    let located = locate_or_err(repos, cycle_id)?;
    let mut cycle = load_valid(&located)?;
    if cycle
        .participants
        .iter()
        .any(|p| p.id != id && same_name(&p.name, name))
    {
        return Err(IpcError::new(
            "duplicate",
            "a participant with this name already exists",
        ));
    }
    let person = cycle
        .participants
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| IpcError::new("not_found", "participant not found"))?;
    person.name = name.to_string();
    located.repo.save_cycle(&cycle)?;
    store.sync_cycle_participants(cycle_id, &participant_pairs(&cycle))?;
    Ok(())
}

/// Removes a person from `cycle.yaml` and deletes the stored participant with the voice
/// profile.
pub fn delete_cycle_participant(
    repos: &[CycleRepoSetting],
    store: &Store,
    info: &ParticipantInfo,
) -> IpcResult<()> {
    let (cycle_id, id) = cycle_and_id(info)?;
    if let Some(located) = locate(repos, cycle_id)? {
        let mut cycle = load_valid(&located)?;
        let before = cycle.participants.len();
        cycle.participants.retain(|p| p.id != id);
        if cycle.participants.len() != before {
            located.repo.save_cycle(&cycle)?;
        }
    }
    store.delete_participant(&info.id)?;
    Ok(())
}

fn cycle_and_id(info: &ParticipantInfo) -> IpcResult<(&str, &str)> {
    match (info.cycle_id.as_deref(), info.cycle_participant_id()) {
        (Some(c), Some(id)) => Ok((c, id)),
        _ => Err(IpcError::new(
            "invalid",
            "this participant does not belong to a cycle",
        )),
    }
}

// ----- tests -------------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use store::{Meeting, MeetingState};

    /// A temporary data folder with a default repository and a store.
    pub struct Lab {
        pub dir: tempfile::TempDir,
        pub settings: Settings,
        pub store: Store,
    }

    impl Lab {
        pub fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let mut settings = Settings::default();
            resolve_target_repo(&mut settings, None, dir.path()).unwrap();
            Self {
                dir,
                settings,
                store: Store::open_in_memory().unwrap(),
            }
        }

        pub fn repos(&self) -> &[CycleRepoSetting] {
            &self.settings.cycle_repos
        }

        pub fn make_cycle(&self, id: &str) -> CycleDetails {
            create_cycle(
                self.repos(),
                &self.settings.cycle_repos[0],
                id,
                "Nomad rover",
            )
            .unwrap()
        }

        pub fn meeting(&self, id: &str, cycle: Option<&str>) {
            self.store
                .create_meeting(&Meeting {
                    id: id.into(),
                    title: id.into(),
                    started_unix_ms: 1,
                    duration_ms: None,
                    state: MeetingState::Stopped,
                    dir: id.into(),
                })
                .unwrap();
            if let Some(c) = cycle {
                self.store
                    .set_report_settings(
                        id,
                        &ReportSettings {
                            cycle_id: Some(c.into()),
                            ..ReportSettings::default()
                        },
                    )
                    .unwrap();
            }
        }

        pub fn repo(&self) -> Repo {
            Repo::open(&self.settings.cycle_repos[0].path).unwrap()
        }
    }

    fn person(id: &str, name: &str) -> CycleParticipantDto {
        CycleParticipantDto {
            id: id.into(),
            name: name.into(),
            github: None,
            discord: None,
        }
    }

    fn choice(mode: &str) -> ReportChoice {
        ReportChoice {
            cycle_id: "NOMAD".into(),
            mode: mode.into(),
            ..ReportChoice::default()
        }
    }

    fn code(e: IpcError) -> &'static str {
        e.code
    }

    #[test]
    fn the_default_repository_is_created_lazily_and_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::default();
        assert!(!dir.path().join(DEFAULT_REPO_DIR).exists());
        let first = resolve_target_repo(&mut settings, None, dir.path()).unwrap();
        assert_eq!(first.id, DEFAULT_REPO_ID);
        assert_eq!(first.kind, RepoKind::Local);
        assert!(dir.path().join(DEFAULT_REPO_DIR).join(".git").exists());
        assert!(dir.path().join(DEFAULT_REPO_DIR).join("cycles").is_dir());
        let second = resolve_target_repo(&mut settings, None, dir.path()).unwrap();
        assert_eq!(first, second);
        assert_eq!(settings.cycle_repos.len(), 1);
        let e = resolve_target_repo(&mut settings, Some("nope"), dir.path()).unwrap_err();
        assert_eq!(e.code, "not_found");
    }

    #[test]
    fn creating_a_cycle_writes_the_three_files_and_lists_it() {
        let lab = Lab::new();
        let d = lab.make_cycle("NOMAD");
        assert_eq!(d.cycle.id, "NOMAD");
        assert!(d.valid && d.issues.is_empty());
        assert_eq!(d.repo_id, DEFAULT_REPO_ID);
        assert_eq!(d.templates.len(), 1);
        assert_eq!(d.templates[0].name, "default");
        assert!(d.topics.is_empty());
        let root = lab
            .dir
            .path()
            .join(DEFAULT_REPO_DIR)
            .join("cycles")
            .join("NOMAD");
        assert!(root.join("cycle.yaml").is_file());
        assert!(root.join("topics.yaml").is_file());
        assert!(root.join("templates").join("default.md").is_file());

        let list = list_cycles(lab.repos());
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "NOMAD");
        assert_eq!(list[0].name, "Nomad rover");
        assert_eq!(list[0].participants_count, 0);
        assert_eq!(list[0].default_mode, Some(ReportMode::Fixed));
        assert!(list[0].valid);
        // A local repository has no pull state and is never "offline".
        assert_eq!(list[0].state_as_of, None);
        assert!(!list[0].offline);
    }

    #[test]
    fn the_rule_of_a_target_is_set_in_cycle_yaml_and_nothing_else_changes() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let repo = lab.repo();
        let yaml = |repo: &Repo| {
            std::fs::read_to_string(repo.cycle_dir("NOMAD").unwrap().join("cycle.yaml")).unwrap()
        };
        let before = yaml(&repo);
        assert!(!before.contains("targets"), "a new cycle has no targets");

        // GitHub: the target is created with the default folder, then only the rule changes.
        let t = set_target_rule(lab.repos(), "NOMAD", "github", PublishMode::Auto).unwrap();
        let github = t.github.unwrap();
        assert_eq!(
            (github.publish, github.path.as_str(), github.issues),
            (PublishMode::Auto, "meetings", true)
        );
        let written = repo.load_cycle("NOMAD").unwrap();
        assert_eq!(written.targets.github.unwrap().publish, PublishMode::Auto);
        let t = set_target_rule(lab.repos(), "NOMAD", "github", PublishMode::Off).unwrap();
        assert_eq!(t.github.unwrap().publish, PublishMode::Off);
        // Setting the value it already has writes nothing.
        let text = yaml(&repo);
        set_target_rule(lab.repos(), "NOMAD", "github", PublishMode::Off).unwrap();
        assert_eq!(yaml(&repo), text);

        // Discord needs a target with a webhook reference.
        let e = set_target_rule(lab.repos(), "NOMAD", "discord", PublishMode::Auto).unwrap_err();
        assert_eq!(e.code, "no_target");
        let mut cycle = repo.load_cycle("NOMAD").unwrap();
        cycle.targets.discord = Some(DiscordTarget {
            webhook: Some("cred:discord/nomad".into()),
            bot: None,
            publish: PublishMode::OnApproval,
            template: Some("short".into()),
        });
        repo.save_cycle(&cycle).unwrap();
        let t = set_target_rule(lab.repos(), "NOMAD", "discord", PublishMode::Auto).unwrap();
        let discord = t.discord.unwrap();
        assert_eq!(discord.publish, PublishMode::Auto);
        assert_eq!(
            discord.webhook.as_deref(),
            Some("cred:discord/nomad"),
            "the reference is kept"
        );
        assert_eq!(discord.template.as_deref(), Some("short"));

        // Notion needs a target with a token and a database.
        let e = set_target_rule(lab.repos(), "NOMAD", "notion", PublishMode::Auto).unwrap_err();
        assert_eq!(e.code, "no_target");
        let mut cycle = repo.load_cycle("NOMAD").unwrap();
        cycle.targets.notion = Some(NotionTarget {
            token: "cred:notion/nomad".into(),
            database: "0123456789abcdef0123456789abcdef".into(),
            data_source: None,
            publish: PublishMode::OnApproval,
            properties: NotionProperties::default(),
        });
        repo.save_cycle(&cycle).unwrap();
        let t = set_target_rule(lab.repos(), "NOMAD", "notion", PublishMode::Off).unwrap();
        let notion = t.notion.unwrap();
        assert_eq!(notion.publish, PublishMode::Off);
        assert_eq!(notion.token, "cred:notion/nomad", "the reference is kept");
        assert_eq!(notion.properties, NotionPropertiesDto::default());
        let text = yaml(&repo);
        set_target_rule(lab.repos(), "NOMAD", "notion", PublishMode::Off).unwrap();
        assert_eq!(yaml(&repo), text, "the same rule writes nothing");

        // The rest of the file is as it was, and the other targets kept their rules.
        let after = repo.load_cycle("NOMAD").unwrap();
        assert_eq!(after.targets.github.unwrap().publish, PublishMode::Off);
        assert_eq!(
            after.targets.discord.unwrap().publish,
            PublishMode::Auto,
            "a Notion rule leaves Discord alone"
        );
        assert_eq!(after.name, "Nomad rover");

        // Other targets and unknown cycles are refused.
        for bad in ["", "Github", "slack"] {
            let e = set_target_rule(lab.repos(), "NOMAD", bad, PublishMode::Auto).unwrap_err();
            assert_eq!(e.code, "invalid", "{bad:?}");
        }
        let e = set_target_rule(lab.repos(), "GHOST", "github", PublishMode::Auto).unwrap_err();
        assert_eq!(e.code, "not_found");
    }

    #[test]
    fn a_cycle_that_cannot_be_used_is_not_rewritten_by_a_rule_change() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let path = lab.repo().cycle_dir("NOMAD").unwrap().join("cycle.yaml");
        std::fs::write(
            &path,
            "id: NOMAD
name: Nomad
targets:
  github: { path: \"../x\" }
",
        )
        .unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let e = set_target_rule(lab.repos(), "NOMAD", "github", PublishMode::Auto).unwrap_err();
        assert_eq!(e.code, "cycle_invalid");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn creating_refuses_bad_ids_empty_names_and_duplicates() {
        let lab = Lab::new();
        let target = lab.settings.cycle_repos[0].clone();
        for bad in ["", "a b", "../x", "a.b", "-x"] {
            assert_eq!(
                code(create_cycle(lab.repos(), &target, bad, "n").unwrap_err()),
                "invalid",
                "{bad:?}"
            );
        }
        assert_eq!(
            code(create_cycle(lab.repos(), &target, "OK", "  ").unwrap_err()),
            "invalid"
        );
        lab.make_cycle("NOMAD");
        assert_eq!(
            code(create_cycle(lab.repos(), &target, "NOMAD", "again").unwrap_err()),
            "duplicate"
        );
    }

    #[test]
    fn the_dto_round_trips_through_cycle_yaml_in_camel_case() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let mut dto = get_cycle(lab.repos(), "NOMAD").unwrap().cycle;
        dto.participants = vec![CycleParticipantDto {
            github: Some("alex-example".into()),
            discord: Some(DiscordRefDto {
                user_id: Some("123456789012345678".into()),
                role_id: None,
            }),
            ..person("alex", "Alex Example")
        }];
        dto.glossary = vec![GlossaryDto {
            term: "CAN bus".into(),
            hint: None,
        }];
        dto.report.by_type.insert("budget".into(), "default".into());
        dto.targets.discord = Some(DiscordTargetDto {
            webhook: Some("cred:discord/nomad".into()),
            bot: None,
            publish: PublishMode::OnApproval,
            template: None,
        });
        let r = save_cycle(lab.repos(), &lab.store, dto.clone()).unwrap();
        assert!(r.saved, "{:?}", r.issues);
        assert_eq!(r.details.as_ref().unwrap().cycle, dto);

        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["report"]["defaultMode"], "fixed");
        assert_eq!(json["report"]["byType"]["budget"], "default");
        assert_eq!(
            json["participants"][0]["discord"]["userId"],
            "123456789012345678"
        );
        assert!(json["participants"][0]["discord"].get("roleId").is_none());
        assert_eq!(json["targets"]["discord"]["publish"], "on-approval");
        // The file holds the reference, never a URL.
        let yaml = std::fs::read_to_string(
            lab.dir
                .path()
                .join(DEFAULT_REPO_DIR)
                .join("cycles/NOMAD/cycle.yaml"),
        )
        .unwrap();
        assert!(yaml.contains("cred:discord/nomad"));
    }

    #[test]
    fn the_bot_and_notion_targets_round_trip_through_the_dto() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let mut dto = get_cycle(lab.repos(), "NOMAD").unwrap().cycle;
        dto.targets.discord = Some(DiscordTargetDto {
            webhook: None,
            bot: Some(DiscordBotDto {
                token: "cred:discord-bot/team".into(),
                guild: "111111111111111111".into(),
                channel: "222222222222222222".into(),
                thread: true,
                edit_on_correction: false,
            }),
            publish: PublishMode::OnApproval,
            template: None,
        });
        dto.targets.notion = Some(NotionTargetDto {
            token: "cred:notion/nomad".into(),
            database: "0123456789abcdef0123456789abcdef".into(),
            data_source: Some("fedcba98-7654-3210-fedc-ba9876543210".into()),
            publish: PublishMode::Auto,
            properties: NotionPropertiesDto {
                assignee: Some("Assignee".into()),
                status: Some("Status".into()),
                status_value: Some("Not started".into()),
                ..NotionPropertiesDto::default()
            },
        });
        let r = save_cycle(lab.repos(), &lab.store, dto.clone()).unwrap();
        assert!(r.saved, "{:?}", r.issues);
        assert_eq!(r.details.as_ref().unwrap().cycle, dto);

        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["targets"]["discord"]["bot"]["editOnCorrection"], false);
        assert_eq!(json["targets"]["discord"]["bot"]["thread"], true);
        assert!(json["targets"]["discord"].get("webhook").is_none());
        assert_eq!(json["targets"]["notion"]["properties"]["title"], "Name");
        assert_eq!(
            json["targets"]["notion"]["properties"]["statusValue"],
            "Not started"
        );
        assert!(json["targets"]["notion"]["properties"].get("due").is_none());
        assert_eq!(
            json["targets"]["notion"]["dataSource"],
            "fedcba98-7654-3210-fedc-ba9876543210"
        );

        // Empty names and ids from the editor mean "not set".
        let mut blank = dto.clone();
        {
            let notion = blank.targets.notion.as_mut().unwrap();
            notion.data_source = Some("  ".into());
            notion.properties.assignee = Some(String::new());
            notion.properties.title = String::new();
            notion.properties.status = None;
            notion.properties.status_value = None;
        }
        let cycle = Cycle::from(blank);
        let notion = cycle.targets.notion.unwrap();
        assert_eq!(notion.data_source, None);
        assert_eq!(notion.properties, NotionProperties::default());

        // Both a webhook and a bot is refused by the schema, nothing is written.
        let mut both = dto;
        both.targets.discord.as_mut().unwrap().webhook = Some("cred:discord/nomad".into());
        let r = save_cycle(lab.repos(), &lab.store, both).unwrap();
        assert!(!r.saved);
        assert!(r.issues.iter().any(|i| i.path == "targets.discord"));
    }

    #[test]
    fn an_invalid_cycle_is_not_written_and_returns_issues() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let path = lab
            .dir
            .path()
            .join(DEFAULT_REPO_DIR)
            .join("cycles/NOMAD/cycle.yaml");
        let before = std::fs::read_to_string(&path).unwrap();

        let mut dto = get_cycle(lab.repos(), "NOMAD").unwrap().cycle;
        dto.participants = vec![person("alex", "Alex"), person("alex", "Alex B")];
        dto.report.default_mode = ReportMode::Template;
        dto.report.default_template = "missing".into();
        dto.report.by_type.insert("budget".into(), "nope".into());
        let r = save_cycle(lab.repos(), &lab.store, dto).unwrap();
        assert!(!r.saved);
        assert!(r.details.is_none());
        let paths: Vec<&str> = r.issues.iter().map(|i| i.path.as_str()).collect();
        assert!(paths.contains(&"participants[1].id"), "{paths:?}");
        assert!(paths.contains(&"report.default_template"), "{paths:?}");
        assert!(paths.contains(&"report.by_type.budget"), "{paths:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        assert!(lab
            .store
            .participants_for_cycle("NOMAD")
            .unwrap()
            .is_empty());

        // Two people with the same name cannot be told apart in a transcript.
        let mut dto = get_cycle(lab.repos(), "NOMAD").unwrap().cycle;
        dto.participants = vec![person("a", "Alex"), person("b", "alex")];
        let r = save_cycle(lab.repos(), &lab.store, dto).unwrap();
        assert!(!r.saved);
        assert_eq!(r.issues[0].path, "participants[1].name");

        // And a bad id is reported with its path.
        let mut dto = get_cycle(lab.repos(), "NOMAD").unwrap().cycle;
        dto.participants = vec![person("bad id", "Alex")];
        let r = save_cycle(lab.repos(), &lab.store, dto).unwrap();
        assert!(!r.saved && !r.issues.is_empty());
    }

    #[test]
    fn saving_syncs_the_participants_into_the_store() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let mut dto = get_cycle(lab.repos(), "NOMAD").unwrap().cycle;
        dto.participants = vec![person("alex", "Alex"), person("sam", "Sam")];
        assert!(save_cycle(lab.repos(), &lab.store, dto).unwrap().saved);
        let rows = lab.store.participants_for_cycle("NOMAD").unwrap();
        let names: Vec<&str> = rows.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Alex", "Sam"]);
        assert_eq!(rows[0].cycle_participant_id(), Some("alex"));

        // A rename keeps the row, so the voice profile stays.
        let mut dto = get_cycle(lab.repos(), "NOMAD").unwrap().cycle;
        dto.participants[0].name = "Alexander".into();
        assert!(save_cycle(lab.repos(), &lab.store, dto).unwrap().saved);
        let rows = lab.store.participants_for_cycle("NOMAD").unwrap();
        assert!(rows
            .iter()
            .any(|p| p.name == "Alexander" && p.id == "NOMAD:alex"));

        // Saving an unchanged cycle does not change the file.
        let path = lab
            .dir
            .path()
            .join(DEFAULT_REPO_DIR)
            .join("cycles/NOMAD/cycle.yaml");
        let before = std::fs::read_to_string(&path).unwrap();
        let dto = get_cycle(lab.repos(), "NOMAD").unwrap().cycle;
        assert!(save_cycle(lab.repos(), &lab.store, dto).unwrap().saved);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn saving_an_unknown_cycle_is_not_found() {
        let lab = Lab::new();
        let dto = CycleDto::from(&Cycle::new("GHOST", "Ghost"));
        assert_eq!(
            code(save_cycle(lab.repos(), &lab.store, dto).unwrap_err()),
            "not_found"
        );
    }

    #[test]
    fn a_broken_cycle_is_listed_with_its_issues_but_cannot_be_used() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let path = lab
            .dir
            .path()
            .join(DEFAULT_REPO_DIR)
            .join("cycles/NOMAD/cycle.yaml");
        std::fs::write(
            &path,
            "id: NOMAD\nname: Nomad\nparticipants:\n  - {id: a, name: A}\n  - {id: a, name: B}\n",
        )
        .unwrap();
        let list = list_cycles(lab.repos());
        assert!(!list[0].valid);
        assert_eq!(list[0].participants_count, 2);
        assert_eq!(list[0].issues[0].path, "participants[1].id");
        // The editor can still open it to fix it.
        let d = get_cycle(lab.repos(), "NOMAD").unwrap();
        assert!(!d.valid);
        // But a recording cannot start on it, and the context cannot be built.
        assert_eq!(
            code(validate_report(lab.repos(), &choice("fixed")).unwrap_err()),
            "report_mode_required"
        );
        assert!(load_cycle_data(lab.repos(), "NOMAD", false).is_err());

        std::fs::write(&path, "id: NOMAD\nname: [unclosed\n").unwrap();
        let list = list_cycles(lab.repos());
        assert!(!list[0].valid);
        assert_eq!(list[0].name, "NOMAD");
        assert_eq!(list[0].default_mode, None);
        assert_eq!(
            code(get_cycle(lab.repos(), "NOMAD").unwrap_err()),
            "cycle_invalid"
        );
    }

    #[test]
    fn templates_are_listed_read_validated_and_saved() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let t = TemplateDto {
            name: "weekly".into(),
            sections: vec![
                TemplateSectionDto {
                    id: "progress".into(),
                    title: "Progress".into(),
                    description: "What was done".into(),
                },
                TemplateSectionDto {
                    id: "blockers".into(),
                    title: "Blockers".into(),
                    description: String::new(),
                },
            ],
            body: "# {title}\n\n{section:progress}\n\n{section:blockers}\n".into(),
        };
        let check = validate_template(&t);
        assert!(check.valid && !check.saved);
        assert!(check.placeholders.iter().any(|p| p == "summary"));
        assert!(check.placeholders.iter().any(|p| p == "summary_section"));
        let saved = save_template(lab.repos(), "NOMAD", &t).unwrap();
        assert!(saved.valid && saved.saved);
        assert_eq!(get_template(lab.repos(), "NOMAD", "weekly").unwrap(), t);
        let list = list_templates(lab.repos(), "NOMAD").unwrap();
        let names: Vec<&str> = list.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["default", "weekly"]);
        assert_eq!(list[1].sections.len(), 2);
        assert_eq!(list[1].sections[0].title, "Progress");

        // An unknown placeholder, a section that is not declared and a bad name are issues.
        let mut bad = t.clone();
        bad.body = "{nonsense} {section:ghost}".into();
        let check = validate_template(&bad);
        assert!(!check.valid);
        assert!(
            check.issues[0].message.contains("nonsense"),
            "{:?}",
            check.issues
        );
        let r = save_template(lab.repos(), "NOMAD", &bad).unwrap();
        assert!(!r.saved && !r.valid);
        let mut undeclared = t.clone();
        undeclared.body = "{section:ghost}".into();
        assert!(!validate_template(&undeclared).valid);
        assert_eq!(get_template(lab.repos(), "NOMAD", "weekly").unwrap(), t);
        let mut bad = t.clone();
        bad.name = "../evil".into();
        assert!(!validate_template(&bad).valid);
        assert!(!save_template(lab.repos(), "NOMAD", &bad).unwrap().saved);
        assert_eq!(
            code(get_template(lab.repos(), "NOMAD", "missing").unwrap_err()),
            "not_found"
        );
    }

    fn weekly_cycle(lab: &Lab) {
        lab.make_cycle("NOMAD");
        save_template(
            lab.repos(),
            "NOMAD",
            &TemplateDto {
                name: "weekly".into(),
                sections: vec![TemplateSectionDto {
                    id: "progress".into(),
                    title: "Progress".into(),
                    description: String::new(),
                }],
                body: "# {title}\n\n{section:progress}\n".into(),
            },
        )
        .unwrap();
        let mut dto = get_cycle(lab.repos(), "NOMAD").unwrap().cycle;
        dto.report.default_template = "weekly".into();
        dto.report.by_type.insert("budget".into(), "weekly".into());
        assert!(save_cycle(lab.repos(), &lab.store, dto).unwrap().saved);
    }

    #[test]
    fn the_gate_refuses_with_the_stable_code_in_every_case() {
        let lab = Lab::new();
        weekly_cycle(&lab);
        let refused = |c: ReportChoice| {
            let e = validate_report(lab.repos(), &c).unwrap_err();
            assert_eq!(e.code, "report_mode_required", "{}", e.message);
        };
        // No cycle, an unknown cycle, a bad id.
        refused(ReportChoice::default());
        refused(ReportChoice {
            cycle_id: "GHOST".into(),
            mode: "fixed".into(),
            ..ReportChoice::default()
        });
        refused(ReportChoice {
            cycle_id: "../x".into(),
            mode: "fixed".into(),
            ..ReportChoice::default()
        });
        // No mode, an unknown mode.
        refused(choice(""));
        refused(choice("freeform"));
        // A bad language.
        refused(ReportChoice {
            output_language: Some("de".into()),
            ..choice("fixed")
        });
        // A template that does not exist, in any mode.
        refused(ReportChoice {
            template: Some("ghost".into()),
            ..choice("template")
        });
        refused(ReportChoice {
            template: Some("ghost".into()),
            ..choice("fixed")
        });
        // By-type without a type, or with one the cycle does not define.
        refused(choice("by-type"));
        refused(ReportChoice {
            meeting_type: Some("casual".into()),
            ..choice("by-type")
        });
        // Missing everywhere: nothing sent and no last choice.
        assert_eq!(
            resolve_choice(None, false, None).unwrap_err().code,
            "report_mode_required"
        );
        let last = LastReport {
            cycle_id: "NOMAD".into(),
            mode: "fixed".into(),
            template: None,
            meeting_type: None,
        };
        assert_eq!(
            resolve_choice(None, false, Some(&last)).unwrap_err().code,
            "report_mode_required"
        );
        assert_eq!(
            resolve_choice(None, true, None).unwrap_err().code,
            "report_mode_required"
        );
    }

    #[test]
    fn the_gate_accepts_valid_choices_and_resolves_them() {
        let lab = Lab::new();
        weekly_cycle(&lab);
        let fixed = validate_report(lab.repos(), &choice("fixed")).unwrap();
        assert_eq!(fixed.cycle_id.as_deref(), Some("NOMAD"));
        assert_eq!(fixed.report_mode.as_deref(), Some("fixed"));
        assert_eq!(fixed.report_template, None);

        // Template mode without a name takes the cycle's default template.
        let t = validate_report(lab.repos(), &choice("template")).unwrap();
        assert_eq!(t.report_template.as_deref(), Some("weekly"));
        let t = validate_report(
            lab.repos(),
            &ReportChoice {
                template: Some("default".into()),
                output_language: Some("pl".into()),
                ..choice("template")
            },
        )
        .unwrap();
        assert_eq!(t.report_template.as_deref(), Some("default"));
        assert_eq!(t.output_language.as_deref(), Some("pl"));

        let auto = validate_report(lab.repos(), &choice("auto")).unwrap();
        assert_eq!(auto.report_mode.as_deref(), Some("auto"));

        // By type: the type is kept, the template comes from the type.
        let b = validate_report(
            lab.repos(),
            &ReportChoice {
                meeting_type: Some("budget".into()),
                template: Some("default".into()),
                ..choice("by-type")
            },
        )
        .unwrap();
        assert_eq!(b.meeting_type.as_deref(), Some("budget"));
        assert_eq!(b.report_template, None);

        // The result is exactly what the store accepts.
        lab.meeting("m1", None);
        assert!(lab.store.set_report_settings("m1", &b).unwrap());
    }

    #[test]
    fn the_last_report_is_remembered_and_can_stand_in_when_allowed() {
        let lab = Lab::new();
        weekly_cycle(&lab);
        let settings = validate_report(
            lab.repos(),
            &ReportChoice {
                meeting_type: Some("budget".into()),
                ..choice("by-type")
            },
        )
        .unwrap();
        let last = last_report_of(&settings).unwrap();
        assert_eq!(
            last,
            LastReport {
                cycle_id: "NOMAD".into(),
                mode: "by-type".into(),
                template: None,
                meeting_type: Some("budget".into()),
            }
        );
        assert_eq!(last_report_of(&ReportSettings::default()), None);

        // The widget sends nothing and asks for the last settings.
        let again = resolve_choice(None, true, Some(&last)).unwrap();
        let validated = validate_report(lab.repos(), &again).unwrap();
        assert_eq!(validated, settings);
        // Still validated: a last choice for a cycle that is gone is refused.
        let gone = LastReport {
            cycle_id: "GHOST".into(),
            ..last.clone()
        };
        let c = resolve_choice(None, true, Some(&gone)).unwrap();
        assert_eq!(
            validate_report(lab.repos(), &c).unwrap_err().code,
            "report_mode_required"
        );
        // A choice that was sent wins over the last one.
        let sent = resolve_choice(Some(choice("auto")), true, Some(&last)).unwrap();
        assert_eq!(sent.mode, "auto");
    }

    #[test]
    fn report_defaults_offer_the_cycles_values_and_the_allowed_choices() {
        let lab = Lab::new();
        weekly_cycle(&lab);
        let last = LastReport {
            cycle_id: "NOMAD".into(),
            mode: "auto".into(),
            template: None,
            meeting_type: None,
        };
        let d = report_defaults(lab.repos(), "NOMAD", Some(&last)).unwrap();
        assert_eq!(d.default_mode, ReportMode::Fixed);
        assert_eq!(d.default_template, "weekly");
        assert_eq!(d.default_meeting_type.as_deref(), Some("budget"));
        assert_eq!(d.output_language, LanguageSetting::Auto);
        assert_eq!(d.templates.len(), 2);
        assert_eq!(d.meeting_types.len(), 1);
        assert_eq!(d.meeting_types[0].template, "weekly");
        assert_eq!(d.last, Some(last.clone()));
        let other = LastReport {
            cycle_id: "OTHER".into(),
            ..last
        };
        assert_eq!(
            report_defaults(lab.repos(), "NOMAD", Some(&other))
                .unwrap()
                .last,
            None
        );
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["defaultMode"], "fixed");
        assert_eq!(json["meetingTypes"][0]["name"], "budget");
        assert_eq!(
            report_defaults(lab.repos(), "GHOST", None)
                .unwrap_err()
                .code,
            "not_found"
        );
    }

    #[test]
    fn creating_a_participant_from_the_labeling_screen_writes_cycle_yaml_and_the_store() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let a =
            add_cycle_participant(lab.repos(), &lab.store, "NOMAD", "  Zażółć Gęślą  ").unwrap();
        assert_eq!(a.name, "Zażółć Gęślą");
        assert_eq!(a.id, "NOMAD:zazolc-gesla");
        assert_eq!(a.cycle_id.as_deref(), Some("NOMAD"));
        let cycle = get_cycle(lab.repos(), "NOMAD").unwrap().cycle;
        assert_eq!(
            cycle.participants,
            vec![person("zazolc-gesla", "Zażółć Gęślą")]
        );

        // The same name (any case) reuses the person; a different one gets a unique id.
        let again =
            add_cycle_participant(lab.repos(), &lab.store, "NOMAD", "ZAŻÓŁĆ gęślą").unwrap();
        assert_eq!(again.id, a.id);
        let b = add_cycle_participant(lab.repos(), &lab.store, "NOMAD", "Alex").unwrap();
        let c = add_cycle_participant(lab.repos(), &lab.store, "NOMAD", "alex.").unwrap();
        assert_eq!(b.id, "NOMAD:alex");
        assert_eq!(c.id, "NOMAD:alex-2");
        assert_eq!(
            get_cycle(lab.repos(), "NOMAD")
                .unwrap()
                .cycle
                .participants
                .len(),
            3
        );
        assert_eq!(lab.store.participants_for_cycle("NOMAD").unwrap().len(), 3);

        assert_eq!(
            add_cycle_participant(lab.repos(), &lab.store, "GHOST", "X")
                .unwrap_err()
                .code,
            "not_found"
        );
    }

    #[test]
    fn participants_rename_and_delete_within_the_cycle() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let a = add_cycle_participant(lab.repos(), &lab.store, "NOMAD", "Alex").unwrap();
        let s = add_cycle_participant(lab.repos(), &lab.store, "NOMAD", "Sam").unwrap();

        assert_eq!(
            rename_cycle_participant(lab.repos(), &lab.store, &a, "sam")
                .unwrap_err()
                .code,
            "duplicate"
        );
        rename_cycle_participant(lab.repos(), &lab.store, &a, "Alexander").unwrap();
        let cycle = get_cycle(lab.repos(), "NOMAD").unwrap().cycle;
        assert!(cycle
            .participants
            .iter()
            .any(|p| p.id == "alex" && p.name == "Alexander"));
        let rows = lab.store.participants_for_cycle("NOMAD").unwrap();
        assert!(rows.iter().any(|p| p.id == a.id && p.name == "Alexander"));

        delete_cycle_participant(lab.repos(), &lab.store, &s).unwrap();
        let cycle = get_cycle(lab.repos(), "NOMAD").unwrap().cycle;
        assert_eq!(cycle.participants.len(), 1);
        assert_eq!(lab.store.participants_for_cycle("NOMAD").unwrap().len(), 1);

        // A legacy participant is not a cycle's.
        lab.store.create_participant("p1", "Old", 1).unwrap();
        let old = find_participant(&lab.store, "p1").unwrap().unwrap();
        assert_eq!(
            rename_cycle_participant(lab.repos(), &lab.store, &old, "New")
                .unwrap_err()
                .code,
            "invalid"
        );
    }

    #[test]
    fn naming_a_voice_after_a_new_person_uses_the_meetings_cycle() {
        // The speaker command's flow: the cycle of the meeting, then the voice assignment.
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        lab.meeting("m1", Some("NOMAD"));
        lab.store
            .put_meeting_voices(
                "m1",
                &[store::VoiceRow {
                    voice_id: "discord:SPEAKER_00".into(),
                    track: "discord".into(),
                    label: "SPEAKER_00".into(),
                    speech_ms: 1000,
                    embedding: vec![0.1, 0.2],
                    decision: store::VoiceDecision::Unknown,
                    score: None,
                    matched_participant: None,
                    assigned_participant: None,
                    confirmed: false,
                    merged_into: None,
                }],
            )
            .unwrap();
        let cycle = lab
            .store
            .get_report_settings("m1")
            .unwrap()
            .unwrap()
            .cycle_id
            .unwrap();
        let p = add_cycle_participant(lab.repos(), &lab.store, &cycle, "Alex").unwrap();
        assert!(queue::voices::assign_voice(
            &lab.store,
            "m1",
            "discord:SPEAKER_00",
            Some(&p.id),
            5
        )
        .unwrap());
        let voices = lab.store.list_meeting_voices("m1").unwrap();
        assert_eq!(
            voices[0].assigned_participant.as_deref(),
            Some("NOMAD:alex")
        );
        let names = queue::voices::speaker_names(
            &voices,
            &queue::voices::participants_for_meeting(&lab.store, "m1").unwrap(),
        );
        assert_eq!(names["discord:SPEAKER_00"], "Alex");
        assert_eq!(
            get_cycle(lab.repos(), "NOMAD").unwrap().cycle.participants[0].id,
            "alex"
        );
    }

    #[test]
    fn the_cycle_source_loads_the_cycle_templates_topics_issues_and_meeting_folder() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        assert!(load_cycle_data(lab.repos(), "GHOST", false)
            .unwrap()
            .is_none());
        let data = load_cycle_data(lab.repos(), "NOMAD", false)
            .unwrap()
            .unwrap();
        assert_eq!(data.cycle.id, "NOMAD");
        assert_eq!(data.templates.len(), 1);
        assert!(data.topics.is_empty() && data.issues.is_empty());
        // No meetings folder yet: nothing to read.
        assert!(data.previous_dirs.is_empty());

        // Cached issues come from inside .git; the meetings folder once it exists.
        let repo = lab.repo();
        IssueCache {
            repo: "o/r".into(),
            label: "cycle:NOMAD".into(),
            fetched_unix_ms: Some(1),
            issues: vec![cycle_store::OpenIssue {
                number: 12,
                title: "Wheel size".into(),
                assignee: None,
                url: None,
            }],
            ..IssueCache::default()
        }
        .save(&repo.issue_cache_path("NOMAD").unwrap())
        .unwrap();
        repo.save_topics(
            "NOMAD",
            &[Topic {
                id: "t-1".into(),
                text: "Wheel size".into(),
                status: TopicStatus::Open,
                opened_in: "2026-11-06".into(),
                closed_in: None,
            }],
        )
        .unwrap();
        std::fs::create_dir_all(repo.meetings_dir("NOMAD").unwrap()).unwrap();
        let data = load_cycle_data(lab.repos(), "NOMAD", false)
            .unwrap()
            .unwrap();
        assert_eq!(data.issues.len(), 1);
        assert_eq!(data.topics.len(), 1);
        assert_eq!(
            data.previous_dirs,
            vec![repo.meetings_dir("NOMAD").unwrap()]
        );

        // A cycle that publishes to another folder reads the meetings from there as well as
        // from the default one, where the meetings of before the change are.
        let mut cycle = repo.load_cycle("NOMAD").unwrap();
        cycle.targets.github = Some(GithubTarget {
            path: "docs/notes".into(),
            ..GithubTarget::default()
        });
        repo.save_cycle(&cycle).unwrap();
        let other = repo.meetings_dir_at("docs/notes", "NOMAD").unwrap();
        assert!(other.ends_with("docs/notes/NOMAD"));
        assert_eq!(
            load_cycle_data(lab.repos(), "NOMAD", false)
                .unwrap()
                .unwrap()
                .previous_dirs,
            vec![repo.meetings_dir("NOMAD").unwrap()],
            "a folder that does not exist yet is left out"
        );
        std::fs::create_dir_all(&other).unwrap();
        let expected = vec![other, repo.meetings_dir("NOMAD").unwrap()];
        assert_eq!(
            load_cycle_data(lab.repos(), "NOMAD", false)
                .unwrap()
                .unwrap()
                .previous_dirs,
            expected
        );
        assert_eq!(previous_meeting_dirs(lab.repos(), "NOMAD"), expected);

        // Through the trait, as the runner calls it (a local repository never pulls).
        let repos = lab.settings.cycle_repos.clone();
        let source = RepoCycleSource::new(move || repos.clone());
        assert!(source.load("NOMAD").unwrap().is_some());
        assert!(source.load("GHOST").unwrap().is_none());
    }

    #[test]
    fn closed_issues_and_item_links_of_the_cache_reach_the_context_data() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let repo = lab.repo();
        // A cache written before closed issues were known loads with nothing closed.
        std::fs::write(
            repo.issue_cache_path("NOMAD").unwrap(),
            r#"{"repo":"o/r","label":"cycle:NOMAD","fetched_unix_ms":1,"issues":[{"number":12,"title":"t"}]}"#,
        )
        .unwrap();
        let data = load_cycle_data(lab.repos(), "NOMAD", false)
            .unwrap()
            .unwrap();
        assert!(data.closed_issue_ids.is_empty() && data.issue_links.is_empty());

        let link = cycle_store::IssueLink {
            number: 41,
            meeting: "20261113-140501".into(),
            item: "i1".into(),
            open: false,
        };
        IssueCache {
            repo: "o/r".into(),
            label: "cycle:NOMAD".into(),
            fetched_unix_ms: Some(2),
            closed: vec![40, 41],
            links: vec![link.clone()],
            ..IssueCache::default()
        }
        .save(&repo.issue_cache_path("NOMAD").unwrap())
        .unwrap();
        let data = load_cycle_data(lab.repos(), "NOMAD", false)
            .unwrap()
            .unwrap();
        assert_eq!(data.closed_issue_ids, ["40", "41"]);
        assert_eq!(data.issue_links, vec![link]);
    }

    #[test]
    fn the_context_of_a_meeting_is_built_from_the_registered_repository() {
        // End to end through the queue's own step, with the application's source.
        let lab = Lab::new();
        weekly_cycle(&lab);
        lab.meeting("m1", None);
        lab.store
            .set_report_settings(
                "m1",
                &validate_report(
                    lab.repos(),
                    &ReportChoice {
                        meeting_type: Some("budget".into()),
                        ..choice("by-type")
                    },
                )
                .unwrap(),
            )
            .unwrap();
        let root = lab.dir.path().join("recordings");
        std::fs::create_dir_all(root.join("m1")).unwrap();
        let repos = lab.settings.cycle_repos.clone();
        let source = RepoCycleSource::new(move || repos.clone());
        let env = queue::context::ContextEnv {
            meeting_root: &root,
            utc_offset_minutes: 0,
        };
        let outcome = queue::context::prepare_context(&lab.store, &source, &env, "m1").unwrap();
        assert!(matches!(
            outcome,
            queue::context::ContextOutcome::Written { .. }
        ));
        let ctx: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("m1/work/context.json")).unwrap())
                .unwrap();
        assert_eq!(ctx["cycle"], "NOMAD");
        assert_eq!(ctx["mode"], "by-type");
        assert_eq!(ctx["template"], "weekly");
    }

    #[test]
    fn remote_addresses_with_passwords_or_option_look_alikes_are_refused() {
        assert!(validate_remote("https://github.com/org/repo.git").is_ok());
        assert!(validate_remote("git@github.com:org/repo.git").is_ok());
        assert!(validate_remote("ssh://git@github.com/org/repo.git").is_ok());
        assert_eq!(validate_remote("").unwrap_err().code, "invalid");
        // A token is usually put in as the user name.
        for bad in [
            "https://user@host/org/repo.git",
            "https://ghp_abc123@github.com/org/repo.git",
            "ext::sh -c touch% /tmp/pwned",
            "fd::17/foo",
        ] {
            let e = validate_remote(bad).unwrap_err();
            assert_eq!(e.code, "invalid", "{bad}");
            assert!(!e.message.contains("ghp_abc123"));
        }
        assert_eq!(
            validate_remote("--upload-pack=x").unwrap_err().code,
            "invalid"
        );
        let e = validate_remote("https://user:hunter2@host/r.git").unwrap_err();
        assert_eq!(e.code, "invalid");
        assert!(!e.message.contains("hunter2"));
    }

    #[test]
    fn repository_ids_come_from_the_url_and_stay_unique() {
        let none: Vec<String> = Vec::new();
        assert_eq!(
            slug("https://github.com/org/Team-Cycles.git", &none, "r"),
            "team-cycles"
        );
        assert_eq!(slug("git@github.com:org/cycles.git", &none, "r"), "cycles");
        assert_eq!(slug("C:\\Users\\me\\My Cycles\\", &none, "r"), "my-cycles");
        assert_eq!(slug("!!!", &none, "repository"), "repository");
        let taken = vec!["cycles".to_string(), "cycles-2".to_string()];
        assert_eq!(slug("x/cycles", &taken, "r"), "cycles-3");
        assert!(is_identifier(&slug("x/Ünï", &none, "r")));
    }

    #[test]
    fn a_local_folder_is_registered_once() {
        let lab = Lab::new();
        let other = lab.dir.path().join("team-cycles");
        std::fs::create_dir_all(&other).unwrap();
        let mut settings = lab.settings.clone();
        let added = add_local_repo(&mut settings, other.to_str().unwrap()).unwrap();
        assert_eq!(added.id, "team-cycles");
        assert_eq!(added.kind, RepoKind::Local);
        assert_eq!(settings.cycle_repos.len(), 2);
        assert_eq!(
            add_local_repo(&mut settings, other.to_str().unwrap())
                .unwrap_err()
                .code,
            "duplicate"
        );
        assert_eq!(
            add_local_repo(&mut settings, "Z:/does/not/exist")
                .unwrap_err()
                .code,
            "invalid"
        );
        assert_eq!(
            add_local_repo(&mut settings, " ").unwrap_err().code,
            "invalid"
        );

        // Cycles of both repositories are listed together; ids are global.
        Repo::init_local(&other).unwrap();
        create_cycle(&settings.cycle_repos, &added, "TEAM", "Team").unwrap();
        lab_cycle_in(&settings, "NOMAD");
        let ids: Vec<String> = list_cycles(&settings.cycle_repos)
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, ["NOMAD", "TEAM"]);
        assert_eq!(
            create_cycle(&settings.cycle_repos, &added, "NOMAD", "dup")
                .unwrap_err()
                .code,
            "duplicate"
        );
    }

    fn lab_cycle_in(settings: &Settings, id: &str) {
        create_cycle(&settings.cycle_repos, &settings.cycle_repos[0], id, "n").unwrap();
    }

    #[test]
    fn cloning_registers_the_repository_with_its_remote_and_state() {
        // A clone of a local "remote": no network involved.
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let origin = lab.settings.cycle_repos[0].path.clone();
        let (id, dest) = plan_clone(&lab.settings, lab.dir.path(), &origin);
        assert!(dest.starts_with(lab.dir.path().join(CLONES_DIR)));
        // `git clone` of a repository without commits yields an empty clone, which is fine:
        // the point is the registration and the state.
        let repo = Repo::clone(&origin, &dest).unwrap();
        let mut settings = lab.settings.clone();
        let s = register_clone(&mut settings, &id, repo.root(), &origin);
        assert_eq!(s.kind, RepoKind::Clone);
        assert_eq!(s.remote.as_deref(), Some(origin.as_str()));
        let dto = list_repos(&settings.cycle_repos);
        assert_eq!(dto.len(), 2);
        assert!(dto[1].available);
        assert!(dto[1].state_as_of.is_some());
        assert!(!dto[1].offline);
        // The id is taken, so another clone of the same URL gets another one.
        let (id2, _) = plan_clone(&settings, lab.dir.path(), &origin);
        assert_ne!(id, id2);
    }

    /// Runs git with a fixed identity, for fixtures that commit.
    fn git(dir: &Path, args: &[&str]) {
        let mut full = vec![
            "-C",
            dir.to_str().unwrap(),
            "-c",
            "user.name=Test User",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
        ];
        full.extend_from_slice(args);
        let out = std::process::Command::new("git")
            .args(&full)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// An origin repository with a committed cycle NOMAD, a clone of it and the settings
    /// that register the clone.
    fn origin_and_clone(root: &Path) -> (Repo, Vec<CycleRepoSetting>, Repo) {
        let origin = Repo::init_local(root.join("origin")).unwrap();
        origin.create_cycle(&Cycle::new("NOMAD", "Nomad")).unwrap();
        git(origin.root(), &["add", "-A"]);
        git(origin.root(), &["commit", "--quiet", "-m", "start"]);
        let dest = root.join("clone");
        let clone = Repo::clone(origin.root().to_str().unwrap(), &dest).unwrap();
        let settings = vec![CycleRepoSetting {
            id: "team".into(),
            path: dest.to_string_lossy().into_owned(),
            remote: Some(origin.root().to_string_lossy().into_owned()),
            kind: RepoKind::Clone,
            github_repo: None,
            github_api: None,
        }];
        (origin, settings, clone)
    }

    fn upstream_adds(origin: &Repo, id: &str) {
        origin.create_cycle(&Cycle::new(id, id)).unwrap();
        git(origin.root(), &["add", "-A"]);
        git(origin.root(), &["commit", "--quiet", "-m", id]);
    }

    fn has_cycle(clone: &Repo, id: &str) -> bool {
        clone.cycle_dir(id).unwrap().join("cycle.yaml").is_file()
    }

    #[test]
    fn the_ledger_lets_each_attempt_of_each_job_through_once() {
        let ledger = PullLedger::default();
        assert!(ledger.first_time("m1", 0));
        assert!(!ledger.first_time("m1", 0));
        assert!(!ledger.first_time("m1", 0));
        assert!(ledger.first_time("m2", 0), "another job has its own turn");
        assert!(ledger.first_time("m1", 1), "a retried stage pulls again");
        assert!(!ledger.first_time("m1", 1));
    }

    #[test]
    fn the_briefing_pulls_once_per_attempt_not_every_time_the_queue_looks() {
        let tmp = tempfile::tempdir().unwrap();
        let (origin, repos, clone) = origin_and_clone(tmp.path());
        let ledger = PullLedger::default();

        upstream_adds(&origin, "ROVER");
        pull_for_briefing(&ledger, &repos, "m1", 0, "NOMAD");
        assert!(has_cycle(&clone, "ROVER"), "the first look pulls");

        // The queue looks again every few seconds while the GPU is busy: no more pulls.
        upstream_adds(&origin, "THIRD");
        for _ in 0..5 {
            pull_for_briefing(&ledger, &repos, "m1", 0, "NOMAD");
        }
        assert!(!has_cycle(&clone, "THIRD"));

        // The stage ran and was retried: a new attempt pulls.
        pull_for_briefing(&ledger, &repos, "m1", 1, "NOMAD");
        assert!(has_cycle(&clone, "THIRD"));
    }

    #[test]
    fn the_briefing_leaves_a_repository_with_uncommitted_cycle_edits_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let (origin, repos, clone) = origin_and_clone(tmp.path());
        // The user renamed the cycle in the app (never committed); a teammate renamed it too.
        let mut mine = clone.load_cycle("NOMAD").unwrap();
        mine.name = "My name".into();
        clone.save_cycle(&mine).unwrap();
        let mut theirs = origin.load_cycle("NOMAD").unwrap();
        theirs.name = "Their name".into();
        origin.save_cycle(&theirs).unwrap();
        git(origin.root(), &["add", "-A"]);
        git(origin.root(), &["commit", "--quiet", "-m", "rename"]);

        pull_for_briefing(&PullLedger::default(), &repos, "m1", 0, "NOMAD");

        let text =
            std::fs::read_to_string(clone.cycle_dir("NOMAD").unwrap().join("cycle.yaml")).unwrap();
        assert!(
            !text.contains("<<<<<<<") && !text.contains(">>>>>>>"),
            "{text}"
        );
        assert_eq!(clone.load_cycle("NOMAD").unwrap().name, "My name");
        let stash = std::process::Command::new("git")
            .args(["-C", clone.root().to_str().unwrap(), "stash", "list"])
            .output()
            .unwrap();
        assert!(stash.stdout.is_empty(), "nothing was stashed");
        // The cycle still loads for the briefing.
        assert!(load_cycle_data(&repos, "NOMAD", false).unwrap().is_some());
    }

    #[test]
    fn a_manual_pull_with_uncommitted_edits_says_repo_dirty() {
        let tmp = tempfile::tempdir().unwrap();
        let (origin, repos, clone) = origin_and_clone(tmp.path());
        upstream_adds(&origin, "ROVER");

        let mut mine = clone.load_cycle("NOMAD").unwrap();
        mine.name = "Edited in the app".into();
        clone.save_cycle(&mine).unwrap();
        let e = pull_repo(&repos[0]).unwrap_err();
        assert_eq!(e.code, "repo_dirty");
        assert!(e.message.contains("not committed"), "{}", e.message);
        assert!(!has_cycle(&clone, "ROVER"));
        assert_eq!(clone.load_cycle("NOMAD").unwrap().name, "Edited in the app");

        // Committed, it pulls and keeps the edit.
        git(clone.root(), &["add", "-A"]);
        git(clone.root(), &["commit", "--quiet", "-m", "edit"]);
        let r = pull_repo(&repos[0]).unwrap();
        assert_eq!(r.outcome, "updated");
        assert!(r.warnings.is_empty());
        assert!(has_cycle(&clone, "ROVER"));
        assert_eq!(clone.load_cycle("NOMAD").unwrap().name, "Edited in the app");
    }

    #[test]
    fn a_pull_that_breaks_a_cycle_warns_and_the_warning_reaches_the_result() {
        let tmp = tempfile::tempdir().unwrap();
        let (origin, repos, clone) = origin_and_clone(tmp.path());
        std::fs::write(
            origin.cycle_dir("NOMAD").unwrap().join("cycle.yaml"),
            "id: [\n",
        )
        .unwrap();
        git(origin.root(), &["add", "-A"]);
        git(origin.root(), &["commit", "--quiet", "-m", "break"]);
        let r = pull_repo(&repos[0]).unwrap();
        assert_eq!(r.outcome, "updated");
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
        assert!(r.warnings[0].contains("NOMAD"));
        assert!(clone.load_cycle("NOMAD").is_err());
        let json = serde_json::to_value(&r).unwrap();
        assert!(json["warnings"].is_array());
    }

    #[test]
    fn a_participant_whose_id_changed_upstream_does_not_stop_the_cycle_from_saving() {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        let mut dto = get_cycle(lab.repos(), "NOMAD").unwrap().cycle;
        dto.participants = vec![person("alex", "Alex"), person("sam", "Sam")];
        assert!(save_cycle(lab.repos(), &lab.store, dto).unwrap().saved);
        let old = store::cycle_participant_key("NOMAD", "alex");
        lab.store
            .put_sample(
                &old,
                &store::SampleRow {
                    source_key: "m:v".into(),
                    embedding: vec![1.0],
                    speech_ms: 9_000,
                },
                1,
            )
            .unwrap();

        // The id changes (an upstream edit); the person and the name stay.
        let mut dto = get_cycle(lab.repos(), "NOMAD").unwrap().cycle;
        dto.participants[0].id = "alexander".into();
        let r = save_cycle(lab.repos(), &lab.store, dto).unwrap();
        assert!(r.saved, "{:?}", r.issues);
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
        assert!(r.warnings[0].contains("alexander"), "{:?}", r.warnings);
        let rows = lab.store.participants_for_cycle("NOMAD").unwrap();
        let alex = rows.iter().find(|p| p.name == "Alex").unwrap();
        assert_eq!(alex.id, store::cycle_participant_key("NOMAD", "alexander"));
        assert_eq!(alex.sample_count, 1, "the voice profile came along");
        assert!(rows.iter().all(|p| p.id != old));

        // Later syncs (a pull, a recording) work and say nothing more.
        assert!(sync_repo(&lab.settings.cycle_repos[0], &lab.store).is_empty());
        sync_cycle(lab.repos(), &lab.store, "NOMAD").unwrap();
    }

    #[test]
    fn a_missing_repository_folder_is_reported_not_fatal() {
        let settings = vec![CycleRepoSetting {
            id: "gone".into(),
            path: "Z:/does/not/exist".into(),
            remote: None,
            kind: RepoKind::Local,
            github_repo: None,
            github_api: None,
        }];
        assert!(list_cycles(&settings).is_empty());
        let r = list_repos(&settings);
        assert!(!r[0].available);
        assert!(locate(&settings, "NOMAD").unwrap().is_none());
    }
}
