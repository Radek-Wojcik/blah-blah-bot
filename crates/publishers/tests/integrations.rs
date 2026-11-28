//! End-to-end and resilience tests for Stage 7 integrations: Notion task export and the
//! REST-only Discord bot (threads, nonce deduplication, permission checks, correction edits,
//! offline recovery, and secret hygiene). Everything runs against local bare repositories and
//! loopback fake servers.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use cycle_store::{Correction, ItemType, PublishMode};
use publishers::discord_bot::perms::{READ_MESSAGE_HISTORY, SEND_MESSAGES, VIEW_CHANNEL};
use publishers::notion;
use publishers::outbox::Env;
use publishers::rules::{PublishTarget, Trigger};
use publishers::testing::fixtures::{item, task};
use publishers::testing::lab::{git, lab_draft, Lab, LabOptions, TOKEN};
use publishers::testing::notion::{conflict, rate_limited};
use publishers::testing::Fault;
use serde_json::{json, Value};
use store::{OpState, OutboxTarget, RefTarget};

const ALL: [PublishTarget; 3] = [
    PublishTarget::Github,
    PublishTarget::Notion,
    PublishTarget::Discord,
];

fn stage7_options() -> LabOptions {
    LabOptions {
        github: PublishMode::OnApproval,
        discord: Some(PublishMode::OnApproval),
        discord_bot: true,
        discord_thread: true,
        discord_edit_on_correction: true,
        notion: Some(PublishMode::OnApproval),
        ..LabOptions::default()
    }
}

fn anchors() -> BTreeMap<String, String> {
    BTreeMap::from([("i1".to_string(), "t-00-12-25".to_string())])
}

fn draft() -> cycle_store::Draft {
    lab_draft(
        vec![
            task("i1", "Design the mount", Some("alex"), Some("2026-11-20")),
            task("i2", "Order the wheels", Some("sam"), None),
            item("i3", ItemType::Decision, "Use six wheels"),
        ],
        vec![],
    )
}

fn correction(id: &str, date: &str, text: &str, item: Option<&str>) -> Correction {
    Correction {
        id: id.into(),
        date: date.into(),
        text: text.into(),
        item: item.map(String::from),
        issue: None,
    }
}

fn op_result(lab: &Lab, key: &str) -> Value {
    let text = lab.op(key).result.expect("operation result");
    serde_json::from_str(&text).expect("valid JSON result")
}

