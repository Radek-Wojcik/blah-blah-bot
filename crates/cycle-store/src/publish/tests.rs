//! Tests of the Git publisher against local bare repositories. No network is involved: a
//! remote is a folder, "offline" is a renamed folder and a colleague is a second clone.

use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::cycle::Cycle;
use crate::topics::TopicStatus;

// ----- fixtures ----------------------------------------------------------------------------------

/// Runs git in `dir` with a fixed identity and no signing.
fn g(dir: &Path, args: &[&str]) -> String {
    let mut full = vec![
        "-c",
        "user.name=Test User",
        "-c",
        "user.email=test@example.com",
        "-c",
        "commit.gpgsign=false",
    ];
    full.extend_from_slice(args);
    git::run_ok(Some(dir), full).unwrap_or_else(|e| panic!("git {args:?}: {e}"))
}

fn set_identity(dir: &Path) {
    g(dir, &["config", "user.name", "Test User"]);
    g(dir, &["config", "user.email", "test@example.com"]);
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn topic(id: &str, text: &str, status: TopicStatus) -> Topic {
    Topic {
        id: id.into(),
        text: text.into(),
        status,
        opened_in: "2026-11-06".into(),
        closed_in: (status == TopicStatus::Closed).then(|| "2026-11-13".to_string()),
    }
}

struct Lab {
    tmp: test_tmp::TestDir,
    remote: PathBuf,
}

impl Lab {
    /// A bare remote holding a cycle NOMAD with two topics, and nothing else.
    fn new() -> Lab {
        let tmp = test_tmp::TestDir::new("publish-tests");
        let remote = tmp.path().join("remote.git");
        fs::create_dir_all(&remote).unwrap();
        g(
            &remote,
            &["init", "--bare", "--quiet", "--initial-branch=main"],
        );
        let seed = Repo::init_local(tmp.path().join("seed")).unwrap();
        set_identity(seed.root());
        seed.create_cycle(&Cycle::new("NOMAD", "Nomad rover"))
            .unwrap();
        seed.save_topics(
            "NOMAD",
            &[
                topic("t-1", "Wheel size", TopicStatus::Open),
                topic("t-2", "Battery", TopicStatus::Open),
            ],
        )
        .unwrap();
        g(seed.root(), &["add", "-A"]);
        g(seed.root(), &["commit", "--quiet", "-m", "add NOMAD"]);
        g(
            seed.root(),
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        g(seed.root(), &["push", "--quiet", "origin", "main"]);
        Lab { tmp, remote }
    }

    fn clone(&self, name: &str) -> Repo {
        let repo = Repo::clone(self.remote.to_str().unwrap(), self.tmp.path().join(name)).unwrap();
        set_identity(repo.root());
        repo
    }

    fn tip(&self) -> String {
        g(&self.remote, &["rev-parse", "main"])
    }

    fn count(&self) -> usize {
        g(&self.remote, &["rev-list", "--count", "main"])
            .parse()
            .unwrap()
    }

    fn paths(&self) -> Vec<String> {
        g(&self.remote, &["ls-tree", "-r", "--name-only", "main"])
            .lines()
            .map(String::from)
            .collect()
    }

    fn show(&self, path: &str) -> String {
        g(&self.remote, &["show", &format!("main:{path}")])
    }

    /// A colleague pushes a commit that adds `files`.
    fn colleague(&self, name: &str, files: &[(&str, &str)]) -> String {
        let dir = self.tmp.path().join(format!("colleague-{name}"));
        let repo = Repo::clone(self.remote.to_str().unwrap(), &dir).unwrap();
        set_identity(repo.root());
        for (path, text) in files {
            write(&dir.join(path), text);
        }
        g(&dir, &["add", "-A"]);
        g(
            &dir,
            &["commit", "--quiet", "-m", &format!("colleague {name}")],
        );
        g(&dir, &["push", "--quiet", "origin", "main"]);
        g(&dir, &["rev-parse", "HEAD"])
    }
}

/// A snapshot folder with the files of a meeting.
struct Snapshot {
    files: Vec<PublishFile>,
}

fn snapshot(root: &Path, name: &str, marker: &str) -> Snapshot {
    let dir = root.join(format!("snap-{name}"));
    write(
        &dir.join("meeting.json"),
        &format!("{{\"meeting\": \"{marker}\"}}\n"),
    );
    write(&dir.join("report.md"), &format!("# Report {marker}\n"));
    write(
        &dir.join("transcript.md"),
        &format!("# Transcript {marker}\n"),
    );
    fs::create_dir_all(dir.join("img")).unwrap();
    fs::write(
        dir.join("img/shot-1.png"),
        [0x89, b'P', b'N', b'G', 0, 1, 2, 3, 13, 10, 26, 10],
    )
    .unwrap();
    let rels = [
        "meeting.json",
        "report.md",
        "transcript.md",
        "img/shot-1.png",
    ];
    Snapshot {
        files: rels
            .iter()
            .map(|rel| PublishFile {
                rel: (*rel).to_string(),
                path: dir.join(rel),
            })
            .collect(),
    }
}

fn request<'a>(files: &'a [PublishFile], topics: Option<&'a [Topic]>) -> PublishRequest<'a> {
    PublishRequest {
        cycle_id: "NOMAD",
        base_path: "meetings",
        date: "2026-11-13",
        files,
        topics,
        message: "Add meeting {folder}",
        auth: None,
    }
}

fn status(repo: &Repo) -> String {
    g(repo.root(), &["status", "--porcelain=v1"])
}

// ----- the commit ------------------------------------------------------------------------------

#[test]
fn only_the_new_files_are_committed_and_the_working_tree_is_left_alone() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    let old_tip = lab.tip();
    // Unfinished work of the user: an edited cycle file and an untracked note.
    let mut cycle = repo.load_cycle("NOMAD").unwrap();
    cycle.name = "Edited, not committed".into();
    repo.save_cycle(&cycle).unwrap();
    write(&repo.root().join("notes.txt"), "mine\n");
    let before = status(&repo);
    assert!(
        before.contains("cycle.yaml") && before.contains("?? notes.txt"),
        "{before}"
    );

    let snap = snapshot(lab.tmp.path(), r"ab", "A");
    let published = publish_meeting(&repo, &request(&snap.files, None)).unwrap();

    assert_eq!(published.folder, "2026-11-13");
    assert!(published.pushed && !published.already_published);
    assert_eq!(published.attempts, 1);
    assert_eq!(published.remote.as_deref(), Some("origin"));
    assert_eq!(published.branch, "main");
    assert_eq!(published.commit, lab.tip());
    assert_eq!(published.local, LocalSync::Updated);
    assert_eq!(published.topics, None);
    let new_paths = [
        "meetings/NOMAD/2026-11-13/img/shot-1.png",
        "meetings/NOMAD/2026-11-13/meeting.json",
        "meetings/NOMAD/2026-11-13/report.md",
        "meetings/NOMAD/2026-11-13/transcript.md",
    ];
    assert_eq!(published.paths.len(), 4);
    for p in new_paths {
        assert!(published.paths.contains(&p.to_string()), "{p}");
    }

    // The remote gained exactly these files, in one commit on top of the old tip.
    let changes = g(&lab.remote, &["diff", "--name-status", &old_tip, "main"]);
    let mut added: Vec<&str> = changes.lines().collect();
    added.sort_unstable();
    assert_eq!(
        added,
        new_paths
            .iter()
            .map(|p| format!("A\t{p}"))
            .collect::<Vec<_>>()
    );
    assert_eq!(g(&lab.remote, &["rev-parse", "main^"]), old_tip);
    assert_eq!(
        g(&lab.remote, &["log", "-1", "--format=%s", "main"]),
        "Add meeting 2026-11-13"
    );
    assert_eq!(
        lab.show("meetings/NOMAD/2026-11-13/report.md"),
        "# Report A"
    );

    // The clone moved to the commit; the unfinished work is exactly as it was.
    assert_eq!(g(repo.root(), &["rev-parse", "HEAD"]), published.commit);
    assert_eq!(status(&repo), before);
    assert_eq!(
        repo.load_cycle("NOMAD").unwrap().name,
        "Edited, not committed"
    );
    assert_eq!(
        fs::read_to_string(repo.root().join("notes.txt")).unwrap(),
        "mine\n"
    );
    assert!(g(repo.root(), &["diff", "--cached", "--name-only"]).is_empty());
    assert!(repo
        .root()
        .join("meetings/NOMAD/2026-11-13/img/shot-1.png")
        .is_file());
    assert_eq!(
        g(repo.root(), &["rev-parse", "refs/remotes/origin/main"]),
        published.commit
    );
    let leftovers: Vec<_> = fs::read_dir(repo.git_dir().unwrap())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("bbb-publish"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    // Binary files are stored as they are.
    let png = fs::read(&snap.files[3].path).unwrap();
    let stored = std::process::Command::new("git")
        .args([
            "-C",
            lab.remote.to_str().unwrap(),
            "show",
            "main:meetings/NOMAD/2026-11-13/img/shot-1.png",
        ])
        .output()
        .unwrap()
        .stdout;
    assert_eq!(stored, png);
}

#[test]
fn a_second_meeting_on_the_same_day_gets_a_suffix() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    let first = snapshot(lab.tmp.path(), "1", "ONE");
    let second = snapshot(lab.tmp.path(), "2", "TWO");
    let third = snapshot(lab.tmp.path(), "3", "THREE");
    let a = publish_meeting(&repo, &request(&first.files, None)).unwrap();
    let b = publish_meeting(&repo, &request(&second.files, None)).unwrap();
    let c = publish_meeting(&repo, &request(&third.files, None)).unwrap();
    assert_eq!(
        [a.folder.as_str(), b.folder.as_str(), c.folder.as_str()],
        ["2026-11-13", "2026-11-13-2", "2026-11-13-3"]
    );
    assert_eq!(
        lab.show("meetings/NOMAD/2026-11-13-2/report.md"),
        "# Report TWO"
    );
    assert_eq!(
        lab.show("meetings/NOMAD/2026-11-13-3/report.md"),
        "# Report THREE"
    );
    assert_eq!(
        lab.show("meetings/NOMAD/2026-11-13/report.md"),
        "# Report ONE"
    );
    assert_eq!(c.local, LocalSync::Updated);
    assert_eq!(lab.count(), 4);
}

#[test]
fn the_meeting_folder_goes_under_the_configured_path() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    let first = snapshot(lab.tmp.path(), "1", "ONE");
    let second = snapshot(lab.tmp.path(), "2", "TWO");
    let mut req = request(&first.files, None);
    req.base_path = "docs/team-notes";
    let a = publish_meeting(&repo, &req).unwrap();
    assert_eq!(a.folder, "2026-11-13");
    assert_eq!(
        a.paths,
        [
            "docs/team-notes/NOMAD/2026-11-13/meeting.json",
            "docs/team-notes/NOMAD/2026-11-13/report.md",
            "docs/team-notes/NOMAD/2026-11-13/transcript.md",
            "docs/team-notes/NOMAD/2026-11-13/img/shot-1.png",
        ]
    );
    assert_eq!(
        lab.show("docs/team-notes/NOMAD/2026-11-13/report.md"),
        "# Report ONE"
    );
    assert!(
        lab.paths().iter().all(|p| !p.starts_with("meetings/")),
        "nothing went to the default folder: {:?}",
        lab.paths()
    );
    // The suffix is chosen among the folders of that path, and publishing again is idempotent.
    let mut req2 = request(&second.files, None);
    req2.base_path = "docs/team-notes";
    let b = publish_meeting(&repo, &req2).unwrap();
    assert_eq!(b.folder, "2026-11-13-2");
    let again = publish_meeting(&repo, &req2).unwrap();
    assert!(again.already_published, "{again:?}");
    assert_eq!(again.folder, "2026-11-13-2");
    // The same day in the default folder does not collide with the other path.
    let third = snapshot(lab.tmp.path(), "3", "THREE");
    let c = publish_meeting(&repo, &request(&third.files, None)).unwrap();
    assert_eq!(c.folder, "2026-11-13");
    assert_eq!(
        lab.show("meetings/NOMAD/2026-11-13/report.md"),
        "# Report THREE"
    );
}

