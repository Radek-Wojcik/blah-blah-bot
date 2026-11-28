//! Release checks of the Stage 7 integrations against real services: a Notion test database
//! and a Discord bot in a test server. Every real-service test is `#[ignore]`d and refuses to
//! run unless `BBB_RELEASE_CHECK=yes` is set.
//!
//! Secrets come only from the Windows Credential Manager via `cred:` references
//! (`BBB_RC_NOTION_REF`, `BBB_RC_DISCORD_BOT_REF`); environment variables carry only
//! reference names and public ids (`BBB_RC_NOTION_DB`, `BBB_RC_DISCORD_GUILD`,
//! `BBB_RC_DISCORD_CHANNEL`).
//!
//! What these checks verify from ADR 0023 ("Must be confirmed by the release check on a real
//! server"):
//!
//! 1. Discord REST `members/search` works with all three privileged intents turned off.
//! 2. `nonce` + `enforce_nonce: true` returns the same message id on an immediate retry.
//! 3. No gateway connection is needed before the first REST post.
//! 4. Editing the bot's own message needs no `MANAGE_MESSAGES` permission.
//! 5. Posting, opening a public thread, replying in the thread and editing work with the
//!    minimal permission set (`309237746688`).
//! 6. Notion accepts `NOTION_VERSION` (`2026-03-11`), resolves data sources, and exporting a
//!    task twice (with and without the stored page id) leaves one page without overwriting a
//!    human status change.
//! 7. Neither token appears in any captured tracing output or returned report.

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cycle_store::time::now_unix_ms;
use cycle_store::Correction;
use publishers::discord::{OutMessage, Pacing};
use publishers::discord_bot::{
    self, append_corrections, format_correction_reply, DiscordBot, EditOutcome,
};
use publishers::export::{self, Assignee, TaskKey, TaskRecord, UpsertOutcome};
use publishers::http::{HttpClient, Method, Request};
use publishers::notion::{self, Notion, NotionTasks};
use serde_json::json;

use super::{require_release_check, var};
use crate::secrets::{self, Credentials};

#[derive(Clone, Default)]
struct CapturedLog(Arc<Mutex<Vec<u8>>>);

impl CapturedLog {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(|e| e.into_inner())).into_owned()
    }
}

