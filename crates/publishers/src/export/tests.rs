use std::cell::RefCell;

use cycle_store::{ItemType, PublishMode};

use super::*;
use crate::testing::fixtures::{cycle, draft, item, source, task};

fn key(item: &str) -> TaskKey {
    TaskKey::new("NOMAD", "20261113-140501", item).unwrap()
}

fn record(item: &str) -> TaskRecord {
    let c = cycle(Some(PublishMode::OnApproval), None);
    let it = task(item, "Order the battery", Some("alex"), Some("2026-11-20"));
    let d = draft("20261113-140501", vec![it.clone()], vec![]);
    TaskRecord::from_item(&c, &d, &it, "20261113-140501")
}

// ----- the key ---------------------------------------------------------------------------

#[test]
fn a_key_round_trips_through_its_text() {
    let k = key("i3");
    assert_eq!(k.text(), "bbb:v1:NOMAD:20261113-140501:i3");
    assert_eq!(TaskKey::parse(&k.text()), Some(k));
    assert_eq!(
        TaskKey::parse("  bbb:v1:NOMAD:20261113-140501:i3\n"),
        Some(key("i3"))
    );
}

#[test]
fn keys_differ_across_cycle_meeting_and_item() {
    let base = TaskKey::new("A", "m1", "i1").unwrap();
    let others = [
        TaskKey::new("B", "m1", "i1").unwrap(),
        TaskKey::new("A", "m2", "i1").unwrap(),
        TaskKey::new("A", "m1", "i2").unwrap(),
    ];
    for other in &others {
        assert_ne!(base.text(), other.text());
    }
    // The separator keeps neighbouring ids apart.
    assert_ne!(
        TaskKey::new("A", "mi", "1").unwrap().text(),
        TaskKey::new("A", "m", "i1").unwrap().text()
    );
}

#[test]
fn ids_that_cannot_be_told_apart_are_refused() {
    for bad in ["", "a:b", "a b", "a\nb", &"x".repeat(101)] {
        assert!(TaskKey::new(bad, "m", "i").is_none(), "{bad:?}");
        assert!(TaskKey::new("c", bad, "i").is_none(), "{bad:?}");
        assert!(TaskKey::new("c", "m", bad).is_none(), "{bad:?}");
    }
    for bad in [
        "",
        "bbb:v1:",
        "bbb:v1:a:b",
        "bbb:v1:a:b:c:d",
        "bbb:v2:a:b:c",
        "bbb:v1::b:c",
        "NOMAD:m:i",
        "bbb:v1:a:b:c d",
    ] {
        assert!(TaskKey::parse(bad).is_none(), "{bad:?}");
    }
    let broken = TaskKey {
        cycle: "a:b".into(),
        meeting: "m".into(),
        item: "i".into(),
    };
    assert!(!broken.is_valid());
}

// ----- the record ------------------------------------------------------------------------

#[test]
fn a_record_is_built_from_the_item_and_the_cycle() {
    let r = record("i3");
    assert_eq!(r.key, key("i3"));
    assert_eq!(r.title, "Order the battery");
    let a = r.assignee.as_ref().unwrap();
    assert_eq!(a.participant_id, "alex");
    assert_eq!(a.name, "Alex Example");
    assert_eq!(a.github.as_deref(), Some("alex-gh"));
    assert_eq!(r.due.as_deref(), Some("2026-11-20"));
    assert_eq!(r.meeting_date, "2026-11-13");
    assert_eq!(r.cycle_name, "Nomad rover");
    assert_eq!(r.quote.as_deref(), Some("We will do it by Friday"));
    assert_eq!(r.time_ms, Some(745_000));
    assert_eq!(r.report_url, None);
    assert_eq!(r.transcript_url, None);
    assert_eq!(r.transcript_anchor, None);
}