#[test]
fn a_path_that_could_leave_the_repository_is_refused_before_git_is_touched() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    let snap = snapshot(lab.tmp.path(), r"ab", "A");
    let tip = lab.tip();
    for bad in [
        "",
        "..",
        "../outside",
        "a/../b",
        "./meetings",
        "a/./b",
        "/etc",
        r"\share",
        "C:/temp",
        "c:meetings",
        r"a\b",
        "a//b",
        "meetings/",
        ".git",
        "x/.GIT/y",
        "trailing.",
        "sp ace",
        "naïve",
    ] {
        let mut req = request(&snap.files, None);
        req.base_path = bad;
        let err = publish_meeting(&repo, &req).unwrap_err();
        assert!(!err.is_transient(), "{bad:?}: {err}");
        assert!(
            err.message.contains("not usable"),
            "{bad:?}: {}",
            err.message
        );
    }
    for good in ["meetings", "docs/meetings", ".github/notes", "a_b-c/d.e"] {
        assert_eq!(crate::cycle::repo_path_problem(good), None, "{good}");
    }
    assert_eq!(lab.tip(), tip, "the remote was never touched");
}

#[test]
fn publishing_twice_is_idempotent_even_after_a_lost_answer() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    let snap = snapshot(lab.tmp.path(), r"ab", "A");
    let start = g(repo.root(), &["rev-parse", "HEAD"]);
    let first = publish_meeting(&repo, &request(&snap.files, None)).unwrap();
    let count = lab.count();

    let again = publish_meeting(&repo, &request(&snap.files, None)).unwrap();
    assert!(again.already_published && again.pushed);
    assert_eq!(again.folder, first.folder);
    assert_eq!(again.commit, first.commit);
    assert_eq!(lab.count(), count, "no second commit");

    // The answer of the push was lost: this clone never learned about it.
    g(repo.root(), &["reset", "--hard", "--quiet", &start]);
    g(
        repo.root(),
        &["update-ref", "refs/remotes/origin/main", &start],
    );
    let retry = publish_meeting(&repo, &request(&snap.files, None)).unwrap();
    assert!(retry.already_published);
    assert_eq!(retry.commit, first.commit);
    assert_eq!(retry.folder, "2026-11-13");
    assert_eq!(retry.local, LocalSync::Updated);
    assert_eq!(lab.count(), count);
    assert_eq!(g(repo.root(), &["rev-parse", "HEAD"]), first.commit);
}

