//! The global "When to publish" rules and how a project relates to them (ADR 0019, WP-08e).
//!
//! The rule of a target is global by default: `Settings::publish_rules` holds one rule per
//! target and a project follows it until its `cycle.yaml` has its own `targets.<x>.publish`.
//! The resolver is [`publishers::rules::resolve_rule`]; this module only exposes its result to
//! the screens ([`effective_rules`]), changes the global rules and migrates the projects that
//! predate the global rules and were never touched.

use cycle_store::{Cycle, PublishMode};
use publishers::rules::{resolve_rule, GlobalPublishRules, PublishTarget, RuleSource};
use serde::Serialize;

use crate::cycles::{self};
use crate::ipc::{IpcError, IpcResult};
use crate::publishing::mode_name;
use crate::settings::{CycleRepoSetting, Settings};

/// The rule in force for one target of a project, with where it comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveRuleDto {
    /// `github`, `discord` or `notion`.
    pub target: &'static str,
    /// The project's own rule; `None` when it follows the global one.
    pub project: Option<&'static str>,
    /// The global rule of the preferences for this target.
    pub global: &'static str,
    /// The rule in force.
    pub mode: &'static str,
    /// `project` or `global`.
    pub source: &'static str,
}

fn source_name(source: RuleSource) -> &'static str {
    match source {
        RuleSource::Project => "project",
        RuleSource::Global => "global",
    }
}

/// The effective rule of every target `cycle` has (GitHub always: a cycle without a GitHub
/// target still has the report target with the global rule).
pub fn effective_rules(cycle: &Cycle, global: &GlobalPublishRules) -> Vec<EffectiveRuleDto> {
    let own = |target: PublishTarget| match target {
        PublishTarget::Github => Some(cycle.targets.github.as_ref().and_then(|t| t.publish)),
        PublishTarget::Discord => cycle.targets.discord.as_ref().map(|t| t.publish),
        PublishTarget::Notion => cycle.targets.notion.as_ref().map(|t| t.publish),
    };
    PublishTarget::ALL
        .into_iter()
        .filter_map(|target| {
            let project = own(target)?;
            let effective = resolve_rule(project, global.get(target));
            Some(EffectiveRuleDto {
                target: target.as_str(),
                project: project.map(mode_name),
                global: mode_name(global.get(target)),
                mode: mode_name(effective.mode),
                source: source_name(effective.source),
            })
        })
        .collect()
}

/// The effective rules of a registered cycle under the global rules in `settings`.
pub fn effective_rules_of(settings: &Settings, cycle_id: &str) -> IpcResult<Vec<EffectiveRuleDto>> {
    let located = cycles::locate(&settings.cycle_repos, cycle_id)?.ok_or_else(|| {
        IpcError::new(
            "not_found",
            format!("the cycle {cycle_id} is not in any registered repository"),
        )
    })?;
    let cycle = located.entry.cycle.ok_or_else(|| {
        IpcError::new(
            "cycle_invalid",
            "the cycle cannot be used until it is fixed",
        )
    })?;
    Ok(effective_rules(&cycle, &settings.publish_rules))
}

/// Sets the global rule of one target (`github`, `discord` or `notion`).
pub fn set_global_rule(
    settings: &mut Settings,
    target: &str,
    mode: PublishMode,
) -> IpcResult<GlobalPublishRules> {
    let target = PublishTarget::parse(target).ok_or_else(|| {
        IpcError::new(
            "invalid",
            format!("{target} is not a target with a publishing rule (github, discord or notion)"),
        )
    })?;
    settings.publish_rules.set(target, mode);
    Ok(settings.publish_rules)
}

/// Moves the projects that predate the global rules to "use the global setting", but only the
/// untouched ones: a target whose stored rule is the old default `on-approval` and whose
/// `cycle.yaml` never held another rule in the Git history of its repository
/// ([`cycle_store::Repo::publish_rule_untouched`]). When that cannot be told the explicit value
/// stays. A repository is looked at once (its id is remembered in
/// `Settings::publish_rules_migrated`), so a rule a user picks later is never rewritten.
///
/// Returns whether `settings` changed (the caller saves them) and how many projects were
/// migrated.
pub fn migrate_untouched(settings: &mut Settings) -> (bool, usize) {
    let mut changed = false;
    let mut migrated = 0;
    let repos: Vec<CycleRepoSetting> = settings.cycle_repos.clone();
    for setting in repos {
        if settings.publish_rules_migrated.contains(&setting.id) {
            continue;
        }
        let Ok(repo) = cycle_store::Repo::open(&setting.path) else {
            // Not available now: look again at the next start.
            continue;
        };
        let Ok(entries) = repo.list_cycles() else {
            continue;
        };
        for entry in entries.into_iter().filter(|e| e.is_valid()) {
            let Some(mut cycle) = entry.cycle else {
                continue;
            };
            if !has_default_rule(&cycle) || !repo.publish_rule_untouched(&entry.dir_name) {
                continue;
            }
            if let Some(t) = cycle.targets.github.as_mut() {
                t.publish = t.publish.filter(|m| *m != PublishMode::OnApproval);
            }
            if let Some(t) = cycle.targets.discord.as_mut() {
                t.publish = t.publish.filter(|m| *m != PublishMode::OnApproval);
            }
            if let Some(t) = cycle.targets.notion.as_mut() {
                t.publish = t.publish.filter(|m| *m != PublishMode::OnApproval);
            }
            match repo.save_cycle(&cycle) {
                Ok(()) => migrated += 1,
                Err(e) => tracing::warn!(
                    cycle = %entry.dir_name,
                    error = %e,
                    "could not move the cycle to the global publish rule"
                ),
            }
        }
        settings.publish_rules_migrated.push(setting.id);
        changed = true;
    }
    (changed, migrated)
}