#[test]
fn full_loop_with_github_notion_and_discord_bot_thread() {
    let lab = Lab::new(stage7_options());
    let plan = lab.preview(&draft(), Trigger::Explicit, &ALL, &anchors());
    assert!(!plan.is_blocked(), "{:?}", plan.blocks);
    assert_eq!(plan.summary.notion.len(), 2);
    assert_eq!(plan.summary.discord_mode, Some("bot"));
    assert!(plan.summary.discord_thread);

    lab.publish(&draft(), Trigger::Explicit, &ALL, &anchors())
        .expect("publish");
    lab.run_until_settled(10);

    let states = lab.states();
    assert!(states.iter().all(|(_, s)| *s == "done"), "{states:?}");

    // Notion received one page per task with mapped fields and write pacing.
    let pages = lab.notion.pages();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].text("Name").as_deref(), Some("Design the mount"));
    assert_eq!(
        pages[0].text("BBB key").as_deref(),
        Some("bbb:v1:NOMAD:20261113-140501:i1")
    );
    assert_eq!(pages[0].option("Assignee").as_deref(), Some("Alex Example"));
    assert_eq!(pages[0].date("Due").as_deref(), Some("2026-11-20"));
    assert_eq!(pages[0].option("Status").as_deref(), Some("Not started"));
    assert_eq!(pages[0].text("Cycle").as_deref(), Some("Nomad rover"));
    assert!(pages[0].url_value("Meeting").is_some());
    assert_eq!(pages[1].text("Name").as_deref(), Some("Order the wheels"));

    let notion_sleeps = lab
        .env
        .sleeps
        .lock()
        .unwrap()
        .iter()
        .filter(|d| **d == notion::WRITE_PACING)
        .count();
    assert_eq!(notion_sleeps, 2, "350 ms pacing after each Notion write");

    // Discord bot posted the summary in the channel, created one public thread under it,
    // and placed the issue links and screenshot inside the thread.
    let channel = lab.discord_bot.channel_id();
    let channel_msgs = lab.discord_bot.messages(&channel);
    assert_eq!(channel_msgs.len(), 1);
    assert!(channel_msgs[0].content.contains("Meeting NOMAD 2026-11-13"));
    assert!(!channel_msgs[0].content.contains("Issues:"));
    assert!(channel_msgs[0].files.is_empty());

    let threads = lab.discord_bot.threads();
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].id, channel_msgs[0].id);
    let thread_msgs = lab.discord_bot.messages(&threads[0].id);
    assert_eq!(
        thread_msgs.len(),
        2,
        "issue links and screenshot in the thread"
    );
    assert!(thread_msgs[0].content.contains("Issues:"));
    assert!(thread_msgs[0].content.contains("#1"));
    assert_eq!(thread_msgs[1].files.len(), 1);
    assert_eq!(thread_msgs[1].files[0].filename, "0001-00-12-41.png");

    // external_refs holds both Notion pages and the Discord post metadata.
    let refs = lab.external_refs();
    assert_eq!(refs.len(), 3);
    assert!(refs
        .iter()
        .any(|r| r.target == RefTarget::Notion && r.ref_key == "task:i1"));
    assert!(refs
        .iter()
        .any(|r| r.target == RefTarget::Notion && r.ref_key == "task:i2"));
    let post_ref = refs
        .iter()
        .find(|r| r.target == RefTarget::Discord && r.ref_key == "post")
        .expect("discord post ref");
    assert_eq!(post_ref.external_id, channel_msgs[0].id);
    let extra: Value = serde_json::from_str(&post_ref.extra).unwrap();
    assert_eq!(extra["channel_id"], channel);
    assert_eq!(extra["thread_id"], threads[0].id);
    assert_eq!(extra["message_ids"].as_array().unwrap().len(), 3);
}

#[test]
fn notion_reexport_updates_fields_preserves_status_and_never_recreates_trashed_page() {
    let lab = Lab::new(stage7_options());
    lab.publish(
        &draft(),
        Trigger::Explicit,
        &[PublishTarget::Notion],
        &anchors(),
    )
    .expect("first export");
    lab.run_until_settled(6);
    assert_eq!(lab.notion.pages().len(), 2);

    // A person changes the status of i1 in Notion and trashes i2.
    let p1 = lab.notion.pages()[0].clone();
    let p2 = lab.notion.pages()[1].clone();
    lab.notion
        .set_property(&p1.id, "Status", json!({"status": {"name": "Done"}}));
    lab.notion.trash(&p2.id);

    // Preview sees existing URLs from external_refs.
    let preview = lab.preview(
        &draft(),
        Trigger::Explicit,
        &[PublishTarget::Notion],
        &anchors(),
    );
    assert!(preview.summary.notion[0].existing_url.is_some());

    // Re-export updates i1 while keeping Status = "Done", and leaves trashed i2 alone.
    let mut updated_draft = draft();
    updated_draft.items[0].text = "Design the carbon mount".into();
    lab.reexport_notion(&updated_draft, &anchors())
        .expect("reexport");
    lab.run_until_settled(6);

    let states = lab.states();
    assert!(states.iter().all(|(_, s)| *s == "done"), "{states:?}");
    let pages = lab.notion.pages();
    assert_eq!(pages.len(), 2, "no duplicate or recreated page");
    assert_eq!(
        pages[0].text("Name").as_deref(),
        Some("Design the carbon mount")
    );
    assert_eq!(
        pages[0].option("Status").as_deref(),
        Some("Done"),
        "human status change in Notion is preserved on re-export"
    );
    assert!(pages[1].in_trash, "trashed page stays trashed");

    let r1 = op_result(&lab, "notion:i1:r2");
    assert_eq!(r1["updated"], true);
    assert_eq!(r1["created"], false);
    assert_eq!(r1["removed"], false);

    let r2 = op_result(&lab, "notion:i2:r2");
    assert_eq!(r2["removed"], true);
    assert_eq!(r2["created"], false);
}