// ----- races -------------------------------------------------------------------------------------

#[test]
fn a_push_refused_because_of_a_concurrent_commit_is_rebuilt_on_the_new_tip() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    let snap = snapshot(lab.tmp.path(), r"ab", "A");
    let mut colleague_commit = String::new();
    let published = publish_meeting_observed(&repo, &request(&snap.files, None), &mut |attempt| {
        if attempt == 1 {
            colleague_commit = lab.colleague("x", &[("docs/colleague.md", "hello\n")]);
        }
    })
    .unwrap();
    assert_eq!(published.attempts, 2);
    assert_eq!(published.folder, "2026-11-13");
    // History is linear: seed, the colleague's commit, ours; both contributions are there.
    assert_eq!(lab.count(), 3);
    assert_eq!(g(&lab.remote, &["rev-parse", "main^"]), colleague_commit);
    assert_eq!(lab.show("docs/colleague.md"), "hello");
    assert_eq!(
        lab.show("meetings/NOMAD/2026-11-13/report.md"),
        "# Report A"
    );
    assert_eq!(published.commit, lab.tip());
    assert_eq!(g(repo.root(), &["rev-parse", "HEAD"]), published.commit);
    assert_eq!(published.local, LocalSync::Updated);
}

#[test]
fn a_colleague_taking_the_folder_meanwhile_moves_us_to_the_next_suffix() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    let snap = snapshot(lab.tmp.path(), r"ab", "A");
    let published = publish_meeting_observed(&repo, &request(&snap.files, None), &mut |attempt| {
        if attempt == 1 {
            lab.colleague(
                "y",
                &[(
                    "meetings/NOMAD/2026-11-13/meeting.json",
                    "{\"meeting\": \"THEIRS\"}\n",
                )],
            );
        }
    })
    .unwrap();
    assert_eq!(published.attempts, 2);
    assert_eq!(published.folder, "2026-11-13-2");
    assert_eq!(
        lab.show("meetings/NOMAD/2026-11-13/meeting.json"),
        "{\"meeting\": \"THEIRS\"}"
    );
    assert_eq!(
        lab.show("meetings/NOMAD/2026-11-13-2/meeting.json"),
        "{\"meeting\": \"A\"}"
    );
}

#[test]
fn the_push_is_tried_three_times_and_then_left_for_later() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    let snap = snapshot(lab.tmp.path(), r"ab", "A");
    let head = g(repo.root(), &["rev-parse", "HEAD"]);
    let mut calls = 0;
    let err = publish_meeting_observed(&repo, &request(&snap.files, None), &mut |attempt| {
        calls += 1;
        lab.colleague(
            &format!("busy{attempt}"),
            &[(&format!("docs/{attempt}.md"), "x\n")],
        );
    })
    .unwrap_err();
    assert_eq!(calls, MAX_ATTEMPTS);
    assert!(err.is_transient(), "{err}");
    assert!(!lab.paths().iter().any(|p| p.starts_with("meetings/")));
    // Nothing changed locally either.
    assert_eq!(g(repo.root(), &["rev-parse", "HEAD"]), head);
    assert!(status(&repo).is_empty());

    // Quiet again: the next run goes through.
    let published = publish_meeting(&repo, &request(&snap.files, None)).unwrap();
    assert_eq!(published.attempts, 1);
    assert!(lab
        .paths()
        .iter()
        .any(|p| p.starts_with("meetings/NOMAD/2026-11-13/")));
}

// ----- topics ---------------------------------------------------------------------------------

