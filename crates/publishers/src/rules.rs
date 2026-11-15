//! Which targets a meeting goes to, and when: the per-target rules of `cycle.yaml`
//! (`targets.<name>.publish`: `auto`, `on-approval` or `off`) and their defaults.
//!
//! The defaults are two constants in `cycle-store` (`DEFAULT_GITHUB_PUBLISH`,
//! `DEFAULT_DISCORD_PUBLISH`), read by the parser of `cycle.yaml` and by these rules alike:
//! changing one is a change of a constant. They are proposals of ADR 0018, chosen because
//! what is published cannot be taken back: a pushed transcript stays in the history of the
//! repository, and an issue assignment or a Discord ping has already notified somebody.
//!
//! * `auto`: the operations are queued as soon as the draft of a meeting is complete (no newer
//!   draft waits for a decision). Status changes are never part of it.
//! * `on-approval`: queued when the user presses Publish for that target.
//! * `off`: never, not even when the user asks.

use cycle_store::{Cycle, PublishMode};

pub use cycle_store::{DEFAULT_DISCORD_PUBLISH, DEFAULT_GITHUB_PUBLISH};
/// Status changes (closing or reopening an issue or a topic) are written back only after the
/// user approved each one, with every mode. This is not a setting: a model's guess that a
/// task is done is not enough to close an issue of other people.
pub const STATUS_CHANGES_NEED_APPROVAL: bool = true;

/// A place a meeting can be published to. GitHub covers the commit with the report and the
/// transcript, the issues and the status changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PublishTarget {
    Github,
    Discord,
}

impl PublishTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Discord => "discord",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "github" => Some(Self::Github),
            "discord" => Some(Self::Discord),
            _ => None,
        }
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
}

impl TargetRules {
    /// The rules of `cycle`, with the defaults where it says nothing.
    pub fn of(cycle: &Cycle) -> Self {
        Self {
            // A cycle without `targets.github` still has the report target, with the default.
            github: cycle
                .targets
                .github
                .as_ref()
                .map_or(DEFAULT_GITHUB_PUBLISH, |t| t.publish),
            issues: cycle.targets.github.as_ref().is_none_or(|t| t.issues),
            // A cycle without `targets.discord` has no Discord target at all.
            discord: cycle.targets.discord.as_ref().map(|t| t.publish),
        }
    }

    /// The mode of a target; `None` when the cycle has no such target.
    pub fn mode(&self, target: PublishTarget) -> Option<PublishMode> {
        match target {
            PublishTarget::Github => Some(self.github),
            PublishTarget::Discord => self.discord,
        }
    }

    /// Whether `trigger` may publish to `target`: `auto` only for targets set to auto,
    /// an explicit request for everything but `off`.
    pub fn allows(&self, target: PublishTarget, trigger: Trigger) -> bool {
        match (self.mode(target), trigger) {
            (Some(PublishMode::Auto), _) => true,
            (Some(PublishMode::OnApproval), Trigger::Explicit) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cycle_store::{DiscordTarget, GithubTarget};

    fn with(github: Option<PublishMode>, discord: Option<PublishMode>) -> Cycle {
        let mut c = Cycle::new("NOMAD", "Nomad");
        c.targets.github = github.map(|publish| GithubTarget {
            publish,
            ..GithubTarget::default()
        });
        c.targets.discord = discord.map(|publish| DiscordTarget {
            webhook: "cred:discord/nomad".into(),
            publish,
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
        for t in [PublishTarget::Github, PublishTarget::Discord] {
            assert_eq!(PublishTarget::parse(t.as_str()), Some(t));
        }
        assert_eq!(PublishTarget::parse("notion"), None);
        assert!(STATUS_CHANGES_NEED_APPROVAL);
    }
}