#[test]
fn notion_handles_409_429_retries_and_lag_queries_without_duplicates() {
    let lab = Lab::new(stage7_options());
    let single = lab_draft(
        vec![task(
            "i1",
            "Design the mount",
            Some("alex"),
            Some("2026-11-20"),
        )],
        vec![],
    );
    lab.publish(
        &single,
        Trigger::Explicit,
        &[PublishTarget::Notion],
        &anchors(),
    )
    .expect("publish");

    // 409 conflict, then 429 rate limit with Retry-After, then a lost answer after create
    // (the retry finds the page by TaskKey and stores its id without creating a duplicate).
    lab.notion
        .inject_for("POST", "/v1/pages", Fault::Reply(conflict()), 1);
    lab.notion
        .inject_for("POST", "/v1/pages", Fault::Reply(rate_limited(2)), 1);
    lab.notion
        .inject_for("POST", "/v1/pages", Fault::DropAfter, 1);

    let first = lab.tick();
    assert_eq!(first.retrying, 1);
    let op = lab.op("notion:i1");
    assert_eq!(op.state, OpState::Queued);
    assert_eq!(op.attempts, 1);

    lab.env.advance(35_000);
    let second = lab.tick();
    assert_eq!(second.retrying, 1);
    let op = lab.op("notion:i1");
    let wait = op.next_attempt_unix_ms.unwrap() - lab.env.now_ms();
    assert!(
        (1_500..=2_500).contains(&wait),
        "Retry-After 2s respected: {wait}"
    );

    // Third attempt creates the page on the server and drops the answer.
    lab.env.advance(3_000);
    let third = lab.tick();
    assert_eq!(third.retrying, 1);
    assert_eq!(lab.notion.pages().len(), 1);

    // Fourth attempt queries by key, finds the created page, stores external_refs, and updates it.
    lab.run_until_settled(6);
    assert_eq!(lab.op("notion:i1").state, OpState::Done);
    assert_eq!(
        lab.notion.pages().len(),
        1,
        "lost answer created no duplicate"
    );

    // Even when Notion queries lag behind newly created pages, re-export uses the stored
    // page id from external_refs and updates directly without creating a second page.
    lab.notion.lag_queries(5);
    lab.reexport_notion(&single, &anchors()).expect("reexport");
    lab.run_until_settled(4);
    assert_eq!(lab.op("notion:i1:r2").state, OpState::Done);
    assert_eq!(lab.notion.pages().len(), 1);
}