#[test]
fn topics_are_merged_per_topic_and_the_dirty_topics_file_still_fast_forwards() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    // Locally (uncommitted): t-1 is closed and the meeting adds a topic.
    let ours = vec![
        topic("t-1", "Wheel size", TopicStatus::Closed),
        topic("t-2", "Battery", TopicStatus::Open),
        topic("20261113-1", "Charging", TopicStatus::Open),
    ];
    repo.save_topics("NOMAD", &ours).unwrap();
    // Meanwhile a colleague reworded t-1 and closed t-2, and pushed.
    let theirs = vec![
        topic("t-1", "Wheel size (rework)", TopicStatus::Open),
        topic("t-2", "Battery", TopicStatus::Closed),
    ];
    {
        let dir = lab.tmp.path().join("colleague-topics");
        let c = Repo::clone(lab.remote.to_str().unwrap(), &dir).unwrap();
        set_identity(c.root());
        c.save_topics("NOMAD", &theirs).unwrap();
        g(&dir, &["commit", "--quiet", "-am", "colleague topics"]);
        g(&dir, &["push", "--quiet", "origin", "main"]);
    }
    let snap = snapshot(lab.tmp.path(), r"ab", "A");
    let published = publish_meeting(&repo, &request(&snap.files, Some(&ours))).unwrap();

    let merge = published.topics.clone().expect("a topic merge");
    // Ours wins for t-1; the colleague's t-2 stays; our new topic follows.
    let result = topics::parse_topics(&lab.show("cycles/NOMAD/topics.yaml")).unwrap();
    assert_eq!(result, merge.topics);
    assert_eq!(result[0].id, "t-1");
    assert_eq!(result[0].text, "Wheel size");
    assert_eq!(result[0].status, TopicStatus::Closed);
    assert_eq!(
        result[1].status,
        TopicStatus::Closed,
        "the colleague's closure is kept"
    );
    assert_eq!(result[2].id, "20261113-1");
    // What was overridden is reported, and the diff shows what the commit changes.
    assert_eq!(merge.conflicts.len(), 1);
    assert_eq!(merge.conflicts[0].id, "t-1");
    assert_eq!(
        merge.conflicts[0].theirs.as_ref().unwrap().text,
        "Wheel size (rework)"
    );
    assert_eq!(merge.diff.added.len(), 1);
    assert_eq!(merge.diff.changed.len(), 1);
    assert_eq!(merge.diff.changed[0].id, "t-1");

    // The clone caught up although its topics.yaml was edited: no uncommitted leftovers.
    assert_eq!(published.local, LocalSync::Updated);
    assert_eq!(g(repo.root(), &["rev-parse", "HEAD"]), published.commit);
    assert_eq!(repo.load_topics("NOMAD").unwrap(), merge.topics);
    assert!(status(&repo).is_empty(), "{}", status(&repo));
}

#[test]
fn topics_that_nobody_changed_leave_the_file_out_of_the_commit() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    let same = repo.load_topics("NOMAD").unwrap();
    let snap = snapshot(lab.tmp.path(), r"ab", "A");
    let old_tip = lab.tip();
    let published = publish_meeting(&repo, &request(&snap.files, Some(&same))).unwrap();
    let merge = published.topics.unwrap();
    assert!(merge.diff.is_empty() && merge.conflicts.is_empty());
    let changed = g(&lab.remote, &["diff", "--name-only", &old_tip, "main"]);
    assert!(!changed.contains("topics.yaml"), "{changed}");
}

#[test]
fn the_preview_shows_the_merge_without_changing_anything() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    let mut ours = repo.load_topics("NOMAD").unwrap();
    ours.push(topic("20261113-1", "Charging", TopicStatus::Open));
    ours[0].status = TopicStatus::Closed;
    ours[0].closed_in = Some("2026-11-13".into());
    let before = g(repo.root(), &["rev-parse", "HEAD"]);
    let preview = preview_topics(&repo, "NOMAD", &ours).unwrap();
    assert!(
        preview.against_remote,
        "a clone has a remote-tracking branch"
    );
    let merge = preview.merge;
    assert_eq!(merge.diff.added.len(), 1);
    assert_eq!(merge.diff.changed.len(), 1);
    assert!(merge.conflicts.is_empty());
    assert_eq!(g(repo.root(), &["rev-parse", "HEAD"]), before);
    assert!(preview_topics(&repo, "../x", &ours).is_err());

    // A remote that was never fetched into this clone: the comparison is with the clone's own
    // branch, and the preview says so.
    g(
        repo.root(),
        &["update-ref", "-d", "refs/remotes/origin/main"],
    );
    let preview = preview_topics(&repo, "NOMAD", &ours).unwrap();
    assert!(!preview.against_remote);
    assert_eq!(preview.merge.diff.added.len(), 1);
}

// ----- other repositories --------------------------------------------------------------------

#[test]
fn a_repository_without_a_remote_gets_a_local_commit() {
    let tmp = test_tmp::TestDir::new("publish-tests");
    let repo = Repo::init_local(tmp.path().join("local")).unwrap();
    set_identity(repo.root());
    repo.create_cycle(&Cycle::new("NOMAD", "Nomad rover"))
        .unwrap();
    let ours = vec![topic("20261113-1", "Charging", TopicStatus::Open)];
    repo.save_topics("NOMAD", &ours).unwrap();
    let snap = snapshot(tmp.path(), r"ab", "A");

    let first = publish_meeting(&repo, &request(&snap.files, Some(&ours))).unwrap();
    assert!(!first.pushed && first.remote.is_none());
    assert_eq!(first.local, LocalSync::Updated);
    assert_eq!(g(repo.root(), &["rev-parse", "HEAD"]), first.commit);
    let tracked = g(repo.root(), &["ls-files"]);
    assert!(
        tracked.contains("meetings/NOMAD/2026-11-13/report.md"),
        "{tracked}"
    );
    assert!(tracked.contains("cycles/NOMAD/topics.yaml"));
    // Other cycle files were not committed (they are not the publisher's to commit).
    assert!(!tracked.contains("cycle.yaml"), "{tracked}");
    assert!(repo
        .root()
        .join("meetings/NOMAD/2026-11-13/transcript.md")
        .is_file());
    assert!(
        status(&repo).lines().all(|l| l.starts_with("??")),
        "{}",
        status(&repo)
    );

    // A second meeting builds on the first.
    let second = snapshot(tmp.path(), "b", "B");
    let next = publish_meeting(&repo, &request(&second.files, Some(&ours))).unwrap();
    assert_eq!(next.folder, "2026-11-13-2");
    assert_eq!(g(repo.root(), &["rev-parse", "HEAD^"]), first.commit);
    let again = publish_meeting(&repo, &request(&second.files, Some(&ours))).unwrap();
    assert!(again.already_published);
    assert_eq!(again.commit, next.commit);
}

