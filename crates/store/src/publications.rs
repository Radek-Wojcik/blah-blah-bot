//! Where a meeting was published, and which public repositories the user confirmed.
//!
//! A publication row is created when the user publishes a meeting (it points at the frozen
//! snapshot the operations read from) and filled in as the commit lands. A repository
//! confirmation records that the user accepted publishing meeting data to a repository that
//! is, or may be, public.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{Result, Store};

/// A publication row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Publication {
    pub meeting_id: String,
    pub cycle_id: String,
    /// Folder of the meeting in the cycle repository, once the commit is built.
    pub folder: Option<String>,
    /// Identity of the repository (see `repo_key`).
    pub repo_key: Option<String>,
    pub commit_sha: Option<String>,
    /// Link to the published report.
    pub report_url: Option<String>,
    /// Frozen copy of what is published, relative to the data root.
    pub snapshot_dir: String,
    /// Set when the commit has landed.
    pub published_unix_ms: Option<i64>,
}

/// A recorded confirmation to publish to a repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoConfirmation {
    pub repo_key: String,
    /// What the user confirmed against: `public` or `unknown`.
    pub visibility: String,
    pub confirmed_unix_ms: i64,
}

/// Key under which a repository is remembered: host and path in lower case, without
/// credentials, scheme or a trailing `.git` or slash (`github.com/owner/repo`).
pub fn repo_key(host: &str, path: &str) -> String {
    let path = path.trim().trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    format!(
        "{}/{}",
        host.trim().to_ascii_lowercase(),
        path.to_ascii_lowercase()
    )
}

fn publication_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Publication> {
    Ok(Publication {
        meeting_id: r.get(0)?,
        cycle_id: r.get(1)?,
        folder: r.get(2)?,
        repo_key: r.get(3)?,
        commit_sha: r.get(4)?,
        report_url: r.get(5)?,
        snapshot_dir: r.get(6)?,
        published_unix_ms: r.get(7)?,
    })
}

const PUBLICATION_COLUMNS: &str =
    "meeting_id, cycle_id, folder, repo_key, commit_sha, report_url, snapshot_dir, published_unix_ms";

impl Store {
    /// Create or replace the publication of a meeting.
    pub fn put_publication(&self, p: &Publication) -> Result<()> {
        self.conn.execute(
            "INSERT INTO publications (meeting_id, cycle_id, folder, repo_key, commit_sha,
                                       report_url, snapshot_dir, published_unix_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(meeting_id) DO UPDATE SET
               cycle_id = excluded.cycle_id, folder = excluded.folder,
               repo_key = excluded.repo_key, commit_sha = excluded.commit_sha,
               report_url = excluded.report_url, snapshot_dir = excluded.snapshot_dir,
               published_unix_ms = excluded.published_unix_ms",
            params![
                p.meeting_id,
                p.cycle_id,
                p.folder,
                p.repo_key,
                p.commit_sha,
                p.report_url,
                p.snapshot_dir,
                p.published_unix_ms
            ],
        )?;
        Ok(())
    }

