//! Tests of the publishing glue: planning, the lock, the rules, the gate and the tasks, over
//! temporary repositories and the fake servers. No Tauri and no real host.

use std::sync::{Arc, Mutex};

use cycle_store::issues::{IssueCache, IssueLink, OpenIssue};
use cycle_store::{Cycle, Decision, Draft, ItemType, ProposedStatus, PublishMode, Repo};
use publishers::testing::fixtures::{
    cycle as fixture_cycle, draft as fixture_draft, item, proposal, task,
};
use publishers::testing::lab::{git, Lab as PublishLab, LabOptions, REPO, TOKEN};
use publishers::testing::Visibility;
use queue::draft::DraftFiles;
use store::{Meeting, MeetingState, OpState, Store};

use super::*;
use crate::cycles::tests::Lab;
use crate::draft_commands::{add_proposal, ensure_draft_unlocked, set_decision};
use crate::publish_secrets::set_github_repo;
use crate::secrets::MemorySecrets;
use crate::settings::{CycleRepoSetting, RepoKind};

const MEETING: &str = "20261113-140501";

/// Repositories, a store with one meeting, and a publishing service over them.
struct Rig {
    _keep: Option<tempfile::TempDir>,
    root: PathBuf,
    repos: Arc<Mutex<Vec<CycleRepoSetting>>>,
    secrets: Arc<MemorySecrets>,
    store: Arc<Mutex<Store>>,
    publishing: Arc<Publishing>,
}

impl Rig {
    fn new(root: PathBuf, repos: Vec<CycleRepoSetting>, keep: Option<tempfile::TempDir>) -> Rig {
        std::fs::create_dir_all(root.join(MEETING).join("work")).unwrap();
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        store
            .lock()
            .unwrap()
            .create_meeting(&Meeting {
                id: MEETING.into(),
                title: "Weekly".into(),
                started_unix_ms: 1,
                duration_ms: None,
                state: MeetingState::Stopped,
                dir: MEETING.into(),
            })
            .unwrap();
        let repos = Arc::new(Mutex::new(repos));
        let secrets = Arc::new(MemorySecrets::default());
        let (r, root2) = (repos.clone(), root.clone());
        let env = Arc::new(AppEnv::new(
            move || r.lock().unwrap().clone(),
            move || root2.clone(),
            secrets.clone(),
        ));
        let publishing = Publishing::new(store.clone(), env);
        Rig {
            _keep: keep,
            root,
            repos,
            secrets,
            store,
            publishing,
        }
    }

    /// The default local repository of the cycle lab, with the cycle NOMAD.
    fn local(cycle: Option<Cycle>) -> (Rig, Lab) {
        let lab = Lab::new();
        lab.make_cycle("NOMAD");
        if let Some(c) = cycle {
            lab.repo().save_cycle(&c).unwrap();
        }
        let rig = Rig::new(
            lab.dir.path().join("recordings"),
            lab.repos().to_vec(),
            None,
        );
        (rig, lab)
    }

    fn meeting_dir(&self) -> PathBuf {
        self.root.join(MEETING)
    }

    fn save_draft(&self, d: &Draft) {
        DraftFiles::new(&self.meeting_dir()).save(d).unwrap();
    }

    fn tick(&self) -> publishers::outbox::TickReport {
        self.publishing.dispatcher.tick()
    }

    fn ops(&self) -> Vec<store::OutboxOp> {
        self.store.lock().unwrap().list_ops(None).unwrap()
    }
}

fn draft() -> Draft {
    fixture_draft(
        MEETING,
        vec![
            task("i1", "Design the mount", Some("alex"), Some("2026-11-20")),
            item("i2", ItemType::Decision, "Use six wheels"),
        ],
        vec![],
    )
}

fn github_only() -> Vec<String> {
    vec!["github".to_string()]
}