#[test]
fn an_empty_remote_gets_its_first_branch_from_the_local_history() {
    let tmp = test_tmp::TestDir::new("publish-tests");
    let remote = tmp.path().join("empty.git");
    fs::create_dir_all(&remote).unwrap();
    g(
        &remote,
        &["init", "--bare", "--quiet", "--initial-branch=main"],
    );
    let repo = Repo::init_local(tmp.path().join("local")).unwrap();
    set_identity(repo.root());
    repo.create_cycle(&Cycle::new("NOMAD", "Nomad rover"))
        .unwrap();
    g(repo.root(), &["add", "-A"]);
    g(repo.root(), &["commit", "--quiet", "-m", "start"]);
    g(
        repo.root(),
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    let snap = snapshot(tmp.path(), r"ab", "A");
    let published = publish_meeting(&repo, &request(&snap.files, None)).unwrap();
    assert!(published.pushed);
    assert_eq!(g(&remote, &["rev-parse", "main"]), published.commit);
    assert_eq!(g(&remote, &["rev-list", "--count", "main"]), "2");
    assert_eq!(published.local, LocalSync::Updated);
}

#[test]
fn an_unreachable_remote_is_a_transient_error_and_recovery_works() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    let snap = snapshot(lab.tmp.path(), r"ab", "A");
    let head = g(repo.root(), &["rev-parse", "HEAD"]);
    let away = lab.tmp.path().join("away.git");
    fs::rename(&lab.remote, &away).unwrap();
    let err = publish_meeting(&repo, &request(&snap.files, None)).unwrap_err();
    assert!(err.is_transient(), "{err}");
    assert_eq!(
        g(repo.root(), &["rev-parse", "HEAD"]),
        head,
        "nothing was committed locally"
    );
    assert!(!repo.root().join("meetings").exists());

    fs::rename(&away, &lab.remote).unwrap();
    let published = publish_meeting(&repo, &request(&snap.files, None)).unwrap();
    assert!(published.pushed);
    assert_eq!(published.commit, lab.tip());
}

#[test]
fn local_commits_that_are_not_pushed_stay_and_the_clone_is_reported_behind() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    write(&repo.root().join("docs/local.md"), "local\n");
    g(repo.root(), &["add", "-A"]);
    g(repo.root(), &["commit", "--quiet", "-m", "local only"]);
    let local_head = g(repo.root(), &["rev-parse", "HEAD"]);
    let snap = snapshot(lab.tmp.path(), r"ab", "A");
    let published = publish_meeting(&repo, &request(&snap.files, None)).unwrap();
    assert!(published.pushed);
    assert!(
        matches!(published.local, LocalSync::Behind(_)),
        "{:?}",
        published.local
    );
    assert_eq!(g(repo.root(), &["rev-parse", "HEAD"]), local_head);
    assert!(!lab.paths().contains(&"docs/local.md".to_string()));
    // A pull puts the clone right: the local commit is rebased on top.
    let report = repo.pull_rebase().unwrap();
    assert_eq!(report.outcome, crate::PullOutcome::Updated);
    assert!(repo
        .root()
        .join("meetings/NOMAD/2026-11-13/report.md")
        .is_file());
}

#[test]
fn a_local_edit_in_the_way_keeps_the_branch_and_loses_nothing() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    lab.colleague(
        "z",
        &[("cycles/NOMAD/templates/default.md", "# changed upstream\n")],
    );
    // The same file is edited here and not committed.
    write(
        &repo.root().join("cycles/NOMAD/templates/default.md"),
        "# my local edit\n",
    );
    let snap = snapshot(lab.tmp.path(), r"ab", "A");
    let head = g(repo.root(), &["rev-parse", "HEAD"]);
    let published = publish_meeting(&repo, &request(&snap.files, None)).unwrap();
    assert!(published.pushed);
    let LocalSync::Behind(reason) = &published.local else {
        panic!("{:?}", published.local)
    };
    assert!(reason.contains("in the way"), "{reason}");
    assert_eq!(g(repo.root(), &["rev-parse", "HEAD"]), head);
    assert_eq!(
        fs::read_to_string(repo.root().join("cycles/NOMAD/templates/default.md")).unwrap(),
        "# my local edit\n"
    );
    assert_eq!(
        lab.show("meetings/NOMAD/2026-11-13/report.md"),
        "# Report A"
    );
}

// ----- refusals ------------------------------------------------------------------------------------

#[test]
fn bad_requests_are_refused_before_git_is_touched() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    let snap = snapshot(lab.tmp.path(), r"ab", "A");
    let tip = lab.tip();
    let refused = |req: &PublishRequest| {
        let err = publish_meeting(&repo, req).unwrap_err();
        assert!(!err.is_transient(), "{err}");
        err.message
    };

    let mut r = request(&snap.files, None);
    r.date = "13-11-2026";
    assert!(refused(&r).contains("YYYY-MM-DD"));
    let mut r = request(&snap.files, None);
    r.cycle_id = "../x";
    assert!(refused(&r).contains("project id"));
    assert!(refused(&request(&[], None)).contains("nothing to publish"));

    for rel in [
        "../escape.md",
        "a//b.md",
        "/abs.md",
        "sp ace.md",
        "",
        "x\\y.md",
        ".",
    ] {
        let files = vec![PublishFile {
            rel: rel.into(),
            path: snap.files[0].path.clone(),
        }];
        assert!(
            refused(&request(&files, None)).contains("not allowed"),
            "{rel:?}"
        );
    }
    let dup = vec![snap.files[0].clone(), snap.files[0].clone()];
    assert!(refused(&request(&dup, None)).contains("twice"));
    let missing = vec![PublishFile {
        rel: "report.md".into(),
        path: lab.tmp.path().join("nope.md"),
    }];
    assert!(refused(&request(&missing, None)).contains("missing"));

    let not_git = test_tmp::TestDir::new("publish-tests");
    let plain = Repo::open(not_git.path()).unwrap();
    assert!(publish_meeting(&plain, &request(&snap.files, None))
        .unwrap_err()
        .message
        .contains("not a Git repository"));

    g(repo.root(), &["checkout", "--quiet", "--detach"]);
    assert!(refused(&request(&snap.files, None)).contains("detached"));
    assert_eq!(lab.tip(), tip, "the remote was never touched");
}