#[test]
fn discord_bot_lost_answer_uses_nonce_dedup_and_160004_thread_recovery() {
    let lab = Lab::new(stage7_options());
    let channel = lab.discord_bot.channel_id();
    lab.publish(
        &draft(),
        Trigger::Explicit,
        &[PublishTarget::Discord],
        &anchors(),
    )
    .expect("publish");

    // First POST creates the summary message on the server, then drops the TCP answer.
    lab.discord_bot.inject_for(
        "POST",
        &format!("/api/v10/channels/{channel}/messages"),
        Fault::DropAfter,
        1,
    );

    let first = lab.tick();
    assert_eq!(first.retrying, 1);
    assert_eq!(
        lab.discord_bot.messages(&channel).len(),
        1,
        "summary arrived before the answer was lost"
    );
    let summary_id = lab.discord_bot.messages(&channel)[0].id.clone();

    // Now drop the answer of thread creation as well: Discord creates the thread, the retry
    // gets 160004 ("a thread already exists") and treats it as success.
    lab.discord_bot.inject_for(
        "POST",
        &format!("/api/v10/channels/{channel}/messages/{summary_id}/threads"),
        Fault::DropAfter,
        1,
    );

    lab.env.advance(35_000);
    let second = lab.tick();
    assert_eq!(second.retrying, 1);
    assert_eq!(
        lab.discord_bot.messages(&channel).len(),
        1,
        "nonce + enforce_nonce returned the existing summary instead of duplicating it"
    );
    assert_eq!(lab.discord_bot.threads().len(), 1);

    // Third attempt sees 160004 on thread create -> succeeds and posts the screenshot in thread.
    lab.env.advance(65_000);
    lab.run_until_settled(4);
    assert_eq!(lab.op("discord").state, OpState::Done);
    assert_eq!(lab.discord_bot.messages(&channel).len(), 1);
    assert_eq!(lab.discord_bot.threads().len(), 1);
    assert_eq!(lab.discord_bot.messages(&summary_id).len(), 1);
}

#[test]
fn discord_bot_missing_permissions_names_them_and_resumes_after_fix() {
    let lab = Lab::new(stage7_options());
    let channel = lab.discord_bot.channel_id();
    // Allow posting in the channel, deny creating threads and posting in threads.
    lab.discord_bot
        .set_bot_permissions(VIEW_CHANNEL | SEND_MESSAGES | READ_MESSAGE_HISTORY);

    lab.publish(
        &draft(),
        Trigger::Explicit,
        &[PublishTarget::Discord],
        &anchors(),
    )
    .expect("publish");
    let report = lab.tick();
    assert_eq!(report.failed, 1);

    let op = lab.op("discord");
    assert_eq!(op.state, OpState::Failed);
    let err = op.last_error.as_deref().unwrap_or_default();
    assert!(err.contains("Create Public Threads"), "{err}");
    assert!(err.contains("Send Messages in Threads"), "{err}");
    // The summary message was already posted and saved in external_refs before thread failed.
    assert_eq!(lab.discord_bot.messages(&channel).len(), 1);

    // Grant full invite permissions, retry: resumes from external_refs without reposting.
    lab.discord_bot
        .set_bot_permissions(publishers::discord_bot::perms::INVITE_PERMISSIONS);
    lab.store
        .lock()
        .unwrap()
        .retry_op(op.id, lab.env.now_ms())
        .unwrap();
    lab.run_until_settled(4);

    assert_eq!(lab.op("discord").state, OpState::Done);
    assert_eq!(lab.discord_bot.messages(&channel).len(), 1);
    assert_eq!(lab.discord_bot.threads().len(), 1);
    let thread_id = lab.discord_bot.threads()[0].id.clone();
    assert_eq!(lab.discord_bot.messages(&thread_id).len(), 1);
}

