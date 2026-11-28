//! Which targets a meeting goes to, and when: the per-target rules of `cycle.yaml`
//! (`targets.<name>.publish`: `auto`, `on-approval` or `off`) and their defaults.
//!
//! The defaults are constants in `cycle-store` (`DEFAULT_GITHUB_PUBLISH`,
//! `DEFAULT_DISCORD_PUBLISH`, `DEFAULT_NOTION_PUBLISH`), read by the parser of `cycle.yaml` and by these rules alike:
//! changing one is a change of a constant. They are set by ADR 0019, chosen because
//! what is published cannot be taken back: a pushed transcript stays in the history of the
//! repository, and an issue assignment or a Discord ping has already notified somebody.
//!
//! * `auto`: the operations are queued as soon as the draft of a meeting is complete (no newer
//!   draft waits for a decision). Status changes are never part of it, with any mode: a model's
//!   guess that a task is done is not enough to close an issue of other people, so only a change
//!   the user approved is written back, and only by an explicit publication.
//! * `on-approval`: queued when the user presses Publish for that target.
//! * `off`: never, not even when the user asks.
//!
//! Where the rule of a target comes from (ADR 0019): the global rule of the preferences
//! ([`GlobalPublishRules`]) is the default of every project; a project overrides it only when
//! its `cycle.yaml` has an explicit `publish` key for that target. [`resolve_rule`] is the one
//! place that decides it, used by the plan and shown to the settings screens.

use cycle_store::{Cycle, PublishMode};
use serde::{Deserialize, Serialize};

pub use cycle_store::{DEFAULT_DISCORD_PUBLISH, DEFAULT_GITHUB_PUBLISH, DEFAULT_NOTION_PUBLISH};

/// A place a meeting can be published to. GitHub covers the commit with the report and the
/// transcript, the issues and the status changes; Notion gets the task items only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PublishTarget {
    Github,
    Discord,
    Notion,
}

impl PublishTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Discord => "discord",
            Self::Notion => "notion",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "github" => Some(Self::Github),
            "discord" => Some(Self::Discord),
            "notion" => Some(Self::Notion),
            _ => None,
        }
    }
}

impl PublishTarget {
    pub const ALL: [PublishTarget; 3] = [Self::Github, Self::Discord, Self::Notion];
}

/// The global rules of the preferences: what a target does in every project that does not
/// set its own `publish`. Changed only through the preferences screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GlobalPublishRules {
    pub github: PublishMode,
    pub discord: PublishMode,
    pub notion: PublishMode,
}

impl Default for GlobalPublishRules {
    fn default() -> Self {
        Self {
            github: DEFAULT_GITHUB_PUBLISH,
            discord: DEFAULT_DISCORD_PUBLISH,
            notion: DEFAULT_NOTION_PUBLISH,
        }
    }
}

impl GlobalPublishRules {
    pub fn get(&self, target: PublishTarget) -> PublishMode {
        match target {
            PublishTarget::Github => self.github,
            PublishTarget::Discord => self.discord,
            PublishTarget::Notion => self.notion,
        }
    }

    pub fn set(&mut self, target: PublishTarget, mode: PublishMode) {
        match target {
            PublishTarget::Github => self.github = mode,
            PublishTarget::Discord => self.discord = mode,
            PublishTarget::Notion => self.notion = mode,
        }
    }
}

/// Where an effective rule comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleSource {
    /// The project has its own `publish` for the target.
    Project,
    /// The project has none and uses the global rule.
    Global,
}

/// The rule in force for one target of a project and where it comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EffectiveRule {
    pub mode: PublishMode,
    pub source: RuleSource,
}

/// The one resolver: the project's own rule wins, otherwise the global rule applies.
pub fn resolve_rule(project: Option<PublishMode>, global: PublishMode) -> EffectiveRule {
    match project {
        Some(mode) => EffectiveRule {
            mode,
            source: RuleSource::Project,
        },
        None => EffectiveRule {
            mode: global,
            source: RuleSource::Global,
        },
    }
}

/// What starts a publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// The draft of a meeting was completed.
    Auto,
    /// The user pressed Publish.
    Explicit,
}