#[test]
fn a_broken_topics_file_on_the_remote_is_a_permanent_error() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    lab.colleague("t", &[("cycles/NOMAD/topics.yaml", "- id: [\n")]);
    let snap = snapshot(lab.tmp.path(), r"ab", "A");
    let ours = vec![topic("n", "New", TopicStatus::Open)];
    let err = publish_meeting(&repo, &request(&snap.files, Some(&ours))).unwrap_err();
    assert!(!err.is_transient());
    assert!(err.message.contains("topics.yaml on the remote"), "{err}");
}

// ----- credentials and classification ----------------------------------------------------------

#[test]
fn base64_matches_the_standard_vectors() {
    for (plain, coded) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
        ("x-access-token:abc", "eC1hY2Nlc3MtdG9rZW46YWJj"),
    ] {
        assert_eq!(base64(plain.as_bytes()), coded, "{plain:?}");
    }
}

#[test]
fn the_token_goes_into_the_environment_only_for_its_own_remote() {
    let auth = GitAuth {
        base_url: "https://github.com/".into(),
        token: "ghp_SecretTokenValue".into(),
    };
    let env = auth_env("https://github.com/owner/rover.git", &auth);
    let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    assert_eq!(get("GIT_CONFIG_COUNT"), Some("2"));
    assert_eq!(
        get("GIT_CONFIG_KEY_0"),
        Some("http.https://github.com/.extraheader")
    );
    assert_eq!(
        get("GIT_CONFIG_VALUE_0"),
        Some(
            format!(
                "AUTHORIZATION: basic {}",
                base64(b"x-access-token:ghp_SecretTokenValue")
            )
            .as_str()
        )
    );
    assert_eq!(get("GIT_CONFIG_KEY_1"), Some("http.followRedirects"));
    assert_eq!(get("GIT_CONFIG_VALUE_1"), Some("false"));
    // A look-alike host, a plain path, ssh and another server never get it.
    for url in [
        "https://github.com.evil.example/owner/rover.git",
        "https://gitlab.com/owner/rover.git",
        "C:/repos/rover.git",
        "git@github.com:owner/rover.git",
        "http://github.com/owner/rover.git",
    ] {
        assert!(auth_env(url, &auth).is_empty(), "{url}");
    }
    // A base without a trailing slash is the same base; an http base is never used.
    let bare = GitAuth {
        base_url: "https://ghe.example.com".into(),
        token: "t".into(),
    };
    assert!(!auth_env("https://ghe.example.com/team/rover.git", &bare).is_empty());
    let plain = GitAuth {
        base_url: "http://ghe.example.com/".into(),
        token: "t".into(),
    };
    assert!(auth_env("http://ghe.example.com/team/rover.git", &plain).is_empty());
    // The command lines never carry a token: they take none.
    for arg in fetch_args("origin", "main")
        .iter()
        .chain(&push_args("origin", "abc", "main"))
    {
        assert!(!arg.contains("ghp_"), "{arg}");
    }
    assert!(!format!("{auth:?}").contains("ghp_SecretTokenValue"));
}

#[test]
fn git_output_is_scrubbed_of_the_token_and_of_url_credentials() {
    let token = "ghp_SecretTokenValue";
    let header = base64(format!("x-access-token:{token}").as_bytes());
    let text = format!(
        "fatal: unable to access 'https://x-access-token:{token}@github.com/o/r.git/': error\n\
         trace: AUTHORIZATION: basic {header}\n\
         remote: see https://user:pw@example.com/x and plain {token} and {header}"
    );
    let clean = scrub(&text, &[token]);
    for secret in [token, header.as_str(), "user:pw", "x-access-token:ghp"] {
        assert!(!clean.contains(secret), "{secret}: {clean}");
    }
    assert!(
        clean.contains("unable to access") && clean.contains("github.com/o/r.git"),
        "{clean}"
    );
    assert!(clean.contains("https://***@example.com/x"));

    // The same through a failing command of the publisher.
    let git = Git {
        root: Path::new("."),
        env: Vec::new(),
        auth_env: Vec::new(),
        secrets: vec![token.to_string()],
    };
    let out = Output {
        ok: false,
        stdout: String::new(),
        stderr: format!("fatal: Authentication failed for 'https://x:{token}@github.com/o/r.git/'"),
    };
    let err = git.fail("push", &out);
    assert!(!err.message.contains(token), "{err}");
    assert_eq!(err.kind, PublishErrorKind::Permanent);
}

#[test]
fn failures_are_classified_from_git_text() {
    use PublishErrorKind::{Permanent, Transient};
    for (text, kind) in [
        ("fatal: unable to access 'https://x/': Could not resolve host: github.com", Transient),
        ("fatal: unable to access 'https://x/': Failed to connect to github.com port 443", Transient),
        ("fatal: unable to access 'https://x/': Operation timed out", Transient),
        ("fatal: unable to access 'https://x/': The requested URL returned error: 503", Transient),
        ("fatal: '/srv/remote.git' does not appear to be a git repository\nfatal: Could not read from remote repository.", Transient),
        ("error: RPC failed; curl 56 Recv failure: Connection was reset", Transient),
        ("fatal: Authentication failed for 'https://github.com/o/r.git/'", Permanent),
        ("fatal: could not read Username for 'https://github.com': terminal prompts disabled", Permanent),
        ("remote: Permission to o/r.git denied to someone.\nfatal: unable to access: The requested URL returned error: 403", Permanent),
        ("git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.", Permanent),
        ("remote: error: GH006: Protected branch update failed for refs/heads/main.", Permanent),
        ("remote: Repository not found.", Permanent),
        ("something nobody has seen before", Permanent),
    ] {
        assert_eq!(classify_git_failure(text), kind, "{text}");
    }
    assert!(rejected_by_remote_race(
        " ! [rejected]        abc -> main (fetch first)"
    ));
    assert!(rejected_by_remote_race(
        " ! [rejected]        abc -> main (non-fast-forward)"
    ));
    assert!(!rejected_by_remote_race(
        "remote: error: GH006: Protected branch update failed"
    ));
}