#[test]
fn discord_bot_correction_edits_summary_unarchives_thread_and_handles_deleted_message() {
    let lab = Lab::new(stage7_options());
    let channel = lab.discord_bot.channel_id();
    lab.publish(
        &draft(),
        Trigger::Explicit,
        &[PublishTarget::Github, PublishTarget::Discord],
        &anchors(),
    )
    .expect("publish");
    lab.run_until_settled(8);

    let thread_id = lab.discord_bot.threads()[0].id.clone();
    let initial_thread_count = lab.discord_bot.messages(&thread_id).len();

    // Archive the thread to verify automatic unarchive on 50083 during correction reply.
    lab.discord_bot.archive(&thread_id);
    assert!(lab.discord_bot.threads()[0].archived);

    let c1 = correction(
        "c1",
        "2026-11-15",
        "Mount deadline moved to next Monday.",
        Some("i1"),
    );
    let out1 = lab
        .publish_correction(&c1, Trigger::Explicit)
        .expect("publish c1");
    assert!(out1.plan.summary.discord_edit);
    lab.run_until_settled(6);
    assert_eq!(lab.op("discord-edit:c1").state, OpState::Done);

    let r1 = op_result(&lab, "discord-edit:c1");
    assert_eq!(r1["edited"], true);
    assert_eq!(r1["replied"], true);
    assert!(!lab.discord_bot.threads()[0].archived, "thread unarchived");

    let summary = &lab.discord_bot.messages(&channel)[0];
    assert_eq!(summary.edits, 1);
    assert!(summary.content.contains("**Corrections**"));
    assert!(summary
        .content
        .contains("2026-11-15: Mount deadline moved to next Monday."));
    let thread_msgs = lab.discord_bot.messages(&thread_id);
    assert_eq!(thread_msgs.len(), initial_thread_count + 1);
    assert!(thread_msgs
        .last()
        .unwrap()
        .content
        .contains("Mount deadline moved to next Monday."));

    // A second correction deterministically re-renders with both c1 and c2.
    let c2 = correction("c2", "2026-11-16", "Wheels arrived early.", None);
    lab.publish_correction(&c2, Trigger::Explicit)
        .expect("publish c2");
    lab.run_until_settled(6);
    let summary = &lab.discord_bot.messages(&channel)[0];
    assert_eq!(summary.edits, 2);
    assert!(summary
        .content
        .contains("2026-11-15: Mount deadline moved to next Monday."));
    assert!(summary
        .content
        .contains("2026-11-16: Wheels arrived early."));
    assert_eq!(
        lab.discord_bot.messages(&thread_id).len(),
        initial_thread_count + 2
    );

    // Deleting the summary message makes a later correction edit complete as skipped, not failed.
    let summary_id = summary.id.clone();
    lab.discord_bot.delete_message(&channel, &summary_id);
    let c3 = correction("c3", "2026-11-17", "Final note.", None);
    lab.publish_correction(&c3, Trigger::Explicit)
        .expect("publish c3");
    lab.run_until_settled(6);
    assert_eq!(lab.op("discord-edit:c3").state, OpState::Done);
    let r3 = op_result(&lab, "discord-edit:c3");
    assert_eq!(r3["edited"], false);
    assert_eq!(r3["replied"], false);
    assert!(r3["skipped"].as_str().is_some());
}

#[test]
fn offline_recovery_for_notion_and_discord_bot() {
    let lab = Lab::new(stage7_options());
    lab.publish(
        &draft(),
        Trigger::Explicit,
        &[PublishTarget::Notion, PublishTarget::Discord],
        &anchors(),
    )
    .expect("publish");

    lab.notion.go_offline();
    lab.discord_bot.go_offline();

    let report = lab.tick();
    assert_eq!((report.done, report.retrying, report.failed), (0, 2, 0));
    let offline = lab.dispatcher.offline_targets();
    assert!(offline.contains(&OutboxTarget::Notion), "{offline:?}");
    assert!(offline.contains(&OutboxTarget::Discord), "{offline:?}");

    lab.notion.go_online();
    lab.discord_bot.go_online();
    lab.store
        .lock()
        .unwrap()
        .retry_all_ops(lab.env.now_ms())
        .unwrap();
    lab.dispatcher.clear_offline();
    lab.run_until_settled(6);

    let states = lab.states();
    assert!(states.iter().all(|(_, s)| *s == "done"), "{states:?}");
    assert!(lab.dispatcher.offline_targets().is_empty());
    assert_eq!(lab.notion.pages().len(), 2);
    assert_eq!(lab.discord_bot.threads().len(), 1);
}

#[derive(Clone, Default)]
struct LogBuf(Arc<Mutex<Vec<u8>>>);