/// The rules of one cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetRules {
    /// The report, the transcript and, with `issues`, the issues.
    pub github: PublishMode,
    /// `targets.github.issues`.
    pub issues: bool,
    /// `None`: the cycle has no Discord target.
    pub discord: Option<PublishMode>,
    /// `None`: the cycle has no Notion target. Status changes never go to Notion.
    pub notion: Option<PublishMode>,
    /// Where each of the rules above comes from.
    pub github_source: RuleSource,
    pub discord_source: RuleSource,
    pub notion_source: RuleSource,
    /// Whether the GitHub target has a configured remote or linked repository.
    pub github_configured: bool,
    /// Whether the Discord target has a configured webhook reference or bot channel.
    pub discord_configured: bool,
    /// Whether the Notion target has a configured database/page id.
    pub notion_configured: bool,
}

/// A bot channel is configured by its channel; a webhook only when its secret is stored
/// (`webhook_secret_set`), the same rule the settings screen applies.
fn discord_is_configured(cycle: &Cycle, webhook_secret_set: bool) -> bool {
    cycle.targets.discord.as_ref().is_some_and(|t| {
        t.bot.as_ref().map_or_else(
            || webhook_secret_set && t.webhook.as_deref().is_some_and(|w| !w.trim().is_empty()),
            |b| !b.channel.trim().is_empty(),
        )
    })
}

fn notion_is_configured(cycle: &Cycle) -> bool {
    cycle
        .targets
        .notion
        .as_ref()
        .is_some_and(|t| !t.database.trim().is_empty())
}

impl TargetRules {
    /// The rules of `cycle` under the default global rules. Assumes GitHub is configured; use
    /// [`Self::for_repo`] when the repository's state and the real global rules are known.
    pub fn of(cycle: &Cycle) -> Self {
        Self::for_repo(cycle, &GlobalPublishRules::default(), true, true)
    }

    /// The rules of `cycle` under `global`, assuming GitHub is configured.
    pub fn of_with(cycle: &Cycle, global: &GlobalPublishRules) -> Self {
        Self::for_repo(cycle, global, true, true)
    }

    /// The rules of `cycle`: its own `publish` where it has one, otherwise `global`
    /// ([`resolve_rule`]), together with whether its repository is connected to a remote or
    /// GitHub repository, and whether the secret of the Discord webhook reference is stored.
    /// An unconfigured target is never allowed by [`Self::allows`].
    pub fn for_repo(
        cycle: &Cycle,
        global: &GlobalPublishRules,
        github_configured: bool,
        discord_webhook_set: bool,
    ) -> Self {
        // A cycle without `targets.github` still has the report target, with the global rule.
        let github = resolve_rule(
            cycle.targets.github.as_ref().and_then(|t| t.publish),
            global.github,
        );
        // A cycle without `targets.discord` / `targets.notion` has no such target at all.
        let discord = cycle
            .targets
            .discord
            .as_ref()
            .map(|t| resolve_rule(t.publish, global.discord));
        let notion = cycle
            .targets
            .notion
            .as_ref()
            .map(|t| resolve_rule(t.publish, global.notion));
        Self {
            github: github.mode,
            issues: cycle.targets.github.as_ref().is_none_or(|t| t.issues),
            discord: discord.map(|r| r.mode),
            notion: notion.map(|r| r.mode),
            github_source: github.source,
            discord_source: discord.map_or(RuleSource::Global, |r| r.source),
            notion_source: notion.map_or(RuleSource::Global, |r| r.source),
            github_configured,
            discord_configured: discord_is_configured(cycle, discord_webhook_set),
            notion_configured: notion_is_configured(cycle),
        }
    }

    /// Where the rule of `target` comes from.
    pub fn source(&self, target: PublishTarget) -> RuleSource {
        match target {
            PublishTarget::Github => self.github_source,
            PublishTarget::Discord => self.discord_source,
            PublishTarget::Notion => self.notion_source,
        }
    }

    /// Whether `target` has the configuration required to publish (GitHub: a remote or linked
    /// repo; Discord: a webhook or bot channel; Notion: a database/page id).
    pub fn is_configured(&self, target: PublishTarget) -> bool {
        match target {
            PublishTarget::Github => self.github_configured,
            PublishTarget::Discord => self.discord_configured,
            PublishTarget::Notion => self.notion_configured,
        }
    }