#[test]
fn the_plan_shows_the_rules_and_what_would_happen_without_changing_anything() {
    let (rig, _lab) = Rig::local(Some(fixture_cycle(
        Some(PublishMode::OnApproval),
        Some(("cred:discord/nomad", PublishMode::Off)),
    )));
    rig.save_draft(&draft());
    let plan = rig.publishing.plan(MEETING, &[]).unwrap();
    assert_eq!(plan.cycle_id, "NOMAD");
    let modes: Vec<_> = plan
        .targets
        .iter()
        .map(|t| (t.target, t.mode, t.available))
        .collect();
    assert_eq!(
        modes,
        [
            ("github", Some("on-approval"), true),
            ("discord", Some("off"), false)
        ]
    );
    assert!(plan.summary.commit && !plan.summary.pushes);
    assert!(
        plan.summary.issues.is_empty(),
        "the repository is not linked to GitHub"
    );
    assert!(
        plan.notes
            .iter()
            .any(|n| n.contains("not linked to GitHub")),
        "{:?}",
        plan.notes
    );
    assert!(!plan.locked && plan.blocks.is_empty());
    assert!(rig.ops().is_empty(), "planning queues nothing");
    assert!(
        !rig.meeting_dir().join("work/publish").exists(),
        "and freezes nothing"
    );
}

#[test]
fn publishing_freezes_the_draft_locks_it_and_the_dispatcher_commits_it() {
    let (rig, lab) = Rig::local(None);
    let d = draft();
    rig.save_draft(&d);
    let before = std::fs::read(rig.meeting_dir().join("work/draft.json")).unwrap();

    let result = rig.publishing.publish(MEETING, &github_only()).unwrap();
    assert_eq!(result.queued, 1);
    assert!(result.plan.locked);
    assert_eq!(result.plan.ops.len(), 1);
    assert_eq!(
        result.plan.ops[0].label,
        "Add the report and transcript to the repository"
    );

    // The Stage 4 draft file is untouched; the snapshot is next to it.
    assert_eq!(
        std::fs::read(rig.meeting_dir().join("work/draft.json")).unwrap(),
        before
    );
    let snapshot = rig.meeting_dir().join("work/publish/1");
    for file in ["meeting.json", "report.md"] {
        assert!(snapshot.join(file).is_file(), "{file}");
    }
    assert_eq!(
        Draft::from_json_str(&std::fs::read_to_string(snapshot.join("meeting.json")).unwrap())
            .unwrap(),
        d
    );

    // From now on the draft is read-only.
    let err = ensure_draft_unlocked(&rig.store.lock().unwrap(), MEETING).unwrap_err();
    assert_eq!(err.code, "published");

    assert_eq!(rig.tick().done, 1);
    let published = lab
        .repo()
        .root()
        .join("meetings/NOMAD/2026-11-13/meeting.json");
    assert!(published.is_file());
    let state = rig.publishing.state().unwrap();
    assert_eq!(state.ops[0].state, "done");
    assert!(state.offline.is_empty());
    assert_eq!(
        state.ops[0].result.as_ref().unwrap()["folder"],
        "2026-11-13"
    );
}

#[test]
fn cancelling_the_operations_unlocks_the_draft_and_a_running_one_cannot_be_cancelled() {
    let (rig, _lab) = Rig::local(None);
    rig.save_draft(&draft());
    rig.publishing.publish(MEETING, &github_only()).unwrap();
    let id = rig.ops()[0].id;

    // Claimed, as a running dispatcher holds it.
    rig.store
        .lock()
        .unwrap()
        .claim_next_op(rig.publishing.env.now_ms(), &[])
        .unwrap()
        .unwrap();
    assert_eq!(rig.publishing.cancel(id).unwrap_err().code, "busy");
    rig.store
        .lock()
        .unwrap()
        .fail_op(
            id,
            store::Failure::Permanent,
            "nope",
            rig.publishing.env.now_ms(),
        )
        .unwrap();

    let state = rig.publishing.cancel(id).unwrap();
    assert_eq!(state.ops[0].state, "cancelled");
    assert!(ensure_draft_unlocked(&rig.store.lock().unwrap(), MEETING).is_ok());
    assert_eq!(rig.publishing.cancel(999).unwrap_err().code, "not_found");

    // Publishing again starts over with a new snapshot.
    let again = rig.publishing.publish(MEETING, &github_only()).unwrap();
    assert_eq!(again.queued, 1);
    assert!(rig.meeting_dir().join("work/publish/2").is_dir());
}

#[test]
fn a_newer_draft_must_be_decided_before_publishing() {
    let (rig, _lab) = Rig::local(None);
    rig.save_draft(&draft());
    std::fs::write(
        rig.meeting_dir().join("work/draft.new.json"),
        serde_json::to_vec(&draft().to_value()).unwrap(),
    )
    .unwrap();
    let err = rig.publishing.publish(MEETING, &github_only()).unwrap_err();
    assert_eq!(err.code, "new_draft_pending");
    assert!(rig.ops().is_empty());
    assert!(rig.publishing.plan(MEETING, &[]).unwrap().new_draft_pending);
}