    pub fn get_publication(&self, meeting_id: &str) -> Result<Option<Publication>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {PUBLICATION_COLUMNS} FROM publications WHERE meeting_id = ?1"),
                params![meeting_id],
                publication_row,
            )
            .optional()?)
    }

    /// Publications of a cycle, oldest first.
    pub fn list_publications(&self, cycle_id: &str) -> Result<Vec<Publication>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {PUBLICATION_COLUMNS} FROM publications WHERE cycle_id = ?1
             ORDER BY meeting_id"
        ))?;
        let rows = stmt.query_map(params![cycle_id], publication_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Record where the commit landed. Returns whether the meeting has a publication.
    pub fn record_commit(
        &self,
        meeting_id: &str,
        folder: &str,
        commit_sha: &str,
        report_url: Option<&str>,
        now_ms: i64,
    ) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE publications SET folder = ?2, commit_sha = ?3, report_url = ?4,
                published_unix_ms = ?5 WHERE meeting_id = ?1",
            params![meeting_id, folder, commit_sha, report_url, now_ms],
        )? > 0)
    }

    /// Remember that the user confirmed publishing to this repository, replacing an earlier
    /// confirmation.
    pub fn confirm_repo(&self, repo_key: &str, visibility: &str, now_ms: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO repo_confirmations (repo_key, visibility, confirmed_unix_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(repo_key) DO UPDATE SET
               visibility = excluded.visibility, confirmed_unix_ms = excluded.confirmed_unix_ms",
            params![repo_key, visibility, now_ms],
        )?;
        Ok(())
    }

    pub fn get_repo_confirmation(&self, repo_key: &str) -> Result<Option<RepoConfirmation>> {
        Ok(self
            .conn
            .query_row(
                "SELECT repo_key, visibility, confirmed_unix_ms FROM repo_confirmations
                 WHERE repo_key = ?1",
                params![repo_key],
                |r| {
                    Ok(RepoConfirmation {
                        repo_key: r.get(0)?,
                        visibility: r.get(1)?,
                        confirmed_unix_ms: r.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    /// Withdraw a confirmation. Returns whether there was one.
    pub fn clear_repo_confirmation(&self, repo_key: &str) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM repo_confirmations WHERE repo_key = ?1",
            params![repo_key],
        )? > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Meeting, MeetingState};

    fn store() -> Store {
        let s = Store::open_in_memory().unwrap();
        for id in ["20261113-140501", "20261120-090000"] {
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
        s
    }

    fn publication(meeting: &str, cycle: &str) -> Publication {
        Publication {
            meeting_id: meeting.into(),
            cycle_id: cycle.into(),
            folder: None,
            repo_key: Some("github.com/o/r".into()),
            commit_sha: None,
            report_url: None,
            snapshot_dir: "work/publish/1".into(),
            published_unix_ms: None,
        }
    }

    #[test]
    fn repo_keys_ignore_case_scheme_suffix_and_slashes() {
        assert_eq!(
            repo_key("GitHub.com", "/Owner/Repo.git/"),
            "github.com/owner/repo"
        );
        assert_eq!(
            repo_key("github.com", "owner/repo"),
            "github.com/owner/repo"
        );
        assert_eq!(repo_key("ghe.example", "o/r"), "ghe.example/o/r");
    }

    #[test]
    fn a_publication_is_created_then_filled_in_by_the_commit() {
        let s = store();
        let m = "20261113-140501";
        assert!(s.get_publication(m).unwrap().is_none());
        assert!(!s
            .record_commit(m, "NOMAD/2026-11-13", "abc", None, 1)
            .unwrap());
        s.put_publication(&publication(m, "NOMAD")).unwrap();
        assert_eq!(s.get_publication(m).unwrap().unwrap().folder, None);

        assert!(s
            .record_commit(
                m,
                "NOMAD/2026-11-13",
                "abc123",
                Some("https://example.test/r"),
                99
            )
            .unwrap());
        let p = s.get_publication(m).unwrap().unwrap();
        assert_eq!(p.folder.as_deref(), Some("NOMAD/2026-11-13"));
        assert_eq!(p.commit_sha.as_deref(), Some("abc123"));
        assert_eq!(p.report_url.as_deref(), Some("https://example.test/r"));
        assert_eq!(p.published_unix_ms, Some(99));
        assert_eq!(p.snapshot_dir, "work/publish/1");
    }

    #[test]
    fn putting_a_publication_again_replaces_it() {
        let s = store();
        let m = "20261113-140501";
        s.put_publication(&publication(m, "NOMAD")).unwrap();
        let mut again = publication(m, "NOMAD");
        again.snapshot_dir = "work/publish/2".into();
        s.put_publication(&again).unwrap();
        assert_eq!(s.get_publication(m).unwrap().unwrap(), again);
    }

    #[test]
    fn publications_are_listed_per_cycle() {
        let s = store();
        s.put_publication(&publication("20261120-090000", "NOMAD"))
            .unwrap();
        s.put_publication(&publication("20261113-140501", "NOMAD"))
            .unwrap();
        let ids: Vec<_> = s
            .list_publications("NOMAD")
            .unwrap()
            .into_iter()
            .map(|p| p.meeting_id)
            .collect();
        assert_eq!(ids, ["20261113-140501", "20261120-090000"]);
        assert!(s.list_publications("OTHER").unwrap().is_empty());
    }

    #[test]
    fn a_publication_needs_a_meeting_and_goes_with_it() {
        let s = store();
        assert!(s.put_publication(&publication("missing", "NOMAD")).is_err());
        let m = "20261113-140501";
        s.put_publication(&publication(m, "NOMAD")).unwrap();
        s.delete_meeting(m).unwrap();
        assert!(s.get_publication(m).unwrap().is_none());
    }

    #[test]
    fn a_confirmation_is_stored_replaced_and_cleared() {
        let s = store();
        assert!(s.get_repo_confirmation("github.com/o/r").unwrap().is_none());
        s.confirm_repo("github.com/o/r", "unknown", 5).unwrap();
        s.confirm_repo("github.com/o/r", "public", 9).unwrap();
        s.confirm_repo("github.com/o/other", "public", 9).unwrap();
        assert_eq!(
            s.get_repo_confirmation("github.com/o/r").unwrap().unwrap(),
            RepoConfirmation {
                repo_key: "github.com/o/r".into(),
                visibility: "public".into(),
                confirmed_unix_ms: 9
            }
        );
        assert!(s.clear_repo_confirmation("github.com/o/r").unwrap());
        assert!(!s.clear_repo_confirmation("github.com/o/r").unwrap());
        assert!(s.get_repo_confirmation("github.com/o/r").unwrap().is_none());
        assert!(s
            .get_repo_confirmation("github.com/o/other")
            .unwrap()
            .is_some());
    }
}
