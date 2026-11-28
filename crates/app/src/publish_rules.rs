//! The global "When to publish" rules and how a project relates to them (ADR 0019).
//!
//! The rule of a target is global by default: `Settings::publish_rules` holds one rule per
//! target and a project follows it until its `cycle.yaml` has its own `targets.<x>.publish`.
//! The resolver is [`publishers::rules::resolve_rule`]; this module only exposes its result to
//! the screens ([`effective_rules`]) and changes the global rules. Nothing here writes a
//! `cycle.yaml`: a project that already has an explicit rule keeps it.

use cycle_store::{Cycle, PublishMode};
use publishers::rules::{resolve_rule, GlobalPublishRules, PublishTarget, RuleSource};
use serde::Serialize;

use crate::cycles::{self};
use crate::ipc::{IpcError, IpcResult};
use crate::publishing::mode_name;
use crate::settings::Settings;

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
    let cycle = located
        .entry
        .cycle
        .clone()
        .filter(|_| located.entry.is_valid())
        .ok_or_else(|| {
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

    #[test]
    fn an_invalid_cycle_has_no_effective_rules() {
        let tmp = test_tmp::TestDir::new("publish-rules");
        let repo = cycle_store::Repo::init_local(tmp.path().join("r")).unwrap();
        repo.create_cycle(&cycle_with(None, None)).unwrap();
        let mut settings = Settings::default();
        settings
            .cycle_repos
            .push(crate::settings::CycleRepoSetting {
                id: "local".into(),
                path: repo.root().to_string_lossy().into_owned(),
                remote: None,
                kind: crate::settings::RepoKind::Local,
                github_repo: None,
                github_api: None,
            });
        assert!(effective_rules_of(&settings, "NOMAD").is_ok());
        let yaml = repo.cycle_dir("NOMAD").unwrap().join("cycle.yaml");
        std::fs::write(
            &yaml,
            "id: NOMAD
name: Nomad
targets:
  github: { path: \"../x\" }
",
        )
        .unwrap();
        let e = effective_rules_of(&settings, "NOMAD").unwrap_err();
        assert_eq!(e.code, "cycle_invalid");
    }

    #[test]
    fn a_settings_file_with_the_removed_migration_flag_still_loads() {
        let s: Settings = serde_json::from_str(
            r#"{"publishRules":{"discord":"off"},"publishRulesMigrated":true}"#,
        )
        .unwrap();
        assert_eq!(s.publish_rules.discord, PublishMode::Off);
    }
}
