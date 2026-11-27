//! Where the items and posts of a meeting live in external services (a Notion page, the
//! Discord message of the summary). Ids and links only, never secrets.

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::{Result, Store, StoreError};

/// The service a reference points into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefTarget {
    Notion,
    Discord,
}

impl RefTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Notion => "notion",
            Self::Discord => "discord",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        match s {
            "notion" => Ok(Self::Notion),
            "discord" => Ok(Self::Discord),
            other => Err(StoreError::UnknownState(other.to_string())),
        }
    }
}

/// One reference. `ref_key` is `task:<item id>` for Notion and `post` for Discord;
/// `external_id` is the Notion page id or the id of the first Discord message; `extra` is JSON
/// (`{}`, or `{"channel_id","thread_id","message_ids":[..]}` for Discord).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalRef {
    pub meeting_id: String,
    pub target: RefTarget,
    pub ref_key: String,
    pub external_id: String,
    pub url: Option<String>,
    pub extra: String,
    pub updated_unix_ms: i64,
}

const COLUMNS: &str = "meeting_id, target, ref_key, external_id, url, extra, updated_unix_ms";

fn ref_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<ExternalRef>> {
    let target: String = r.get(1)?;
    let (meeting_id, ref_key, external_id) = (r.get(0)?, r.get(2)?, r.get(3)?);
    let (url, extra, updated_unix_ms) = (r.get(4)?, r.get(5)?, r.get(6)?);
    Ok(RefTarget::parse(&target).map(|target| ExternalRef {
        meeting_id,
        target,
        ref_key,
        external_id,
        url,
        extra,
        updated_unix_ms,
    }))
}

impl Store {
    /// Insert the reference, or replace the one with the same meeting, target and key.
    pub fn put_external_ref(&self, r: &ExternalRef) -> Result<()> {
        self.conn.execute(
            "INSERT INTO external_refs (meeting_id, target, ref_key, external_id, url, extra,
                                        updated_unix_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (meeting_id, target, ref_key) DO UPDATE SET
                 external_id = excluded.external_id, url = excluded.url,
                 extra = excluded.extra, updated_unix_ms = excluded.updated_unix_ms",
            params![
                r.meeting_id,
                r.target.as_str(),
                r.ref_key,
                r.external_id,
                r.url,
                r.extra,
                r.updated_unix_ms
            ],
        )?;
        Ok(())
    }

    pub fn get_external_ref(
        &self,
        meeting_id: &str,
        target: RefTarget,
        ref_key: &str,
    ) -> Result<Option<ExternalRef>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM external_refs
             WHERE meeting_id = ?1 AND target = ?2 AND ref_key = ?3"
        ))?;
        let mut rows = stmt.query_map(params![meeting_id, target.as_str(), ref_key], ref_row)?;
        rows.next().transpose()?.transpose()
    }

    /// Every reference of a meeting, ordered by target then key.
    pub fn list_external_refs(&self, meeting_id: &str) -> Result<Vec<ExternalRef>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM external_refs WHERE meeting_id = ?1
             ORDER BY target, ref_key"
        ))?;
        let rows = stmt.query_map(params![meeting_id], ref_row)?;
        rows.map(|r| r?).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Meeting, MeetingState};

    fn store() -> Store {
        let s = Store::open_in_memory().unwrap();
        for id in ["m1", "m2"] {
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

    fn page(meeting: &str, key: &str, id: &str) -> ExternalRef {
        ExternalRef {
            meeting_id: meeting.into(),
            target: RefTarget::Notion,
            ref_key: key.into(),
            external_id: id.into(),
            url: Some(format!("https://example.test/{id}")),
            extra: "{}".into(),
            updated_unix_ms: 1,
        }
    }

    #[test]
    fn a_reference_is_stored_and_replaced_by_the_same_key() {
        let s = store();
        assert!(s
            .get_external_ref("m1", RefTarget::Notion, "task:i1")
            .unwrap()
            .is_none());
        s.put_external_ref(&page("m1", "task:i1", "p1")).unwrap();
        let mut again = page("m1", "task:i1", "p2");
        again.url = None;
        again.extra = r#"{"a":1}"#.into();
        again.updated_unix_ms = 9;
        s.put_external_ref(&again).unwrap();
        let got = s
            .get_external_ref("m1", RefTarget::Notion, "task:i1")
            .unwrap()
            .unwrap();
        assert_eq!(got, again);
        assert_eq!(s.list_external_refs("m1").unwrap().len(), 1);
        // Another target or meeting with the same key is a different reference.
        let mut post = page("m1", "task:i1", "d1");
        post.target = RefTarget::Discord;
        s.put_external_ref(&post).unwrap();
        s.put_external_ref(&page("m2", "task:i1", "p3")).unwrap();
        assert_eq!(s.list_external_refs("m1").unwrap().len(), 2);
        assert_eq!(s.list_external_refs("m2").unwrap().len(), 1);
    }

    #[test]
    fn references_are_listed_in_a_stable_order_and_go_with_the_meeting() {
        let s = store();
        s.put_external_ref(&page("m1", "task:b", "pb")).unwrap();
        s.put_external_ref(&page("m1", "task:a", "pa")).unwrap();
        let mut post = page("m1", "post", "d1");
        post.target = RefTarget::Discord;
        s.put_external_ref(&post).unwrap();
        let keys: Vec<_> = s
            .list_external_refs("m1")
            .unwrap()
            .into_iter()
            .map(|r| (r.target, r.ref_key))
            .collect();
        assert_eq!(
            keys,
            vec![
                (RefTarget::Discord, "post".to_string()),
                (RefTarget::Notion, "task:a".to_string()),
                (RefTarget::Notion, "task:b".to_string()),
            ]
        );
        s.put_external_ref(&page("m2", "task:a", "pz")).unwrap();
        s.delete_meeting("m1").unwrap();
        assert!(s.list_external_refs("m1").unwrap().is_empty());
        assert_eq!(s.list_external_refs("m2").unwrap().len(), 1);
    }

    #[test]
    fn a_reference_for_an_unknown_meeting_or_service_is_refused() {
        let s = store();
        assert!(s.put_external_ref(&page("nope", "task:a", "p")).is_err());
        assert!(s
            .conn
            .execute(
                "INSERT INTO external_refs (meeting_id, target, ref_key, external_id,
                                            updated_unix_ms)
                 VALUES ('m1', 'slack', 'x', 'y', 1)",
                [],
            )
            .is_err());
    }
}