/// Whether any target of `cycle` still stores the old default `on-approval`.
fn has_default_rule(cycle: &Cycle) -> bool {
    let default = Some(PublishMode::OnApproval);
    cycle
        .targets
        .github
        .as_ref()
        .is_some_and(|t| t.publish == default)
        || cycle
            .targets
            .discord
            .as_ref()
            .is_some_and(|t| t.publish == default)
        || cycle
            .targets
            .notion
            .as_ref()
            .is_some_and(|t| t.publish == default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cycle_store::{DiscordTarget, GithubTarget, NotionProperties, NotionTarget};

    fn cycle_with(github: Option<PublishMode>, discord: Option<Option<PublishMode>>) -> Cycle {
        let mut c = Cycle::new("NOMAD", "Nomad");
        c.targets.github = Some(GithubTarget {
            publish: github,
            ..GithubTarget::default()
        });
        c.targets.discord = discord.map(|publish| DiscordTarget {
            webhook: Some("cred:discord/nomad".into()),
            bot: None,
            publish,
            template: None,
        });
        c.targets.notion = Some(NotionTarget {
            token: "cred:notion/nomad".into(),
            database: "0123456789abcdef0123456789abcdef".into(),
            data_source: None,
            publish: None,
            properties: NotionProperties::default(),
        });
        c
    }

    #[test]
    fn effective_rules_name_the_rule_and_its_source() {
        let global = GlobalPublishRules {
            github: PublishMode::Auto,
            discord: PublishMode::Off,
            ..GlobalPublishRules::default()
        };
        let cycle = cycle_with(None, Some(Some(PublishMode::OnApproval)));
        let rules = effective_rules(&cycle, &global);
        let by = |t: &str| rules.iter().find(|r| r.target == t).unwrap().clone();
        let github = by("github");
        assert_eq!(
            (github.project, github.global, github.mode, github.source),
            (None, "auto", "auto", "global")
        );
        let discord = by("discord");
        assert_eq!(
            (
                discord.project,
                discord.global,
                discord.mode,
                discord.source
            ),
            (Some("on-approval"), "off", "on-approval", "project")
        );
        assert_eq!(by("notion").source, "global");
        // A cycle without a Discord target has no Discord row; GitHub is always there.
        let plain = effective_rules(&Cycle::new("X", "x"), &global);
        assert_eq!(plain.len(), 1);
        assert_eq!(plain[0].target, "github");
        assert_eq!(plain[0].mode, "auto");
    }

    #[test]
    fn a_global_rule_is_set_per_target_and_unknown_targets_are_refused() {
        let mut settings = Settings::default();
        let g = set_global_rule(&mut settings, "discord", PublishMode::Off).unwrap();
        assert_eq!(g.discord, PublishMode::Off);
        assert_eq!(g.github, PublishMode::OnApproval);
        assert_eq!(settings.publish_rules, g);
        let e = set_global_rule(&mut settings, "slack", PublishMode::Auto).unwrap_err();
        assert_eq!(e.code, "invalid");
    }

    #[test]
    fn the_global_rules_default_to_on_approval_and_old_settings_files_get_them() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.publish_rules, GlobalPublishRules::default());
        let s: Settings = serde_json::from_str(r#"{"publishRules":{"github":"auto"}}"#).unwrap();
        assert_eq!(s.publish_rules.github, PublishMode::Auto);
        assert_eq!(s.publish_rules.discord, PublishMode::OnApproval);
    }

    fn git(dir: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A repository with `NOMAD` (github + discord on-approval, committed) and `ROVER`
    /// (github on-approval, once `auto`, committed) and `FRESH` (uncommitted).
    fn repo_fixture(tmp: &std::path::Path) -> (Settings, cycle_store::Repo) {
        use cycle_store::Repo;
        let repo = Repo::init_local(tmp.join("r")).unwrap();
        let mut nomad = cycle_with(
            Some(PublishMode::OnApproval),
            Some(Some(PublishMode::OnApproval)),
        );
        nomad.targets.notion = None;
        repo.create_cycle(&nomad).unwrap();
        let mut rover = cycle_with(Some(PublishMode::Auto), None);
        rover.id = "ROVER".into();
        rover.targets.notion = None;
        repo.create_cycle(&rover).unwrap();
        git(repo.root(), &["add", "-A"]);
        git(repo.root(), &["commit", "--quiet", "-m", "cycles"]);
        rover.targets.github.as_mut().unwrap().publish = Some(PublishMode::OnApproval);
        repo.save_cycle(&rover).unwrap();
        git(
            repo.root(),
            &["commit", "--quiet", "-am", "rover back to on-approval"],
        );
        let mut fresh = cycle_with(Some(PublishMode::OnApproval), None);
        fresh.id = "FRESH".into();
        fresh.targets.notion = None;
        repo.create_cycle(&fresh).unwrap();
        let mut settings = Settings::default();
        settings.cycle_repos.push(CycleRepoSetting {
            id: "local".into(),
            path: repo.root().to_string_lossy().into_owned(),
            remote: None,
            kind: crate::settings::RepoKind::Local,
            github_repo: None,
            github_api: None,
        });
        (settings, repo)
    }

    #[test]
    fn only_untouched_projects_move_to_the_global_rule_and_only_once() {
        let tmp = test_tmp::TestDir::new("publish-rules");
        let (mut settings, repo) = repo_fixture(tmp.path());
        let (changed, migrated) = migrate_untouched(&mut settings);
        assert!(changed);
        assert_eq!(migrated, 1, "only NOMAD is committed and never changed");
        assert_eq!(settings.publish_rules_migrated, ["local"]);
        // NOMAD: both targets inherit now.
        let nomad = repo.load_cycle("NOMAD").unwrap();
        assert_eq!(nomad.targets.github.unwrap().publish, None);
        assert_eq!(nomad.targets.discord.unwrap().publish, None);
        // ROVER was set to auto once, FRESH has no history: their explicit values stay.
        assert_eq!(
            repo.load_cycle("ROVER")
                .unwrap()
                .targets
                .github
                .unwrap()
                .publish,
            Some(PublishMode::OnApproval)
        );
        assert_eq!(
            repo.load_cycle("FRESH")
                .unwrap()
                .targets
                .github
                .unwrap()
                .publish,
            Some(PublishMode::OnApproval)
        );
        // Idempotent: a second run touches nothing and leaves the settings as they are.
        let yaml = |id: &str| {
            std::fs::read_to_string(repo.cycle_dir(id).unwrap().join("cycle.yaml")).unwrap()
        };
        let (a, b, c) = (yaml("NOMAD"), yaml("ROVER"), yaml("FRESH"));
        let before = settings.clone();
        assert_eq!(migrate_untouched(&mut settings), (false, 0));
        assert_eq!(settings, before);
        assert_eq!((yaml("NOMAD"), yaml("ROVER"), yaml("FRESH")), (a, b, c));
    }

    #[test]
    fn a_rule_picked_after_the_migration_is_never_rewritten() {
        let tmp = test_tmp::TestDir::new("publish-rules");
        let (mut settings, repo) = repo_fixture(tmp.path());
        migrate_untouched(&mut settings);
        // The user explicitly picks on-approval for NOMAD and commits it.
        let mut nomad = repo.load_cycle("NOMAD").unwrap();
        nomad.targets.github.as_mut().unwrap().publish = Some(PublishMode::OnApproval);
        repo.save_cycle(&nomad).unwrap();
        git(
            repo.root(),
            &["commit", "--quiet", "-am", "explicit on-approval"],
        );
        assert_eq!(migrate_untouched(&mut settings), (false, 0));
        assert_eq!(
            repo.load_cycle("NOMAD")
                .unwrap()
                .targets
                .github
                .unwrap()
                .publish,
            Some(PublishMode::OnApproval)
        );
    }

    #[test]
    fn a_repository_that_cannot_be_opened_is_looked_at_again_later() {
        let mut settings = Settings::default();
        settings.cycle_repos.push(CycleRepoSetting {
            id: "gone".into(),
            path: "Z:/does/not/exist".into(),
            remote: None,
            kind: crate::settings::RepoKind::Local,
            github_repo: None,
            github_api: None,
        });
        assert_eq!(migrate_untouched(&mut settings), (false, 0));
        assert!(settings.publish_rules_migrated.is_empty());
    }
}