#[test]
fn an_unverified_quote_is_dropped_and_an_unknown_assignee_is_none() {
    let c = cycle(None, None);
    let mut it = item("i1", ItemType::Task, "Write the report");
    it.assignee = Some("nobody".into());
    it.source = Some(source(false, 1000));
    let d = draft("m1", vec![it.clone()], vec![]);
    let r = TaskRecord::from_item(&c, &d, &it, "m1");
    assert_eq!(r.assignee, None);
    assert_eq!(r.quote, None);
    // The time stays: it is where the item was said, verified or not.
    assert_eq!(r.time_ms, Some(1000));

    it.source = None;
    let r = TaskRecord::from_item(&c, &d, &it, "m1");
    assert_eq!((r.quote, r.time_ms), (None, None));
}

// ----- the algorithm ---------------------------------------------------------------------

/// An exporter that lives in memory and remembers what it was asked.
#[derive(Default)]
struct Memory {
    items: RefCell<Vec<(ExternalTask, String)>>,
    calls: RefCell<Vec<String>>,
    fail_create: bool,
}

impl Memory {
    fn with(items: &[(&str, &TaskKey, bool)]) -> Memory {
        let m = Memory::default();
        for (id, key, trashed) in items {
            m.items.borrow_mut().push((
                ExternalTask {
                    id: (*id).into(),
                    url: Some(format!("https://example.test/{id}")),
                    trashed: *trashed,
                },
                key.text(),
            ));
        }
        m
    }

    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
}

impl TaskExporter for Memory {
    fn name(&self) -> &'static str {
        "memory"
    }
    fn noun(&self) -> &'static str {
        "page"
    }
    fn get(&self, id: &str) -> Result<Option<ExternalTask>, HttpError> {
        self.calls.borrow_mut().push(format!("get {id}"));
        Ok(self
            .items
            .borrow()
            .iter()
            .find(|(t, _)| t.id == id)
            .map(|(t, _)| t.clone()))
    }
    fn find(&self, key: &TaskKey) -> Result<Vec<ExternalTask>, HttpError> {
        self.calls.borrow_mut().push("find".into());
        Ok(self
            .items
            .borrow()
            .iter()
            .filter(|(_, k)| *k == key.text())
            .map(|(t, _)| t.clone())
            .collect())
    }
    fn create(&self, task: &TaskRecord) -> Result<ExternalTask, HttpError> {
        self.calls.borrow_mut().push("create".into());
        if self.fail_create {
            return Err(permanent_error(ErrorKind::Status, "refused"));
        }
        let id = format!("p{}", self.items.borrow().len() + 1);
        let made = ExternalTask {
            id,
            url: None,
            trashed: false,
        };
        self.items
            .borrow_mut()
            .push((made.clone(), task.key.text()));
        Ok(made)
    }
    fn update(&self, id: &str, _task: &TaskRecord) -> Result<ExternalTask, HttpError> {
        self.calls.borrow_mut().push(format!("update {id}"));
        Ok(ExternalTask {
            id: id.into(),
            url: Some("https://example.test/updated".into()),
            trashed: false,
        })
    }
}

fn run(
    x: &Memory,
    known: Option<&str>,
    update_existing: bool,
) -> (Result<Upserted, HttpError>, Vec<String>) {
    let mut stored = Vec::new();
    let result = upsert(x, &record("i1"), known, update_existing, &mut |t| {
        stored.push(t.id.clone());
    });
    (result, stored)
}

#[test]
fn a_known_live_item_is_updated() {
    let x = Memory::with(&[("p1", &key("i1"), false)]);
    let (result, stored) = run(&x, Some("p1"), true);
    let done = result.unwrap();
    assert!(matches!(&done.outcome, UpsertOutcome::Updated(t) if t.id == "p1"));
    assert!(!done.found_by_key);
    assert_eq!(done.duplicates, 0);
    assert_eq!(x.calls(), ["get p1", "update p1"], "no key lookup");
    assert!(stored.is_empty(), "the id is known already");
}

#[test]
fn a_known_live_item_is_left_alone_when_updates_are_off() {
    let x = Memory::with(&[("p1", &key("i1"), false)]);
    let (result, _) = run(&x, Some("p1"), false);
    assert!(matches!(
        result.unwrap().outcome,
        UpsertOutcome::Unchanged(t) if t.id == "p1"
    ));
    assert_eq!(x.calls(), ["get p1"]);
}