// ----- a cycle that was only ever created in the clone ----------------------------------------

#[test]
fn a_cycle_created_in_the_clone_and_never_committed_is_published_and_the_clone_catches_up() {
    let tmp = test_tmp::TestDir::new("publish-tests");
    let remote = tmp.path().join("remote.git");
    fs::create_dir_all(&remote).unwrap();
    g(
        &remote,
        &["init", "--bare", "--quiet", "--initial-branch=main"],
    );
    // A repository that has a README and no cycle at all, as a fresh test repository.
    let seed = Repo::init_local(tmp.path().join("seed")).unwrap();
    set_identity(seed.root());
    write(&seed.root().join("README.md"), "# test\n");
    g(seed.root(), &["add", "-A"]);
    g(seed.root(), &["commit", "--quiet", "-m", "init"]);
    g(
        seed.root(),
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    g(seed.root(), &["push", "--quiet", "origin", "main"]);

    let repo = Repo::clone(remote.to_str().unwrap(), tmp.path().join("clone")).unwrap();
    set_identity(repo.root());
    // The cycle exists only here: cycle.yaml, an empty topics.yaml and the template.
    repo.create_cycle(&Cycle::new("NOMAD", "Nomad rover"))
        .unwrap();
    assert!(status(&repo).contains("?? cycles/"), "{}", status(&repo));

    // The first meeting adds no topic: topics.yaml stays out of the commit.
    let first = snapshot(tmp.path(), "first", "A");
    let mut req = request(&first.files, None);
    req.date = "2026-11-10";
    let one = publish_meeting(&repo, &req).unwrap();
    assert_eq!(one.local, LocalSync::Updated, "{one:?}");
    assert!(repo
        .root()
        .join("meetings/NOMAD/2026-11-10/report.md")
        .is_file());

    // The second one adds a topic: the file is committed and the clone still catches up.
    // The file here still has none of it: the application writes the topics only through
    // the commit.
    let ours = vec![topic("20261113-1", "Charging", TopicStatus::Open)];
    assert!(repo.load_topics("NOMAD").unwrap().is_empty());
    let second = snapshot(tmp.path(), "second", "B");
    let two = publish_meeting(&repo, &request(&second.files, Some(&ours))).unwrap();
    assert_eq!(two.local, LocalSync::Updated, "{two:?}");
    assert_eq!(g(repo.root(), &["rev-parse", "HEAD"]), two.commit);
    assert!(repo
        .root()
        .join("meetings/NOMAD/2026-11-13/report.md")
        .is_file());
    assert_eq!(repo.load_topics("NOMAD").unwrap(), ours);
    let after = status(&repo);
    assert!(
        after.lines().all(|l| l.starts_with("??")) && !after.contains("topics.yaml"),
        "only the cycle files that were never committed remain: {after}"
    );
}

// ----- adding a file to a published meeting -------------------------------------------------------

fn note(root: &Path, name: &str, text: &str) -> PathBuf {
    let path = root.join(name);
    write(&path, text);
    path
}

fn add_request<'a>(path: &'a Path) -> AddFileRequest<'a> {
    AddFileRequest {
        cycle_id: "NOMAD",
        base_path: "meetings",
        folder: "2026-11-13",
        stem: "corrections",
        ext: "md",
        path,
        message: "Add {file} to the meeting",
        auth: None,
    }
}

#[test]
fn a_file_is_added_next_to_the_meeting_and_nothing_else_changes() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    let snap = snapshot(lab.tmp.path(), "ab", "A");
    publish_meeting(&repo, &request(&snap.files, None)).unwrap();
    let report = lab.show("meetings/NOMAD/2026-11-13/report.md");
    let count = lab.count();

    // Unfinished work of the user is left alone.
    write(&repo.root().join("notes.txt"), "mine\n");
    let file = note(lab.tmp.path(), "c1.md", "first correction\n");
    let added = add_file(&repo, &add_request(&file)).unwrap();
    assert_eq!(added.file, "corrections.md");
    assert_eq!(added.repo_path, "meetings/NOMAD/2026-11-13/corrections.md");
    assert!(added.pushed && !added.already_published);
    assert_eq!(added.local, LocalSync::Updated);
    assert_eq!(lab.count(), count + 1);
    assert_eq!(added.commit, lab.tip());
    assert_eq!(
        lab.show("meetings/NOMAD/2026-11-13/corrections.md"),
        "first correction"
    );
    assert_eq!(
        lab.show("meetings/NOMAD/2026-11-13/report.md"),
        report,
        "the meeting's own files are not touched"
    );
    // Only that file is in the commit.
    assert_eq!(
        g(&lab.remote, &["diff", "--name-only", "main~1", "main"]),
        "meetings/NOMAD/2026-11-13/corrections.md"
    );
    assert!(repo
        .root()
        .join("meetings/NOMAD/2026-11-13/corrections.md")
        .is_file());
    assert!(status(&repo).contains("?? notes.txt"));
    assert_eq!(
        g(&lab.remote, &["log", "-1", "--format=%s", "main"]),
        "Add corrections.md to the meeting"
    );
}

#[test]
fn a_taken_name_gets_the_next_number_and_the_old_file_is_never_changed() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    let snap = snapshot(lab.tmp.path(), "ab", "A");
    publish_meeting(&repo, &request(&snap.files, None)).unwrap();

    let one = note(lab.tmp.path(), "one.md", "one\n");
    let two = note(lab.tmp.path(), "two.md", "two\n");
    let three = note(lab.tmp.path(), "three.md", "three\n");
    assert_eq!(
        add_file(&repo, &add_request(&one)).unwrap().file,
        "corrections.md"
    );
    assert_eq!(
        add_file(&repo, &add_request(&two)).unwrap().file,
        "corrections-2.md"
    );
    assert_eq!(
        add_file(&repo, &add_request(&three)).unwrap().file,
        "corrections-3.md"
    );
    assert_eq!(lab.show("meetings/NOMAD/2026-11-13/corrections.md"), "one");
    assert_eq!(
        lab.show("meetings/NOMAD/2026-11-13/corrections-2.md"),
        "two"
    );
    assert_eq!(
        lab.show("meetings/NOMAD/2026-11-13/corrections-3.md"),
        "three"
    );
}