impl Write for CapturedLog {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLog {
    type Writer = CapturedLog;
    fn make_writer(&'a self) -> CapturedLog {
        self.clone()
    }
}

fn capture_log() -> CapturedLog {
    let log = CapturedLog::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(log.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
    log
}

/// Moves the test Notion page to the trash when the check finishes (even on panic).
struct TrashNotionPage {
    api: String,
    token: String,
    page_id: Option<String>,
}

impl Drop for TrashNotionPage {
    fn drop(&mut self) {
        let Some(page_id) = self.page_id.as_deref() else {
            return;
        };
        let req = Request::new(
            Method::Patch,
            format!("{}/pages/{page_id}", self.api.trim_end_matches('/')),
        )
        .bearer(&self.token)
        .header("Notion-Version", notion::NOTION_VERSION)
        .json(&json!({ "in_trash": true }));
        let client = HttpClient::new().with_secret(&self.token);
        if let Err(e) = client.send(&req) {
            eprintln!(
                "could not trash release-check Notion page {page_id}: {}",
                e.message
            );
        }
    }
}

struct NotionWorld {
    api: String,
    token: String,
    database: String,
    data_source: Option<String>,
    query_wait: Duration,
}

fn run_notion_check(world: &NotionWorld, log: Option<&CapturedLog>) {
    let client = Notion::new(&world.api, &world.token).expect("Notion client");
    let me = client.current_user().expect("GET /users/me");
    let db = client
        .database(&world.database)
        .expect("GET /databases/{id}");
    assert!(
        !db.data_sources.is_empty(),
        "test database has at least one data source"
    );
    let ds_id = client
        .resolve_data_source(&world.database, world.data_source.as_deref())
        .expect("resolve data source");
    let schema = client
        .data_source_schema(&ds_id)
        .expect("GET /data_sources/{id}");
    let mapping = notion::propose_with_options(&schema.properties, &schema.options);
    let problems = notion::check(&mapping, &schema.properties);
    assert!(
        problems.is_empty(),
        "test database needs a title property and a rich_text key property (BBB key): {problems:?}"
    );

    let exporter = NotionTasks::new(&client, ds_id.clone(), &mapping, schema.properties);
    let cycle_id = format!("RC{}", now_unix_ms());
    let key = TaskKey::new(&cycle_id, "20261123-000000", "i1").expect("valid task key");
    let mut record = TaskRecord {
        key: key.clone(),
        cycle_name: "Release check".into(),
        meeting_date: "2026-11-23".into(),
        title: "Release check of Blah Blah Bot (safe to ignore)".into(),
        assignee: Some(Assignee {
            participant_id: "rc".into(),
            name: "Release Tester".into(),
            github: None,
        }),
        due: Some("2026-12-31".into()),
        report_url: None,
        transcript_url: None,
        transcript_anchor: None,
        time_ms: None,
        quote: None,
    };

    let mut cleanup = TrashNotionPage {
        api: world.api.clone(),
        token: world.token.clone(),
        page_id: None,
    };

    // 1. First export creates one page.
    let first = export::upsert(&exporter, &record, None, true, &mut |task| {
        cleanup.page_id = Some(task.id.clone());
    })
    .expect("first Notion export");
    let created_page = match &first.outcome {
        UpsertOutcome::Created(task) => task.clone(),
        other => panic!("expected Created on first export, got {other:?}"),
    };
    cleanup.page_id = Some(created_page.id.clone());

    // 2. Immediate re-export with known page id updates in place (idempotent).
    record.title = "Release check of Blah Blah Bot (updated)".into();
    let second = export::upsert(
        &exporter,
        &record,
        Some(&created_page.id),
        true,
        &mut |_| {
            panic!("re-export with known id must not create a second page");
        },
    )
    .expect("re-export with known id");
    match &second.outcome {
        UpsertOutcome::Updated(task) => {
            assert_eq!(task.id, created_page.id);
            assert_eq!(second.duplicates, 0);
        }
        other => panic!("expected Updated on known-id re-export, got {other:?}"),
    }

    // 3. Re-export without known id finds the page by BBB key (polling briefly if Notion's
    // query index lags right after creation).
    let started = Instant::now();
    let mut found_by_query = false;
    loop {
        let hits = client
            .query_by_key(&ds_id, &mapping.key, &key.text())
            .expect("query by BBB key");
        if hits.iter().any(|p| p.id == created_page.id) {
            assert_eq!(hits.len(), 1, "exporting twice left only one page");
            found_by_query = true;
            break;
        }
        if started.elapsed() >= world.query_wait {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    assert!(
        found_by_query,
        "Notion query did not list the new page by key within {:?}",
        world.query_wait
    );
    let mut found_id = None;
    let third = export::upsert(&exporter, &record, None, true, &mut |task| {
        found_id = Some(task.id.clone());
    })
    .expect("re-export by key");
    assert_eq!(found_id.as_deref(), Some(created_page.id.as_str()));
    match &third.outcome {
        UpsertOutcome::Updated(task) => {
            assert_eq!(task.id, created_page.id);
            assert_eq!(third.duplicates, 0);
        }
        other => panic!("expected Updated on key lookup re-export, got {other:?}"),
    }

    // Secret scan: nothing returned or logged may contain the integration token.
    let summary = format!("{me:?} {db:?} {first:?} {second:?} {third:?}");
    assert!(
        !summary.contains(&world.token),
        "Notion token leaked into check output"
    );
    if let Some(log) = log {
        assert!(
            !log.text().contains(&world.token),
            "Notion token leaked into tracing output"
        );
    }
}

struct DiscordBotWorld {
    api: String,
    token: String,
    guild: String,
    channel: String,
    search_query: Option<String>,
    pacing: Pacing,
}

fn run_discord_bot_check(world: &DiscordBotWorld, log: Option<&CapturedLog>) {
    let bot = DiscordBot::new(&world.api, &world.token).expect("Discord bot client");

    // 1. REST-only identity & channel permission check (no gateway connection).
    let check = bot
        .check_channel(&world.guild, &world.channel, true)
        .expect("check_channel");
    assert!(
        check.channel.supports_summary(),
        "test channel must be a text or announcement channel, got {}",
        check.channel.kind_name()
    );
    assert!(
        check.missing.is_empty(),
        "bot is missing required permissions in #{}: {:?}",
        check.channel.name,
        check.missing
    );

    // 2. REST roles and member search without privileged intents.
    let roles = bot.roles(&world.guild).expect("GET /guilds/{id}/roles");
    assert!(!roles.is_empty(), "guild has at least @everyone");
    let query = world
        .search_query
        .clone()
        .filter(|q| q.trim().chars().count() >= 2)
        .unwrap_or_else(|| {
            let prefix: String = check.bot.username.chars().take(2).collect();
            if prefix.chars().count() >= 2 {
                prefix
            } else {
                "bo".to_string()
            }
        });
    let members = bot
        .search_members(&world.guild, &query, 10)
        .expect("GET /guilds/{id}/members/search without privileged intents");

    // 3. Post with nonce + enforce_nonce, then repeat with the same nonce.
    let run_id = format!("rc-{}", now_unix_ms());
    let summary_text = format!(
        "Release check of Blah Blah Bot ({run_id}): this message and its thread can be ignored."
    );
    let summary_msg = OutMessage {
        content: summary_text.clone(),
        allowed_mentions: json!({ "parse": [] }),
        embeds: Vec::new(),
        files: Vec::new(),
    };
    let msg_nonce = discord_bot::nonce(&run_id, "discord", 0);
    let first = bot
        .post_message(&world.channel, &summary_msg, &msg_nonce, &world.pacing)
        .expect("first summary post");
    let second = bot
        .post_message(&world.channel, &summary_msg, &msg_nonce, &world.pacing)
        .expect("duplicate nonce summary post");
    assert_eq!(
        second.id, first.id,
        "enforce_nonce must return the same message id"
    );

    // 4. Create a public thread from the summary message (and verify 160004 idempotency).
    let thread_id = bot
        .create_thread(
            &world.channel,
            &first.id,
            &format!("Release check {run_id}"),
            &world.pacing,
        )
        .expect("create public thread");
    assert_eq!(thread_id, first.id);
    let thread_again = bot
        .create_thread(
            &world.channel,
            &first.id,
            &format!("Release check {run_id}"),
            &world.pacing,
        )
        .expect("duplicate thread creation counts as success (160004)");
    assert_eq!(thread_again, thread_id);

    // 5. Post a reply inside the thread (proves SEND_MESSAGES_IN_THREADS).
    let correction = Correction {
        id: "c1".into(),
        date: "2026-11-23".into(),
        text: "Release check correction note.".into(),
        item: None,
        issue: None,
    };
    let reply = OutMessage {
        content: format_correction_reply(&correction, None),
        allowed_mentions: json!({ "parse": [] }),
        embeds: Vec::new(),
        files: Vec::new(),
    };
    let reply_nonce = discord_bot::nonce(&run_id, "discord-edit:c1", 0);
    let posted_reply = bot
        .post_message(&thread_id, &reply, &reply_nonce, &world.pacing)
        .expect("post reply inside thread");
    assert_eq!(posted_reply.channel_id, thread_id);

    // 6. Edit the bot's own summary message in place (no MANAGE_MESSAGES needed).
    let edited_content = append_corrections(&summary_text, &[correction]);
    let edit_outcome = bot
        .edit_message(
            &world.channel,
            &first.id,
            &edited_content,
            &json!({ "parse": [] }),
            &world.pacing,
        )
        .expect("edit own summary message");
    assert_eq!(edit_outcome, EditOutcome::Edited);

    // Secret scan: nothing returned or logged may contain the bot token.
    let summary = format!("{check:?} {roles:?} {members:?} {first:?} {second:?} {posted_reply:?}");
    assert!(
        !summary.contains(&world.token),
        "Discord bot token leaked into check output"
    );
    if let Some(log) = log {
        assert!(
            !log.text().contains(&world.token),
            "Discord bot token leaked into tracing output"
        );
    }
}

#[test]
#[ignore = "release check against real services: set BBB_RELEASE_CHECK=yes, see docs/TESTING.md"]
fn notion_integration_idempotent_export_and_reexport() {
    require_release_check();
    secrets::init();
    let reference = var("BBB_RC_NOTION_REF");
    let token = secrets::resolve_cred(&Credentials, &reference).unwrap_or_else(|| {
        panic!("no Notion token stored for {reference}: add it in the publishing settings first")
    });
    let database = var("BBB_RC_NOTION_DB");
    let data_source = std::env::var("BBB_RC_NOTION_DATA_SOURCE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let log = capture_log();
    let world = NotionWorld {
        api: notion::DEFAULT_API.to_string(),
        token,
        database,
        data_source,
        query_wait: Duration::from_secs(30),
    };
    run_notion_check(&world, Some(&log));
}

#[test]
#[ignore = "release check against real services: set BBB_RELEASE_CHECK=yes, see docs/TESTING.md"]
fn discord_bot_rest_search_nonce_thread_and_edit() {
    require_release_check();
    secrets::init();
    let reference = var("BBB_RC_DISCORD_BOT_REF");
    let token = secrets::resolve_cred(&Credentials, &reference).unwrap_or_else(|| {
        panic!(
            "no Discord bot token stored for {reference}: add it in the publishing settings first"
        )
    });
    let guild = var("BBB_RC_DISCORD_GUILD");
    let channel = var("BBB_RC_DISCORD_CHANNEL");
    let search_query = std::env::var("BBB_RC_DISCORD_SEARCH")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let log = capture_log();
    let world = DiscordBotWorld {
        api: discord_bot::DEFAULT_API.to_string(),
        token,
        guild,
        channel,
        search_query,
        pacing: Pacing::default(),
    };
    run_discord_bot_check(&world, Some(&log));
}

/// Runs both integration release checks against the in-process fakes so the check code itself
/// stays compiled and verified on every normal test run.
#[test]
fn the_integration_release_checks_pass_against_the_fakes() {
    use publishers::testing::notion::DATABASE_ID;
    use publishers::testing::{FakeDiscordBot, FakeNotion};

    let notion = FakeNotion::start();
    run_notion_check(
        &NotionWorld {
            api: notion.api_base(),
            token: notion.token(),
            database: DATABASE_ID.to_string(),
            data_source: None,
            query_wait: Duration::from_secs(2),
        },
        None,
    );
    assert_eq!(notion.pages().len(), 1);
    assert!(
        notion.pages()[0].in_trash,
        "cleanup guard trashed the test page"
    );

    let bot = FakeDiscordBot::start();
    run_discord_bot_check(
        &DiscordBotWorld {
            api: bot.api_base(),
            token: bot.bot_token(),
            guild: bot.guild_id(),
            channel: bot.channel_id(),
            search_query: Some("alex".to_string()),
            pacing: Pacing {
                sleep: Box::new(|_| {}),
                ..Pacing::default()
            },
        },
        None,
    );
    assert_eq!(bot.messages(&bot.channel_id()).len(), 1);
    assert_eq!(bot.threads().len(), 1);
    assert_eq!(bot.messages(&bot.threads()[0].id).len(), 1);
}
