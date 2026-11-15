//! The warning for a public repository.
//!
//! A report, a transcript and issues in a public repository are public. Before anything
//! goes to a repository that is public, or whose visibility could not be found out, the
//! user confirms it once per repository (`repo_confirmations` in the store). The check is made
//! in three places with this one rule: when the publish plan is built, when the command that
//! starts a publication runs, and again by the dispatcher right before it sends anything
//! to the repository, so that an operation queued earlier (or while offline) cannot slip
//! past a repository that has become public since.

use crate::github::{Github, RepoRef};

/// Prefix of the error of an operation that was refused by the check; the dispatcher
/// stores it so a confirmation can find the operations it unblocks.
pub const PUBLIC_REPO_CODE: &str = "public_repo_unconfirmed";

/// What is known about the visibility of a repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RepoVisibility {
    Public,
    /// Private or internal.
    NotPublic,
    /// The answer could not be had (offline, no access, a host that is not GitHub). Treated
    /// like public: the warning is for what could be seen by everybody.
    Unknown,
}

impl RepoVisibility {
    /// What is stored with a confirmation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::NotPublic => "private",
            Self::Unknown => "unknown",
        }
    }

    pub fn needs_confirmation(self) -> bool {
        self != Self::NotPublic
    }
}

/// The check failed: the repository is (or may be) public and the user has not confirmed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{PUBLIC_REPO_CODE}: the repository is {}, so what is published is visible to everybody; confirm publishing to it first", .visibility.as_str())]
pub struct PublicRepoUnconfirmed {
    pub visibility: RepoVisibility,
}

/// The one rule: a repository that is not private needs the user's confirmation.
pub fn require(visibility: RepoVisibility, confirmed: bool) -> Result<(), PublicRepoUnconfirmed> {
    if visibility.needs_confirmation() && !confirmed {
        Err(PublicRepoUnconfirmed { visibility })
    } else {
        Ok(())
    }
}

/// Asks GitHub. A repository that cannot be read is `Unknown`, never "private".
pub fn probe(github: &Github, repo: &RepoRef) -> RepoVisibility {
    match github.repo_info(repo) {
        Ok(info) if info.is_public() => RepoVisibility::Public,
        Ok(_) => RepoVisibility::NotPublic,
        Err(_) => RepoVisibility::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::HttpClient;
    use crate::testing::{FakeGithub, Visibility};
    use std::time::Duration;

    #[test]
    fn only_a_private_repository_passes_without_a_confirmation() {
        assert!(require(RepoVisibility::NotPublic, false).is_ok());
        for v in [RepoVisibility::Public, RepoVisibility::Unknown] {
            let err = require(v, false).unwrap_err();
            assert_eq!(err.visibility, v);
            assert!(err.to_string().starts_with(PUBLIC_REPO_CODE));
            assert!(require(v, true).is_ok());
        }
    }

    #[test]
    fn the_probe_reads_visibility_and_never_guesses_private() {
        let gh = FakeGithub::start();
        gh.add_repo("o/r", Visibility::Private);
        gh.add_repo("o/pub", Visibility::Public);
        gh.add_repo("o/inner", Visibility::Internal);
        let client = || HttpClient::with_timeouts(Duration::from_secs(2), Duration::from_secs(5));
        let api = Github::with_client(client(), &gh.api_base(), "t").unwrap();
        let of = |name: &str| probe(&api, &RepoRef::parse(name).unwrap());
        assert_eq!(of("o/r"), RepoVisibility::NotPublic);
        assert_eq!(of("o/inner"), RepoVisibility::NotPublic);
        assert_eq!(of("o/pub"), RepoVisibility::Public);
        assert_eq!(of("o/missing"), RepoVisibility::Unknown);
        gh.go_offline();
        assert_eq!(of("o/r"), RepoVisibility::Unknown);
    }
}