#[test]
fn adding_the_same_file_again_is_idempotent_even_after_a_lost_answer() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    let snap = snapshot(lab.tmp.path(), "ab", "A");
    publish_meeting(&repo, &request(&snap.files, None)).unwrap();
    let before = g(repo.root(), &["rev-parse", "HEAD"]);
    let file = note(lab.tmp.path(), "c.md", "same\n");
    let first = add_file(&repo, &add_request(&file)).unwrap();
    let count = lab.count();

    let again = add_file(&repo, &add_request(&file)).unwrap();
    assert!(again.already_published);
    assert_eq!(again.file, first.file);
    assert_eq!(again.commit, first.commit);
    assert_eq!(lab.count(), count);

    // This clone never learned that the push happened.
    g(repo.root(), &["reset", "--hard", "--quiet", &before]);
    g(
        repo.root(),
        &["update-ref", "refs/remotes/origin/main", &before],
    );
    let retry = add_file(&repo, &add_request(&file)).unwrap();
    assert!(retry.already_published);
    assert_eq!(retry.file, "corrections.md");
    assert_eq!(retry.commit, first.commit);
    assert_eq!(lab.count(), count);
    assert_eq!(retry.local, LocalSync::Updated);
}

#[test]
fn a_meeting_that_is_not_in_the_repository_is_refused() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    let file = note(lab.tmp.path(), "c.md", "x\n");
    let err = add_file(&repo, &add_request(&file)).unwrap_err();
    assert!(!err.is_transient(), "{err}");
    assert!(err.message.contains("not in the repository"), "{err}");
    assert_eq!(lab.count(), 1);

    for (folder, stem, ext) in [("../x", "corrections", "md"), ("2026-11-13", "a/b", "md")] {
        let mut req = add_request(&file);
        req.folder = folder;
        req.stem = stem;
        req.ext = ext;
        let err = add_file(&repo, &req).unwrap_err();
        assert!(!err.is_transient(), "{err}");
    }
    let mut req = add_request(&file);
    req.base_path = "../outside";
    assert!(add_file(&repo, &req).is_err());
    let missing = lab.tmp.path().join("missing.md");
    assert!(add_file(&repo, &add_request(&missing)).is_err());
}

#[test]
fn a_push_refused_because_of_a_colleague_is_rebuilt_and_an_offline_remote_recovers() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    let snap = snapshot(lab.tmp.path(), "ab", "A");
    publish_meeting(&repo, &request(&snap.files, None)).unwrap();
    let file = note(lab.tmp.path(), "c.md", "mine\n");
    let added = add_file_observed(&repo, &add_request(&file), &mut |attempt| {
        if attempt == 1 {
            lab.colleague("x", &[("docs/colleague.md", "hello\n")]);
        }
    })
    .unwrap();
    assert_eq!(added.attempts, 2);
    assert_eq!(lab.show("docs/colleague.md"), "hello");
    assert_eq!(lab.show("meetings/NOMAD/2026-11-13/corrections.md"), "mine");

    // Offline: transient, nothing committed; back online: it works.
    let other = note(lab.tmp.path(), "d.md", "later\n");
    let head = g(repo.root(), &["rev-parse", "HEAD"]);
    let away = lab.tmp.path().join("away.git");
    fs::rename(&lab.remote, &away).unwrap();
    let err = add_file(&repo, &add_request(&other)).unwrap_err();
    assert!(err.is_transient(), "{err}");
    assert_eq!(g(repo.root(), &["rev-parse", "HEAD"]), head);
    fs::rename(&away, &lab.remote).unwrap();
    let ok = add_file(&repo, &add_request(&other)).unwrap();
    assert_eq!(ok.file, "corrections-2.md");
    assert_eq!(ok.commit, lab.tip());
}

#[test]
fn a_repository_without_a_remote_gets_a_local_commit_for_the_file() {
    let tmp = test_tmp::TestDir::new("publish-tests");
    let repo = Repo::init_local(tmp.path().join("local")).unwrap();
    set_identity(repo.root());
    repo.create_cycle(&Cycle::new("NOMAD", "Nomad rover"))
        .unwrap();
    g(repo.root(), &["add", "-A"]);
    g(repo.root(), &["commit", "--quiet", "-m", "start"]);
    let snap = snapshot(tmp.path(), "ab", "A");
    publish_meeting(&repo, &request(&snap.files, None)).unwrap();
    let file = note(tmp.path(), "c.md", "local note\n");
    let added = add_file(&repo, &add_request(&file)).unwrap();
    assert!(!added.pushed && added.remote.is_none());
    assert_eq!(added.local, LocalSync::Updated);
    assert_eq!(g(repo.root(), &["rev-parse", "HEAD"]), added.commit);
}

#[test]
fn a_remote_that_has_not_caught_up_with_the_meeting_is_waited_for() {
    let lab = Lab::new();
    let repo = lab.clone("clone");
    let before = lab.tip();
    let snap = snapshot(lab.tmp.path(), "ab", "A");
    let meeting = publish_meeting(&repo, &request(&snap.files, None)).unwrap();
    let file = note(lab.tmp.path(), "c.md", "late\n");

    // The remote still serves the tip from before the push (a lagging replica).
    g(&lab.remote, &["update-ref", "refs/heads/main", &before]);
    let err = add_file(&repo, &add_request(&file)).unwrap_err();
    assert!(err.is_transient(), "{err}");
    assert!(err.message.contains("does not show the folder"), "{err}");
    assert_eq!(lab.tip(), before, "nothing was pushed");

    // It catches up: the same call goes through.
    g(
        &lab.remote,
        &["update-ref", "refs/heads/main", &meeting.commit],
    );
    let added = add_file(&repo, &add_request(&file)).unwrap();
    assert!(added.pushed && !added.already_published);
    assert_eq!(added.file, "corrections.md");
}