#[test]
fn a_target_that_is_off_or_missing_cannot_be_published() {
    let (rig, _lab) = Rig::local(Some(fixture_cycle(Some(PublishMode::Off), None)));
    rig.save_draft(&draft());
    let err = rig.publishing.publish(MEETING, &github_only()).unwrap_err();
    assert_eq!(err.code, "nothing_to_publish");
    let err = rig
        .publishing
        .publish(MEETING, &["notion".to_string()])
        .unwrap_err();
    assert_eq!(err.code, "invalid");
    let err = rig.publishing.publish(MEETING, &[]).unwrap_err();
    assert_eq!(err.code, "invalid");
    assert!(rig.ops().is_empty());
}

#[test]
fn status_changes_are_decided_in_the_draft_and_only_approved_ones_are_queued() {
    let (rig, _lab) = Rig::local(None);
    let mut d = draft();
    d.status_proposals = vec![
        proposal("s1", "#12", ProposedStatus::Done, Decision::Proposed),
        proposal("s2", "#13", ProposedStatus::Done, Decision::Proposed),
    ];
    rig.save_draft(&d);
    let repos = rig.repos.lock().unwrap().clone();
    let dir = rig.meeting_dir();
    let view = set_decision(
        &repos,
        &dir,
        "s1",
        Decision::Approved,
        Some("Done on Friday".into()),
    )
    .unwrap();
    assert_eq!(view.draft.status_proposals[0].decision, Decision::Approved);
    assert_eq!(
        view.draft.status_proposals[0].comment.as_deref(),
        Some("Done on Friday")
    );
    let view = set_decision(&repos, &dir, "s2", Decision::Rejected, None).unwrap();
    assert_eq!(view.draft.status_proposals[1].decision, Decision::Rejected);
    assert_eq!(
        set_decision(&repos, &dir, "s9", Decision::Approved, None)
            .unwrap_err()
            .code,
        "not_found"
    );

    // Added by hand: an issue, or a topic that exists; waiting for approval.
    let view = add_proposal(&repos, &dir, "#14", ProposedStatus::Done, None).unwrap();
    assert_eq!(view.draft.status_proposals[2].decision, Decision::Proposed);
    assert_eq!(
        add_proposal(&repos, &dir, "t-nope", ProposedStatus::Done, None)
            .unwrap_err()
            .code,
        "invalid"
    );

    let plan = rig.publishing.plan(MEETING, &github_only()).unwrap();
    assert_eq!(plan.summary.proposals_pending, 1, "the one added by hand");
    assert!(
        plan.summary.statuses.is_empty(),
        "no GitHub link here, so nothing to write back"
    );
}

#[test]
fn an_auto_cycle_publishes_a_finished_draft_by_itself_but_never_a_status_change() {
    let (rig, _lab) = Rig::local(Some(fixture_cycle(Some(PublishMode::Auto), None)));
    let mut d = draft();
    d.status_proposals = vec![proposal(
        "s1",
        "#12",
        ProposedStatus::Done,
        Decision::Approved,
    )];
    rig.save_draft(&d);
    assert_eq!(rig.publishing.auto_publish(MEETING).unwrap(), 1);
    let ops = rig.ops();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].op_key, "commit");
    // A second completion changes nothing.
    assert_eq!(rig.publishing.auto_publish(MEETING).unwrap(), 0);
}

#[test]
fn a_cycle_on_approval_waits_for_the_user_and_a_pending_draft_stops_the_auto_rule() {
    let (rig, _lab) = Rig::local(None);
    rig.save_draft(&draft());
    assert_eq!(rig.publishing.auto_publish(MEETING).unwrap(), 0);
    assert!(rig.ops().is_empty());

    let (auto, _lab) = Rig::local(Some(fixture_cycle(Some(PublishMode::Auto), None)));
    auto.save_draft(&draft());
    std::fs::write(
        auto.meeting_dir().join("work/draft.new.json"),
        serde_json::to_vec(&draft().to_value()).unwrap(),
    )
    .unwrap();
    assert_eq!(auto.publishing.auto_publish(MEETING).unwrap(), 0);
    assert!(auto.ops().is_empty());
}