#[test]
fn a_known_trashed_item_is_removed_not_recreated() {
    let x = Memory::with(&[("p1", &key("i1"), true)]);
    let (result, stored) = run(&x, Some("p1"), true);
    assert_eq!(
        result.unwrap().outcome,
        UpsertOutcome::Removed { id: "p1".into() }
    );
    assert_eq!(x.calls(), ["get p1"]);
    assert!(stored.is_empty());
}

#[test]
fn a_known_item_that_is_gone_is_a_permanent_error() {
    let x = Memory::default();
    let (result, _) = run(&x, Some("p9"), true);
    let err = result.unwrap_err();
    assert_eq!(err.failure(), store::Failure::Permanent);
    assert_eq!(err.status, Some(404));
    assert!(
        err.message
            .contains("page was deleted or is no longer shared"),
        "{}",
        err.message
    );
    assert_eq!(x.calls(), ["get p9"], "nothing is created in its place");
}

#[test]
fn without_an_id_the_key_is_looked_up_and_the_hit_is_stored() {
    let x = Memory::with(&[("p1", &key("i1"), false), ("p2", &key("i2"), false)]);
    let (result, stored) = run(&x, None, true);
    let done = result.unwrap();
    assert!(matches!(&done.outcome, UpsertOutcome::Updated(t) if t.id == "p1"));
    assert!(done.found_by_key);
    assert_eq!(done.duplicates, 0);
    assert_eq!(stored, ["p1"]);
    assert_eq!(x.calls(), ["find", "update p1"]);
}

#[test]
fn a_hit_is_stored_even_when_it_was_trashed() {
    let x = Memory::with(&[("p1", &key("i1"), true)]);
    let (result, stored) = run(&x, None, true);
    assert_eq!(
        result.unwrap().outcome,
        UpsertOutcome::Removed { id: "p1".into() }
    );
    assert_eq!(stored, ["p1"]);
}

#[test]
fn several_hits_use_the_oldest_and_are_counted_not_deleted() {
    let x = Memory::with(&[
        ("p1", &key("i1"), false),
        ("p2", &key("i1"), false),
        ("p3", &key("i1"), false),
    ]);
    let (result, stored) = run(&x, None, true);
    let done = result.unwrap();
    assert_eq!(done.outcome.id(), "p1");
    assert_eq!(done.duplicates, 2);
    assert_eq!(stored, ["p1"]);
    assert_eq!(x.items.borrow().len(), 3, "nothing is deleted");
}

#[test]
fn no_hit_creates_and_hands_the_id_over_before_returning() {
    let x = Memory::default();
    let order = RefCell::new(Vec::new());
    let done = upsert(&x, &record("i1"), None, true, &mut |t| {
        order
            .borrow_mut()
            .push(format!("stored {} after {:?}", t.id, x.calls()));
    })
    .unwrap();
    assert!(matches!(&done.outcome, UpsertOutcome::Created(t) if t.id == "p1"));
    assert!(!done.found_by_key);
    assert_eq!(
        *order.borrow(),
        ["stored p1 after [\"find\", \"create\"]".to_string()],
        "called once, right after the creation"
    );
    assert_eq!(x.calls(), ["find", "create"], "a new item is not updated");

    // The second run finds it by its key instead of making another one.
    let (again, _) = run(&x, None, false);
    assert!(matches!(
        again.unwrap().outcome,
        UpsertOutcome::Unchanged(_)
    ));
    assert_eq!(x.items.borrow().len(), 1);
}

#[test]
fn a_failed_creation_stores_nothing() {
    let x = Memory {
        fail_create: true,
        ..Memory::default()
    };
    let (result, stored) = run(&x, None, true);
    assert!(result.is_err());
    assert!(stored.is_empty());
}

#[test]
fn an_unusable_key_is_refused_before_any_request() {
    let x = Memory::default();
    let mut r = record("i1");
    r.key.item = "i:1".into();
    let err = upsert(&x, &r, None, true, &mut |_| {}).unwrap_err();
    assert_eq!(err.failure(), store::Failure::Permanent);
    assert!(x.calls().is_empty());
}