    /// The mode of a target; `None` when the cycle has no such target.
    pub fn mode(&self, target: PublishTarget) -> Option<PublishMode> {
        match target {
            PublishTarget::Github => Some(self.github),
            PublishTarget::Discord => self.discord,
            PublishTarget::Notion => self.notion,
        }
    }

    /// Whether `trigger` may publish to `target`: `auto` only for targets set to auto,
    /// an explicit request for everything but `off`. Always `false` for unconfigured targets.
    pub fn allows(&self, target: PublishTarget, trigger: Trigger) -> bool {
        if !self.is_configured(target) {
            return false;
        }
        matches!(
            (self.mode(target), trigger),
            (Some(PublishMode::Auto), _) | (Some(PublishMode::OnApproval), Trigger::Explicit)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cycle_store::{DiscordTarget, GithubTarget};

    fn with(github: Option<PublishMode>, discord: Option<PublishMode>) -> Cycle {
        let mut c = Cycle::new("NOMAD", "Nomad");
        c.targets.github = github.map(|publish| GithubTarget {
            publish: Some(publish),
            ..GithubTarget::default()
        });
        c.targets.discord = discord.map(|publish| DiscordTarget {
            webhook: Some("cred:discord/nomad".into()),
            bot: None,
            publish: Some(publish),
            template: None,
        });
        c
    }

    #[test]
    fn the_defaults_are_one_constant_each_and_nothing_goes_out_on_its_own() {
        assert_eq!(DEFAULT_GITHUB_PUBLISH, PublishMode::OnApproval);
        assert_eq!(DEFAULT_DISCORD_PUBLISH, PublishMode::OnApproval);
        let rules = TargetRules::of(&Cycle::new("NOMAD", "Nomad"));
        assert_eq!(rules.github, DEFAULT_GITHUB_PUBLISH);
        assert!(rules.issues);
        assert_eq!(rules.discord, None, "no Discord target without a webhook");
        assert_eq!(rules.notion, None, "no Notion target without a token");
        assert_eq!(DEFAULT_NOTION_PUBLISH, PublishMode::OnApproval);
        assert!(!rules.allows(PublishTarget::Notion, Trigger::Explicit));
        assert!(!rules.allows(PublishTarget::Github, Trigger::Auto));
        assert!(rules.allows(PublishTarget::Github, Trigger::Explicit));
        assert!(!rules.allows(PublishTarget::Discord, Trigger::Explicit));
    }

    #[test]
    fn auto_on_approval_and_off_decide_who_may_publish() {
        for (mode, auto, explicit) in [
            (PublishMode::Auto, true, true),
            (PublishMode::OnApproval, false, true),
            (PublishMode::Off, false, false),
        ] {
            let rules = TargetRules::of(&with(Some(mode), Some(mode)));
            for target in [PublishTarget::Github, PublishTarget::Discord] {
                assert_eq!(rules.allows(target, Trigger::Auto), auto, "{mode:?}");
                assert_eq!(
                    rules.allows(target, Trigger::Explicit),
                    explicit,
                    "{mode:?}"
                );
            }
        }
    }

    #[test]
    fn a_target_that_does_not_say_how_to_publish_gets_the_default_constant() {
        let cycle = Cycle::from_yaml_str(
            "id: NOMAD
name: Nomad
targets:
  github: { path: meetings }
  discord: { webhook: \"cred:discord/nomad\" }
",
        )
        .unwrap();
        let rules = TargetRules::of(&cycle);
        assert_eq!(rules.github, DEFAULT_GITHUB_PUBLISH);
        assert_eq!(rules.discord, Some(DEFAULT_DISCORD_PUBLISH));
    }

    #[test]
    fn the_issues_flag_comes_from_the_github_target() {
        let mut cycle = with(Some(PublishMode::Auto), None);
        assert!(TargetRules::of(&cycle).issues);
        cycle.targets.github.as_mut().unwrap().issues = false;
        assert!(!TargetRules::of(&cycle).issues);
    }

    #[test]
    fn targets_have_stable_names() {
        for t in [
            PublishTarget::Github,
            PublishTarget::Discord,
            PublishTarget::Notion,
        ] {
            assert_eq!(PublishTarget::parse(t.as_str()), Some(t));
        }
        assert_eq!(PublishTarget::Notion.as_str(), "notion");
        assert_eq!(PublishTarget::parse("slack"), None);
    }

    #[test]
    fn the_notion_rule_comes_from_its_target_and_defaults_to_on_approval() {
        let cycle = Cycle::from_yaml_str(
            "id: NOMAD
name: Nomad
targets:
  notion: { token: \"cred:notion/nomad\", database: \"0123456789abcdef0123456789abcdef\" }
",
        )
        .unwrap();
        let rules = TargetRules::of(&cycle);
        assert_eq!(rules.notion, Some(DEFAULT_NOTION_PUBLISH));
        assert!(!rules.allows(PublishTarget::Notion, Trigger::Auto));
        assert!(rules.allows(PublishTarget::Notion, Trigger::Explicit));
        for (mode, auto, explicit) in [
            (PublishMode::Auto, true, true),
            (PublishMode::OnApproval, false, true),
            (PublishMode::Off, false, false),
        ] {
            let mut c = cycle.clone();
            c.targets.notion.as_mut().unwrap().publish = Some(mode);
            let rules = TargetRules::of(&c);
            assert_eq!(rules.allows(PublishTarget::Notion, Trigger::Auto), auto);
            assert_eq!(
                rules.allows(PublishTarget::Notion, Trigger::Explicit),
                explicit
            );
        }
    }

    #[test]
    fn a_webhook_counts_as_configured_only_when_its_secret_is_stored() {
        let cycle = with(Some(PublishMode::Auto), Some(PublishMode::Auto));
        assert!(
            TargetRules::for_repo(&cycle, &GlobalPublishRules::default(), true, true)
                .is_configured(PublishTarget::Discord)
        );
        let rules = TargetRules::for_repo(&cycle, &GlobalPublishRules::default(), true, false);
        assert!(!rules.is_configured(PublishTarget::Discord));
        assert!(!rules.allows(PublishTarget::Discord, Trigger::Auto));
        assert!(!rules.allows(PublishTarget::Discord, Trigger::Explicit));
    }

    #[test]
    fn unconfigured_targets_are_never_allowed() {
        let mut cycle = with(Some(PublishMode::Auto), Some(PublishMode::Auto));
        cycle.targets.discord = Some(DiscordTarget {
            webhook: Some("  ".into()),
            bot: None,
            publish: Some(PublishMode::Auto),
            template: None,
        });
        cycle.targets.notion = Some(cycle_store::NotionTarget {
            token: "cred:notion/nomad".into(),
            database: "".into(),
            data_source: None,
            publish: Some(PublishMode::Auto),
            properties: cycle_store::NotionProperties::default(),
        });

        let rules = TargetRules::for_repo(&cycle, &GlobalPublishRules::default(), false, true);
        for target in [
            PublishTarget::Github,
            PublishTarget::Discord,
            PublishTarget::Notion,
        ] {
            assert!(!rules.is_configured(target), "{target:?}");
            assert!(!rules.allows(target, Trigger::Auto), "{target:?}");
            assert!(!rules.allows(target, Trigger::Explicit), "{target:?}");
        }

        // Bot mode with empty channel is also unconfigured; non-empty channel is configured.
        cycle.targets.discord = Some(DiscordTarget {
            webhook: None,
            bot: Some(cycle_store::DiscordBot {
                token: "cred:discord-bot/nomad".into(),
                guild: "123".into(),
                channel: "".into(),
                thread: true,
                edit_on_correction: true,
            }),
            publish: Some(PublishMode::Auto),
            template: None,
        });
        assert!(
            !TargetRules::for_repo(&cycle, &GlobalPublishRules::default(), false, true)
                .allows(PublishTarget::Discord, Trigger::Auto)
        );

        cycle
            .targets
            .discord
            .as_mut()
            .unwrap()
            .bot
            .as_mut()
            .unwrap()
            .channel = "456".into();
        assert!(
            TargetRules::for_repo(&cycle, &GlobalPublishRules::default(), false, true)
                .allows(PublishTarget::Discord, Trigger::Auto)
        );
    }

    fn global(
        github: PublishMode,
        discord: PublishMode,
        notion: PublishMode,
    ) -> GlobalPublishRules {
        GlobalPublishRules {
            github,
            discord,
            notion,
        }
    }

    #[test]
    fn a_project_without_its_own_rule_follows_the_global_one() {
        let cycle = Cycle::from_yaml_str(
            "id: NOMAD
name: Nomad
targets:
  github: { path: meetings }
  discord: { webhook: \"cred:discord/nomad\" }
  notion: { token: \"cred:notion/nomad\", database: \"0123456789abcdef0123456789abcdef\" }
",
        )
        .unwrap();
        let g = global(PublishMode::Auto, PublishMode::Off, PublishMode::OnApproval);
        let rules = TargetRules::of_with(&cycle, &g);
        assert_eq!(rules.github, PublishMode::Auto);
        assert_eq!(rules.discord, Some(PublishMode::Off));
        assert_eq!(rules.notion, Some(PublishMode::OnApproval));
        for t in PublishTarget::ALL {
            assert_eq!(rules.source(t), RuleSource::Global, "{t:?}");
        }
        assert!(rules.allows(PublishTarget::Github, Trigger::Auto));
        assert!(!rules.allows(PublishTarget::Discord, Trigger::Explicit));

        // Changing the global rule changes the project's effective rule, nothing is stored.
        let g2 = global(PublishMode::Off, PublishMode::Auto, PublishMode::OnApproval);
        let rules = TargetRules::of_with(&cycle, &g2);
        assert!(!rules.allows(PublishTarget::Github, Trigger::Explicit));
        assert!(rules.allows(PublishTarget::Discord, Trigger::Auto));
    }

    #[test]
    fn an_explicit_project_rule_overrides_the_global_one() {
        let mut cycle = with(Some(PublishMode::Off), Some(PublishMode::OnApproval));
        let g = global(PublishMode::Auto, PublishMode::Auto, PublishMode::Auto);
        let rules = TargetRules::of_with(&cycle, &g);
        assert_eq!(rules.github, PublishMode::Off);
        assert_eq!(rules.discord, Some(PublishMode::OnApproval));
        assert_eq!(rules.source(PublishTarget::Github), RuleSource::Project);
        assert_eq!(rules.source(PublishTarget::Discord), RuleSource::Project);
        assert!(!rules.allows(PublishTarget::Github, Trigger::Explicit));
        assert!(!rules.allows(PublishTarget::Discord, Trigger::Auto));
        // Back to inherit: the global rule is in force again.
        cycle.targets.github.as_mut().unwrap().publish = None;
        let rules = TargetRules::of_with(&cycle, &g);
        assert_eq!(rules.github, PublishMode::Auto);
        assert_eq!(rules.source(PublishTarget::Github), RuleSource::Global);
    }

    #[test]
    fn the_resolver_names_the_source() {
        let e = resolve_rule(None, PublishMode::Auto);
        assert_eq!((e.mode, e.source), (PublishMode::Auto, RuleSource::Global));
        let e = resolve_rule(Some(PublishMode::Off), PublishMode::Auto);
        assert_eq!((e.mode, e.source), (PublishMode::Off, RuleSource::Project));
        let mut g = GlobalPublishRules::default();
        assert_eq!(g.get(PublishTarget::Notion), DEFAULT_NOTION_PUBLISH);
        g.set(PublishTarget::Notion, PublishMode::Off);
        assert_eq!(g.notion, PublishMode::Off);
        assert_eq!(g.github, DEFAULT_GITHUB_PUBLISH);
    }

    #[test]
    fn an_unconfigured_target_stays_blocked_whatever_the_global_rule_says() {
        // the global rule never makes an unconfigured target publish.
        let cycle = Cycle::from_yaml_str(
            "id: NOMAD
name: Nomad
targets:
  discord: { webhook: \"cred:discord/nomad\" }
",
        )
        .unwrap();
        let g = global(PublishMode::Auto, PublishMode::Auto, PublishMode::Auto);
        let rules = TargetRules::for_repo(&cycle, &g, false, false);
        for t in [PublishTarget::Github, PublishTarget::Discord] {
            assert!(!rules.allows(t, Trigger::Auto), "{t:?}");
            assert!(!rules.allows(t, Trigger::Explicit), "{t:?}");
        }
        let ready = TargetRules::for_repo(&cycle, &g, true, true);
        assert!(ready.allows(PublishTarget::Discord, Trigger::Auto));
    }
}