#[test]
fn the_tasks_of_a_cycle_come_from_the_last_read_of_its_issues() {
    let (rig, lab) = Rig::local(None);
    // An earlier meeting of the cycle, published into the clone.
    let published = lab.repo().root().join("meetings/NOMAD/2026-11-13");
    std::fs::create_dir_all(&published).unwrap();
    std::fs::write(
        published.join("meeting.json"),
        serde_json::to_vec_pretty(&draft().to_value()).unwrap(),
    )
    .unwrap();
    let cache = IssueCache {
        repo: "owner/rover".into(),
        label: "cycle:NOMAD".into(),
        fetched_unix_ms: Some(1_794_000_000_000),
        issues: vec![OpenIssue {
            number: 7,
            title: "Order the wheels".into(),
            assignee: Some("sam-gh".into()),
            url: Some("https://example.com/7".into()),
        }],
        closed: vec![5, 6],
        links: vec![IssueLink {
            number: 5,
            meeting: MEETING.into(),
            item: "i1".into(),
            open: false,
        }],
    };
    cache
        .save(&lab.repo().issue_cache_path("NOMAD").unwrap())
        .unwrap();

    let tasks = rig.publishing.cycle_tasks("NOMAD").unwrap();
    assert_eq!(tasks.as_of.as_deref(), Some("2026-11-06"));
    assert_eq!(tasks.open.len(), 1);
    assert_eq!(tasks.open[0].title, "Order the wheels");
    let done: Vec<_> = tasks
        .done
        .iter()
        .map(|t| (t.number, t.title.as_str()))
        .collect();
    assert_eq!(done, [(5, "Design the mount"), (6, "Issue #6")]);
    assert_eq!(tasks.done[0].meeting.as_deref(), Some(MEETING));
    assert_eq!(
        rig.publishing.cycle_tasks("NOPE").unwrap_err().code,
        "not_found"
    );
}

#[test]
fn the_environment_scrubs_every_secret_it_holds() {
    let (rig, _lab) = Rig::local(Some(fixture_cycle(
        Some(PublishMode::OnApproval),
        Some(("cred:discord/nomad", PublishMode::OnApproval)),
    )));
    let hook = "https://discord.com/api/webhooks/123456789012345678/abcDEF_123-xyz";
    rig.secrets
        .set(
            &crate::secrets::target_entry("cred:discord/nomad").unwrap(),
            hook,
        )
        .unwrap();
    rig.secrets
        .set(
            &crate::secrets::github_token_entry("https://api.github.com").unwrap(),
            "ghp_AppTokenValue0123456789",
        )
        .unwrap();
    let text = rig.publishing.env.scrub(&format!(
        "failed {hook} with ghp_AppTokenValue0123456789 and abcDEF_123-xyz"
    ));
    for secret in ["abcDEF_123-xyz", "ghp_AppTokenValue0123456789", hook] {
        assert!(!text.contains(secret), "{text}");
    }
}

// ----- a public repository, through the command layer -------------------------------------------

/// The repository of a publishers lab, registered as the application would, with a token.
fn gated(visibility: Visibility) -> (Rig, PublishLab) {
    let lab = PublishLab::new(LabOptions {
        visibility,
        discord: None,
        ..LabOptions::default()
    });
    let setting = CycleRepoSetting {
        id: "clone".into(),
        path: lab.clone_dir.to_string_lossy().into_owned(),
        remote: Some(lab.remote.to_string_lossy().into_owned()),
        kind: RepoKind::Clone,
        github_repo: None,
        github_api: None,
    };
    let mut repos = vec![setting];
    set_github_repo(
        &mut repos,
        "clone",
        Some(REPO),
        Some(&lab.github.api_base()),
    )
    .unwrap();
    let root = lab.tmp.path().join("recordings-app");
    let rig = Rig::new(root, repos, None);
    crate::publish_secrets::set_github_token(
        rig.secrets.as_ref(),
        Some(&lab.github.api_base()),
        TOKEN,
    )
    .unwrap();
    (rig, lab)
}