impl Write for LogBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn files_under(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path
            .file_name()
            .is_some_and(|n| n == ".git" || n == "remote.git" || n == "seed")
        {
            continue;
        }
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[test]
fn no_stage7_secret_ends_up_in_db_outbox_external_refs_snapshots_repos_or_logs() {
    let lab = Lab::new(stage7_options());
    let notion_token = lab.notion.token();
    let bot_token = lab.discord_bot.bot_token();
    let wrong_notion = concat!("ntn_", "WrongNotionSecretValue0123456789abcdef");
    let wrong_bot = concat!(
        "MjIyMjIyMjIyMjIyMjIyMjIy",
        ".GwRoNg.",
        "WrongBotTokenSecretValue0123456789"
    );
    let secrets = [
        TOKEN.to_string(),
        notion_token.clone(),
        wrong_notion.to_string(),
        bot_token.clone(),
        wrong_bot.to_string(),
    ];

    let buf = {
        static LOG: std::sync::OnceLock<LogBuf> = std::sync::OnceLock::new();
        LOG.get_or_init(|| {
            let log = LogBuf::default();
            let make = log.clone();
            let subscriber = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::TRACE)
                .with_ansi(false)
                .with_writer(move || make.clone())
                .finish();
            let _ = tracing::subscriber::set_global_default(subscriber);
            tracing::callsite::rebuild_interest_cache();
            log
        })
        .clone()
    };

    lab.publish(&draft(), Trigger::Explicit, &ALL, &anchors())
        .expect("publish");

    // Force 401 failures with wrong Notion & Discord bot tokens, then recover with valid tokens.
    *lab.env.notion_token.lock().unwrap() = Some(wrong_notion.to_string());
    *lab.env.discord_bot_token.lock().unwrap() = Some(wrong_bot.to_string());
    lab.run_until_settled(6);
    assert_eq!(lab.op("notion:i1").state, OpState::Failed);
    assert_eq!(lab.op("discord").state, OpState::Failed);

    *lab.env.notion_token.lock().unwrap() = Some(notion_token);
    *lab.env.discord_bot_token.lock().unwrap() = Some(bot_token);
    lab.store
        .lock()
        .unwrap()
        .retry_all_ops(lab.env.now_ms())
        .unwrap();
    lab.run_until_settled(8);

    // Also run a correction edit in bot mode.
    let c1 = correction("c1", "2026-11-15", "Verified fix.", Some("i1"));
    lab.publish_correction(&c1, Trigger::Explicit)
        .expect("publish c1");
    lab.run_until_settled(6);

    let states = lab.states();
    assert!(states.iter().all(|(_, s)| *s == "done"), "{states:?}");

    // 1. Captured logs.
    let log = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    assert!(log.contains("publishing operation failed"), "{log}");
    for secret in &secrets {
        assert!(
            !log.contains(secret.as_str()),
            "the log holds a secret: {log}"
        );
    }

    // 2. Database, snapshots, corrections-out, clone working tree.
    let mut files = Vec::new();
    files_under(lab.tmp.path(), &mut files);
    assert!(files.iter().any(|f| f == &lab.db_path));
    for file in &files {
        let bytes = std::fs::read(file).unwrap();
        for secret in &secrets {
            assert!(
                !bytes.windows(secret.len()).any(|w| w == secret.as_bytes()),
                "{} holds a secret",
                file.display()
            );
        }
    }

    // 3. Git history and config of both remote and clone.
    for dir in [&lab.remote, &lab.clone_dir] {
        let history = git(dir, &["log", "--all", "-p", "--format=%H %an %ae %s"]);
        let config = std::fs::read_to_string(dir.join("config")).unwrap_or_default();
        for secret in &secrets {
            assert!(!history.contains(secret.as_str()), "git history leak");
            assert!(!config.contains(secret.as_str()), "git config leak");
        }
    }

    // 4. Outbox rows and external_refs rows.
    for op in lab.ops() {
        let dump = format!("{op:?}");
        for secret in &secrets {
            assert!(!dump.contains(secret.as_str()), "outbox row leak: {dump}");
        }
    }
    for r in lab.external_refs() {
        let dump = format!("{r:?}");
        for secret in &secrets {
            assert!(!dump.contains(secret.as_str()), "external_ref leak: {dump}");
        }
    }
}