#[test]
fn a_public_repository_is_refused_until_the_user_confirms_it() {
    let (rig, lab) = gated(Visibility::Public);
    rig.save_draft(&draft());

    let plan = rig.publishing.plan(MEETING, &github_only()).unwrap();
    assert_eq!(plan.repo.visibility, "public");
    assert!(!plan.repo.confirmed);
    assert_eq!(plan.blocks[0].code, "public_repo_unconfirmed");

    let err = rig.publishing.publish(MEETING, &github_only()).unwrap_err();
    assert_eq!(err.code, "public_repo_unconfirmed");
    assert!(rig.ops().is_empty(), "nothing queued");
    assert!(
        !rig.meeting_dir().join("work/publish").exists(),
        "nothing frozen"
    );

    let confirm = rig.publishing.confirm_public_repo("NOMAD").unwrap();
    assert_eq!((confirm.visibility, confirm.released), ("public", 0));
    assert_eq!(confirm.key.as_deref(), Some("127.0.0.1/owner/rover"));
    let plan = rig.publishing.plan(MEETING, &github_only()).unwrap();
    assert!(
        plan.repo.confirmed && plan.blocks.is_empty(),
        "{:?}",
        plan.blocks
    );

    let published = rig.publishing.publish(MEETING, &github_only()).unwrap();
    assert_eq!(published.queued, 2, "the commit and the issue");
    assert_eq!(
        rig.tick().done,
        2,
        "{:?}",
        rig.publishing.state().unwrap().ops
    );
    assert_eq!(lab.github.issues(REPO).len(), 1);
    assert!(git(&lab.remote, &["ls-tree", "-r", "--name-only", "main"])
        .contains("meetings/NOMAD/2026-11-13/report.md"));
}

#[test]
fn the_public_repository_check_holds_when_the_user_switches_a_target_to_automatic() {
    let (rig, lab) = gated(Visibility::Public);
    rig.save_draft(&draft());
    let repos = || rig.repos.lock().unwrap().clone();
    // The default is on-approval: a completed draft publishes nothing by itself.
    assert_eq!(rig.publishing.auto_publish(MEETING).unwrap(), 0);

    crate::cycles::set_target_rule(&repos(), "NOMAD", "github", PublishMode::Auto).unwrap();
    let plan = rig.publishing.plan(MEETING, &github_only()).unwrap();
    assert_eq!(plan.targets[0].mode, Some("auto"));
    // Automatic, but the repository is public and nobody confirmed it: nothing is queued,
    // nothing is frozen, nothing leaves the machine.
    assert_eq!(rig.publishing.auto_publish(MEETING).unwrap(), 0);
    assert!(rig.ops().is_empty());
    assert!(!rig.meeting_dir().join("work/publish").exists());
    assert!(!git(&lab.remote, &["ls-tree", "-r", "--name-only", "main"]).contains("report.md"));
    // An explicit publication is refused as well.
    let err = rig.publishing.publish(MEETING, &github_only()).unwrap_err();
    assert_eq!(err.code, "public_repo_unconfirmed");

    // After the confirmation the rule works, and still queues no status change.
    rig.publishing.confirm_public_repo("NOMAD").unwrap();
    assert_eq!(rig.publishing.auto_publish(MEETING).unwrap(), 2);
    assert_eq!(rig.tick().done, 2);

    // Off is absolute, also for a click.
    let (off, _lab) = gated(Visibility::Private);
    off.save_draft(&draft());
    let repos = off.repos.lock().unwrap().clone();
    crate::cycles::set_target_rule(&repos, "NOMAD", "github", PublishMode::Off).unwrap();
    let err = off.publishing.publish(MEETING, &github_only()).unwrap_err();
    assert_eq!(err.code, "nothing_to_publish");
    assert!(off.ops().is_empty());
}

#[test]
fn a_private_repository_needs_no_confirmation() {
    let (rig, lab) = gated(Visibility::Private);
    rig.save_draft(&draft());
    let result = rig.publishing.publish(MEETING, &github_only()).unwrap();
    assert_eq!(result.plan.repo.visibility, "private");
    assert_eq!(rig.tick().done, 2);
    assert_eq!(lab.github.issues(REPO)[0].assignees, ["alex-gh"]);
    // The state the UI is told about holds no secret.
    let state = serde_json::to_string(&rig.publishing.state().unwrap()).unwrap();
    assert!(!state.contains(TOKEN), "{state}");
    assert_eq!(
        rig.ops()
            .iter()
            .filter(|o| o.state == OpState::Done)
            .count(),
        2
    );
    let _ = Repo::open(&lab.clone_dir).unwrap();
}
