use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use super::perms::{
    ATTACH_FILES, CREATE_PUBLIC_THREADS, INVITE_PERMISSIONS, READ_MESSAGE_HISTORY, SEND_MESSAGES,
    SEND_MESSAGES_IN_THREADS, VIEW_CHANNEL,
};
use super::*;
use crate::discord::Attachment;
use crate::testing::discord::rate_limited;
use crate::testing::discord_bot::{FakeDiscordBot, FakeMember};
use crate::testing::{Fault, Reply};

// ----- helpers ---------------------------------------------------------------------------

fn client() -> HttpClient {
    HttpClient::with_timeouts(Duration::from_millis(800), Duration::from_secs(20))
}

fn bot_of(fake: &FakeDiscordBot) -> DiscordBot {
    DiscordBot::with_client(client(), &fake.api_base(), &fake.bot_token()).expect("fake bot")
}

fn msg(text: &str) -> OutMessage {
    OutMessage {
        content: text.to_string(),
        allowed_mentions: json!({"parse": []}),
        embeds: Vec::new(),
        files: Vec::new(),
    }
}

#[derive(Clone, Default)]
struct Sleeps(Arc<Mutex<Vec<Duration>>>);

impl Sleeps {
    fn pacing(&self, max_wait_secs: u64, retries: u32) -> Pacing {
        let log = Arc::clone(&self.0);
        Pacing {
            max_inline_wait: Duration::from_secs(max_wait_secs),
            max_inline_retries: retries,
            sleep: Box::new(move |d| log.lock().unwrap().push(d)),
        }
    }

    fn all(&self) -> Vec<Duration> {
        self.0.lock().unwrap().clone()
    }
}

fn pacing() -> Pacing {
    Sleeps::default().pacing(5, 2)
}

/// Posts one message to the fake's text channel and returns its id.
fn post(bot: &DiscordBot, fake: &FakeDiscordBot, text: &str, nonce: &str) -> String {
    bot.post_message(&fake.channel_id(), &msg(text), nonce, &pacing())
        .expect("post")
        .id
}

// ----- addresses, encoding, nonces ---------------------------------------------------------

#[test]
fn the_api_base_follows_the_endpoint_rules() {
    for ok in [
        "https://discord.com/api/v10",
        "https://discord.com/api/v10/",
        "https://discordapp.com/api/v10",
        "https://canary.discord.com/api/v10",
        "https://ptb.discord.com/api/v10",
        "http://127.0.0.1:8080/api/v10",
        "http://localhost:9/anything",
    ] {
        assert!(normalize_api_base(ok).is_ok(), "{ok}");
    }
    for bad in [
        "https://example.com/api/v10",
        "https://discord.com.evil.example/api/v10",
        "https://discord.com/api/v9",
        "https://discord.com/",
        "https://discord.com:8443/api/v10",
        "https://user:pw@discord.com/api/v10",
        "http://discord.com/api/v10",
        "http://192.168.1.5/api/v10",
        "https://discord.com/api/v10?x=1",
        "ftp://discord.com/api/v10",
        "not a url",
    ] {
        assert!(normalize_api_base(bad).is_err(), "{bad}");
    }
    assert_eq!(
        normalize_api_base("https://discord.com/api/v10/").unwrap(),
        DEFAULT_API
    );
    // A token can never be handed to another host.
    let err = DiscordBot::new("https://example.com/api/v10", "tok")
        .err()
        .unwrap();
    assert_eq!(err.kind, ErrorKind::InvalidRequest);
    let err = DiscordBot::new(DEFAULT_API, "  ").err().unwrap();
    assert!(err.message.contains("empty"), "{}", err.message);
}

#[test]
fn the_query_encoder_escapes_everything_but_unreserved_characters() {
    assert_eq!(percent_encode("ale"), "ale");
    assert_eq!(percent_encode("A-b_c.d~9"), "A-b_c.d~9");
    assert_eq!(percent_encode("Ala ma/kota?"), "Ala%20ma%2Fkota%3F");
    assert_eq!(percent_encode("a&b=c#d%e+f"), "a%26b%3Dc%23d%25e%2Bf");
    // One %XX per UTF-8 byte, upper-case hex.
    assert_eq!(percent_encode("ł"), "%C5%82");
    assert_eq!(percent_encode("日本"), "%E6%97%A5%E6%9C%AC");
    assert_eq!(percent_encode(""), "");
}

#[test]
fn the_hash_and_the_base_are_the_documented_ones() {
    // FNV-1a 64 test vectors.
    assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    assert_eq!(base36(0), "0");
    assert_eq!(base36(35), "z");
    assert_eq!(base36(36), "10");
    assert_eq!(base36(u64::MAX), "3w5e11264sgsf");
}

#[test]
fn a_nonce_is_deterministic_short_and_different_per_message() {
    let a = nonce("meeting-1", "discord", 0);
    assert_eq!(a, nonce("meeting-1", "discord", 0));
    assert_ne!(a, nonce("meeting-1", "discord", 1));
    assert_ne!(a, nonce("meeting-2", "discord", 0));
    assert_ne!(a, nonce("meeting-1", "discord-edit:3", 0));
    // The parts cannot be shifted into each other.
    assert_ne!(nonce("a:b", "c", 0), nonce("a", "b:c", 0));
    for i in 0..200 {
        let n = nonce(
            "2026-11-13T10-00-00Z-a-long-meeting-identifier",
            "discord-reply:correction-17",
            i,
        );
        assert!((1..=MAX_NONCE_CHARS).contains(&n.len()), "{n}");
        assert!(n
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()));
    }
}

#[test]
fn a_thread_name_is_one_line_of_at_most_100_characters() {
    assert_eq!(
        thread_name("Weekly sync, 2026-11-13"),
        "Weekly sync, 2026-11-13"
    );
    assert_eq!(
        thread_name("  Two\n\nlines\tand   gaps "),
        "Two lines and gaps"
    );
    assert_eq!(thread_name("").as_str(), "Meeting");
    assert_eq!(thread_name(" \n ").as_str(), "Meeting");
    let long = "ł".repeat(300);
    let name = thread_name(&long);
    assert_eq!(name.chars().count(), 100);
    // The cut never ends in a space.
    let spaced = format!("{} {}", "a".repeat(99), "b".repeat(50));
    let name = thread_name(&spaced);
    assert_eq!(name, "a".repeat(99));
    assert!(name.chars().count() <= MAX_THREAD_NAME_CHARS);
}

// ----- authorization and reading ---------------------------------------------------------

#[test]
fn every_request_carries_the_bot_authorization_and_nothing_else_of_the_token() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    bot.current_user().unwrap();
    let seen = fake.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].header("authorization"),
        Some(format!("Bot {}", fake.bot_token()).as_str())
    );
    assert_eq!(seen[0].path, "/api/v10/users/@me");
    assert_eq!(seen[0].method, "GET");
}

#[test]
fn a_wrong_token_is_rejected_in_our_words_and_never_repeated() {
    let fake = FakeDiscordBot::start();
    let wrong = "wrong-Token_value.0123456789";
    let bot = DiscordBot::with_client(client(), &fake.api_base(), wrong).unwrap();
    let err = bot.current_user().unwrap_err();
    assert_eq!((err.status, err.failure), (Some(401), Failure::Permanent));
    assert_eq!(err.message, "Discord rejected the bot token");
    assert!(!err.message.contains(wrong));
}

#[test]
fn the_bot_guild_roles_member_and_channel_are_read() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let me = bot.current_user().unwrap();
    assert_eq!(
        (me.id.as_str(), me.username.as_str()),
        (fake.bot_id().as_str(), "blah-blah-bot")
    );

    let guild = bot.guild(&fake.guild_id()).unwrap();
    assert_eq!(guild.name, "Nomad team");
    assert_ne!(guild.owner_id, me.id);

    let roles = bot.roles(&fake.guild_id()).unwrap();
    assert_eq!(roles.len(), 3);
    let everyone = roles.iter().find(|r| r.id == fake.guild_id()).unwrap();
    assert_eq!(everyone.permissions, 0);
    let bots_role = roles.iter().find(|r| r.name == "Blah Blah Bot").unwrap();
    // Permissions arrive as decimal strings and are read as integers.
    assert_eq!(bots_role.permissions, INVITE_PERMISSIONS);
    let regulars = roles
        .iter()
        .find(|r| r.id == fake.regulars_role_id())
        .unwrap();
    assert!(regulars.mentionable);

    let member = bot.member(&fake.guild_id(), &me.id).unwrap();
    assert!(member.bot);
    assert_eq!(member.roles, [bots_role.id.clone()]);

    let channel = bot.channel(&fake.channel_id()).unwrap();
    assert_eq!(channel.name, "meetings");
    assert_eq!(channel.guild_id.as_deref(), Some(fake.guild_id().as_str()));
    assert_eq!((channel.kind, channel.kind_name()), (CHANNEL_TEXT, "text"));
    assert!(channel.supports_summary());
    let forum = bot.channel(&fake.forum_channel_id()).unwrap();
    assert_eq!(forum.kind_name(), "forum");
    assert!(!forum.supports_summary());
    let news = bot.channel(&fake.announcement_channel_id()).unwrap();
    assert!(news.supports_summary());
}

#[test]
fn unknown_ids_are_named_and_ids_with_other_characters_are_never_sent() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let err = bot.channel("300000000000009999").unwrap_err();
    assert_eq!((err.status, err.failure), (Some(404), Failure::Permanent));
    assert!(
        err.message.contains("does not know this channel"),
        "{}",
        err.message
    );
    let err = bot.guild("400000000000009999").unwrap_err();
    assert!(
        err.message.contains("does not know this server"),
        "{}",
        err.message
    );
    let err = bot
        .member(&fake.guild_id(), "600000000000009999")
        .unwrap_err();
    assert!(err.message.contains("not a member"), "{}", err.message);

    fake.clear_requests();
    for bad in ["../users/@me", "1/2", "", "12 3", "abc"] {
        let err = bot.channel(bad).unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidRequest, "{bad}");
    }
    assert_eq!(fake.request_count(), 0);
}

// ----- member search ---------------------------------------------------------------------

#[test]
fn members_are_found_by_the_prefix_of_the_username_or_the_nickname() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let g = fake.guild_id();
    let names = |q: &str| {
        let mut ids: Vec<String> = bot
            .search_members(&g, q, 10)
            .unwrap()
            .into_iter()
            .map(|m| m.username)
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(names("ale"), ["alex_k", "alexandra_n"]);
    // Case does not matter, and a nickname counts: "Mia" is the nickname of maria_l.
    assert_eq!(names("MI"), ["maria_l"]);
    assert_eq!(names("Alex"), ["alex_k", "alexandra_n"]);
    // A prefix, not a substring, and the global name is not searched.
    assert!(names("lex").is_empty());
    assert!(names("Kowal").is_empty());
    assert!(names("zz").is_empty());

    let found = bot.search_members(&g, "mi", 10).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].user_id, "600000000000000003");
    assert_eq!(found[0].global_name.as_deref(), Some("Maria Lis"));
    assert_eq!(found[0].nick.as_deref(), Some("Mia"));
    assert!(!found[0].bot);
}

#[test]
fn the_query_goes_out_percent_encoded_and_the_limit_is_kept_in_range() {
    let fake = FakeDiscordBot::start();
    fake.add_member(FakeMember::new(
        "600000000000000009",
        "lukasz_z",
        None,
        Some("Łukasz Z."),
    ));
    let bot = bot_of(&fake);
    let g = fake.guild_id();

    let found = bot.search_members(&g, "łu", 10).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].nick.as_deref(), Some("Łukasz Z."));
    let last = fake.requests().pop().unwrap();
    assert_eq!(last.path, format!("/api/v10/guilds/{g}/members/search"));
    assert_eq!(last.query, "query=%C5%82u&limit=10");

    // A query that could break out of the URL stays one value.
    bot.search_members(&g, " a&limit=1000#x ", 10).unwrap();
    let last = fake.requests().pop().unwrap();
    assert_eq!(last.query, "query=a%26limit%3D1000%23x&limit=10");

    bot.search_members(&g, "al", 0).unwrap();
    assert_eq!(fake.requests().pop().unwrap().query, "query=al&limit=1");
    bot.search_members(&g, "al", 5000).unwrap();
    assert_eq!(fake.requests().pop().unwrap().query, "query=al&limit=1000");
    // The limit applies.
    assert_eq!(bot.search_members(&g, "al", 1).unwrap().len(), 1);
    assert_eq!(bot.search_members(&g, "al", 2).unwrap().len(), 2);
}

// ----- the permission check ----------------------------------------------------------------

#[test]
fn a_bot_with_the_invite_permissions_passes_the_check() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let check = bot
        .check_channel(&fake.guild_id(), &fake.channel_id(), true)
        .unwrap();
    assert!(check.ok());
    assert!(check.missing.is_empty());
    assert_eq!(check.permissions, INVITE_PERMISSIONS);
    assert_eq!(check.guild_name, "Nomad team");
    assert_eq!(check.channel.name, "meetings");
    assert_eq!(check.bot.id, fake.bot_id());
}

#[test]
fn the_check_names_what_is_missing() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    fake.set_bot_permissions(VIEW_CHANNEL | SEND_MESSAGES | READ_MESSAGE_HISTORY);
    let check = bot
        .check_channel(&fake.guild_id(), &fake.channel_id(), true)
        .unwrap();
    assert!(!check.ok());
    assert_eq!(
        check.missing,
        [
            "Attach Files",
            "Create Public Threads",
            "Send Messages in Threads"
        ]
    );
    // Without a thread the thread permissions are not needed.
    let check = bot
        .check_channel(&fake.guild_id(), &fake.channel_id(), false)
        .unwrap();
    assert_eq!(check.missing, ["Attach Files"]);
}

#[test]
fn the_check_follows_overwrites_the_owner_and_administrator() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let (g, c) = (fake.guild_id(), fake.channel_id());

    fake.set_overwrites(
        &c,
        vec![Overwrite {
            id: fake.bot_id(),
            kind: OverwriteKind::Member,
            allow: 0,
            deny: SEND_MESSAGES | CREATE_PUBLIC_THREADS,
        }],
    );
    let check = bot.check_channel(&g, &c, true).unwrap();
    // No Send Messages also takes Attach Files with it.
    assert_eq!(
        check.missing,
        ["Send Messages", "Attach Files", "Create Public Threads"]
    );
    // Another channel has no overwrite.
    let other = bot
        .check_channel(&g, &fake.announcement_channel_id(), true)
        .unwrap();
    assert!(other.ok());

    fake.set_overwrites(&c, Vec::new());
    fake.set_bot_permissions(0);
    assert_eq!(bot.check_channel(&g, &c, true).unwrap().missing.len(), 6);
    fake.set_bot_permissions(1 << 3);
    assert!(bot.check_channel(&g, &c, true).unwrap().ok());
    fake.set_bot_permissions(0);
    fake.set_guild_owner(&fake.bot_id());
    assert!(bot.check_channel(&g, &c, true).unwrap().ok());
}

#[test]
fn a_forum_channel_is_not_a_place_for_the_summary() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let check = bot
        .check_channel(&fake.guild_id(), &fake.forum_channel_id(), true)
        .unwrap();
    assert!(!check.channel.supports_summary());
    assert!(!check.ok());
    assert!(check.missing.is_empty());
    let err = bot
        .post_message(&fake.forum_channel_id(), &msg("hello"), "n1", &pacing())
        .unwrap_err();
    assert_eq!(err.failure, Failure::Permanent);
    assert!(fake.messages(&fake.forum_channel_id()).is_empty());
}

// ----- posting -----------------------------------------------------------------------------

#[test]
fn a_message_is_posted_with_its_content_mentions_and_nonce() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let mut message = msg("Meeting of 2026-11-13 <@600000000000000001>");
    message.allowed_mentions = json!({"parse": [], "users": ["600000000000000001"]});
    let posted = bot
        .post_message(&fake.channel_id(), &message, "abc123", &pacing())
        .unwrap();
    assert_eq!(posted.channel_id, fake.channel_id());
    let stored = fake.messages(&fake.channel_id());
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].id, posted.id);
    assert_eq!(stored[0].content, message.content);
    assert_eq!(
        stored[0].allowed_mentions,
        Some(message.allowed_mentions.clone())
    );
    assert_eq!(stored[0].nonce.as_deref(), Some("abc123"));
    assert_eq!(stored[0].payload["enforce_nonce"], true);
    // Links do not unfurl.
    assert_eq!(stored[0].payload["flags"], 4);
    let request = fake.requests().pop().unwrap();
    assert_eq!(
        request.path,
        format!("/api/v10/channels/{}/messages", fake.channel_id())
    );
    assert_eq!(request.header("content-type"), Some("application/json"));
}

#[test]
fn screenshots_go_as_a_multipart_message() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let mut message = msg("Screenshots");
    message.files = vec![
        Attachment::new("one.png", b"11111".to_vec()),
        Attachment::new("two.png", b"22".to_vec()),
    ];
    bot.post_message(&fake.channel_id(), &message, "shots", &pacing())
        .unwrap();
    let stored = fake.messages(&fake.channel_id()).remove(0);
    assert_eq!(stored.files.len(), 2);
    assert_eq!(stored.files[0].filename, "one.png");
    assert_eq!(stored.files[0].size, 5);
    // The nonce is part of the JSON of the multipart body.
    assert_eq!(stored.payload["nonce"], "shots");
    assert_eq!(stored.payload["enforce_nonce"], true);
    assert!(fake
        .requests()
        .pop()
        .unwrap()
        .header("content-type")
        .unwrap()
        .starts_with("multipart/form-data; boundary="));
}

#[test]
fn the_same_nonce_returns_the_same_message() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let first = post(&bot, &fake, "once", "n-1");
    let again = post(&bot, &fake, "once", "n-1");
    assert_eq!(first, again);
    assert_eq!(fake.messages(&fake.channel_id()).len(), 1);
    let other = post(&bot, &fake, "twice", "n-2");
    assert_ne!(first, other);
    assert_eq!(fake.messages(&fake.channel_id()).len(), 2);
    // The window of Discord is short: after it a duplicate is possible.
    fake.forget_nonces();
    let late = post(&bot, &fake, "once", "n-1");
    assert_ne!(first, late);
    assert_eq!(fake.messages(&fake.channel_id()).len(), 3);
}

#[test]
fn a_nonce_is_never_longer_than_discord_accepts() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    post(&bot, &fake, "x", &"n".repeat(60));
    let stored = fake.messages(&fake.channel_id()).remove(0);
    assert_eq!(stored.nonce.as_deref().map(str::len), Some(MAX_NONCE_CHARS));
}

#[test]
fn a_lost_answer_is_harmless_when_the_message_is_sent_again_with_its_nonce() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    fake.inject_for("POST", "/api/v10/channels", Fault::DropAfter, 1);
    let n = nonce("meeting-1", "discord", 0);
    let err = bot
        .post_message(&fake.channel_id(), &msg("Summary"), &n, &pacing())
        .unwrap_err();
    assert!(err.is_offline());
    // Discord has the message, the bot does not know.
    assert_eq!(fake.messages(&fake.channel_id()).len(), 1);
    let retry = bot
        .post_message(&fake.channel_id(), &msg("Summary"), &n, &pacing())
        .unwrap();
    assert_eq!(retry.id, fake.messages(&fake.channel_id())[0].id);
    assert_eq!(fake.messages(&fake.channel_id()).len(), 1);
}

#[test]
fn the_message_limits_of_the_webhook_apply_here_too() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let c = fake.channel_id();
    post(&bot, &fake, &"a".repeat(2000), "ok-1");
    post(&bot, &fake, &"ł".repeat(2000), "ok-2");
    for (i, text) in ["a".repeat(2001), "ł".repeat(2001)].iter().enumerate() {
        let err = bot
            .post_message(&c, &msg(text), &format!("big-{i}"), &pacing())
            .unwrap_err();
        assert_eq!((err.status, err.failure), (Some(400), Failure::Permanent));
        assert!(err.message.contains("Invalid Form Body"), "{}", err.message);
    }
    for text in ["", "   "] {
        let err = bot
            .post_message(&c, &msg(text), "empty", &pacing())
            .unwrap_err();
        assert!(err.message.contains("empty"), "{}", err.message);
    }
    let mut many = msg("embeds");
    many.embeds = (0..11).map(|i| json!({"title": format!("e{i}")})).collect();
    assert_eq!(
        bot.post_message(&c, &many, "embeds", &pacing())
            .unwrap_err()
            .status,
        Some(400)
    );
    let mut files = msg("files");
    files.files = (0..11)
        .map(|_| Attachment::new("a.png", b"x".to_vec()))
        .collect();
    assert_eq!(
        bot.post_message(&c, &files, "files", &pacing())
            .unwrap_err()
            .status,
        Some(400)
    );
    assert_eq!(fake.messages(&c).len(), 2);
}

#[test]
fn a_missing_permission_is_refused_and_named() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let (g, c) = (fake.guild_id(), fake.channel_id());

    fake.set_bot_permissions(INVITE_PERMISSIONS & !SEND_MESSAGES);
    let err = bot.post_message(&c, &msg("x"), "n", &pacing()).unwrap_err();
    assert_eq!((err.status, err.failure), (Some(403), Failure::Permanent));
    assert_eq!(
        err.message,
        "the bot is missing a permission in the channel"
    );
    let named = bot.explain_denied(err, &g, &c, true);
    assert!(
        named
            .message
            .starts_with("the bot is missing a permission in the channel: Send Messages"),
        "{}",
        named.message
    );
    assert!(fake.messages(&c).is_empty());

    // Screenshots need their own permission.
    fake.set_bot_permissions(INVITE_PERMISSIONS & !ATTACH_FILES);
    let mut shots = msg("Screenshots");
    shots.files = vec![Attachment::new("a.png", b"x".to_vec())];
    let err = bot.post_message(&c, &shots, "s", &pacing()).unwrap_err();
    assert_eq!(err.status, Some(403));
    assert!(bot
        .explain_denied(err, &g, &c, true)
        .message
        .ends_with("Attach Files"));
    // Text alone still goes.
    post(&bot, &fake, "text only", "t");
}

#[test]
fn a_channel_the_bot_cannot_see_is_reported_as_such() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    fake.set_bot_permissions(INVITE_PERMISSIONS & !VIEW_CHANNEL);
    for err in [
        bot.post_message(&fake.channel_id(), &msg("x"), "n", &pacing())
            .unwrap_err(),
        bot.channel(&fake.channel_id()).unwrap_err(),
    ] {
        assert_eq!(err.status, Some(403));
        assert_eq!(err.message, "the bot cannot see the channel");
        // Only a missing permission is explained; this is a different error.
        assert_eq!(
            bot.explain_denied(err.clone(), &fake.guild_id(), &fake.channel_id(), true),
            err
        );
    }
}

#[test]
fn explaining_leaves_other_errors_alone() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let unrelated = HttpError {
        kind: ErrorKind::Status,
        message: "HTTP 500".into(),
        status: Some(500),
        failure: Failure::Transient {
            retry_after_ms: None,
        },
    };
    assert_eq!(
        bot.explain_denied(
            unrelated.clone(),
            &fake.guild_id(),
            &fake.channel_id(),
            true
        ),
        unrelated
    );
    // The permission error stays as it is when nothing is missing after all.
    let denied = HttpError {
        kind: ErrorKind::Status,
        message: MISSING_PERMISSION_TEXT.into(),
        status: Some(403),
        failure: Failure::Permanent,
    };
    assert_eq!(
        bot.explain_denied(denied.clone(), &fake.guild_id(), &fake.channel_id(), true),
        denied
    );
}

// ----- threads -------------------------------------------------------------------------------

#[test]
fn a_thread_starts_on_the_summary_and_has_its_id() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let summary = post(&bot, &fake, "Summary", "s");
    let thread = bot
        .create_thread(
            &fake.channel_id(),
            &summary,
            "Weekly sync, 2026-11-13",
            &pacing(),
        )
        .unwrap();
    assert_eq!(thread, summary);
    let threads = fake.threads();
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].id, summary);
    assert_eq!(threads[0].parent_id, fake.channel_id());
    assert_eq!(threads[0].name, "Weekly sync, 2026-11-13");
    assert_eq!(threads[0].auto_archive_duration, 10_080);
    assert!(!threads[0].archived);
    let request = fake.requests().pop().unwrap();
    assert_eq!(
        request.path,
        format!(
            "/api/v10/channels/{}/messages/{summary}/threads",
            fake.channel_id()
        )
    );
    assert_eq!(request.json().unwrap()["type"], 11);
}

#[test]
fn a_long_title_is_cut_to_the_name_limit() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let summary = post(&bot, &fake, "Summary", "s");
    bot.create_thread(&fake.channel_id(), &summary, &"Ł".repeat(500), &pacing())
        .unwrap();
    assert_eq!(fake.threads()[0].name.chars().count(), 100);
    // A blank title still gives a valid name.
    let second = post(&bot, &fake, "Summary 2", "s2");
    bot.create_thread(&fake.channel_id(), &second, "  ", &pacing())
        .unwrap();
    assert_eq!(fake.threads()[1].name, "Meeting");
}

#[test]
fn a_thread_that_already_exists_is_success() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let summary = post(&bot, &fake, "Summary", "s");
    let c = fake.channel_id();
    assert_eq!(
        bot.create_thread(&c, &summary, "T", &pacing()).unwrap(),
        summary
    );
    // A retry after a lost answer: 160004, and the thread is the one we wanted.
    assert_eq!(
        bot.create_thread(&c, &summary, "T", &pacing()).unwrap(),
        summary
    );
    assert_eq!(fake.threads().len(), 1);
    assert_eq!(
        fake.requests_to(
            "POST",
            &format!("/api/v10/channels/{c}/messages/{summary}/threads")
        )
        .len(),
        2
    );
}

#[test]
fn creating_a_thread_needs_its_permission_and_a_suitable_channel() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let (g, c) = (fake.guild_id(), fake.channel_id());
    let summary = post(&bot, &fake, "Summary", "s");

    fake.set_bot_permissions(INVITE_PERMISSIONS & !CREATE_PUBLIC_THREADS);
    let err = bot.create_thread(&c, &summary, "T", &pacing()).unwrap_err();
    assert_eq!(err.status, Some(403));
    let named = bot.explain_denied(err, &g, &c, true);
    assert!(
        named.message.ends_with("Create Public Threads"),
        "{}",
        named.message
    );
    assert!(fake.threads().is_empty());
    fake.set_bot_permissions(INVITE_PERMISSIONS);

    // A message that does not exist.
    let err = bot
        .create_thread(&c, "700000000000099999", "T", &pacing())
        .unwrap_err();
    assert_eq!(err.message, "the message was deleted in Discord");
    // A thread cannot have a thread, and a forum is not a place for the summary.
    bot.create_thread(&c, &summary, "T", &pacing()).unwrap();
    let inner = post_in(&bot, &summary, "inside", "i1");
    let err = bot
        .create_thread(&summary, &inner, "T", &pacing())
        .unwrap_err();
    assert_eq!((err.status, err.failure), (Some(400), Failure::Permanent));
    assert_eq!(fake.threads().len(), 1);
}

fn post_in(bot: &DiscordBot, channel: &str, text: &str, nonce: &str) -> String {
    bot.post_message(channel, &msg(text), nonce, &pacing())
        .expect("post")
        .id
}

#[test]
fn messages_go_into_the_thread_and_need_their_own_permission() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let (g, c) = (fake.guild_id(), fake.channel_id());
    let summary = post(&bot, &fake, "Summary", "s");
    let thread = bot.create_thread(&c, &summary, "T", &pacing()).unwrap();

    post_in(&bot, &thread, "details", "d");
    assert_eq!(fake.messages(&thread).len(), 1);
    assert_eq!(
        fake.messages(&c).len(),
        1,
        "the summary stays in the channel"
    );

    // Sending in the channel is not enough for a thread.
    fake.set_bot_permissions(INVITE_PERMISSIONS & !SEND_MESSAGES_IN_THREADS);
    let err = bot
        .post_message(&thread, &msg("more"), "m", &pacing())
        .unwrap_err();
    assert_eq!(err.status, Some(403));
    let named = bot.explain_denied(err, &g, &c, true);
    assert!(
        named.message.ends_with("Send Messages in Threads"),
        "{}",
        named.message
    );
    assert_eq!(fake.messages(&thread).len(), 1);
}

#[test]
fn an_archived_thread_is_opened_again_and_the_post_repeated_once() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let c = fake.channel_id();
    let summary = post(&bot, &fake, "Summary", "s");
    let thread = bot.create_thread(&c, &summary, "T", &pacing()).unwrap();
    fake.archive(&thread);
    assert!(fake.threads()[0].archived);
    fake.clear_requests();

    let id = post_in(&bot, &thread, "after a correction", "r1");
    assert_eq!(fake.messages(&thread)[0].id, id);
    assert!(!fake.threads()[0].archived);
    let calls: Vec<(String, String)> = fake
        .requests()
        .into_iter()
        .map(|r| (r.method, r.path))
        .collect();
    let messages = format!("/api/v10/channels/{thread}/messages");
    let channel = format!("/api/v10/channels/{thread}");
    assert_eq!(
        calls,
        [
            ("POST".to_string(), messages.clone()),
            ("PATCH".to_string(), channel),
            ("POST".to_string(), messages),
        ]
    );
}

// ----- editing -----------------------------------------------------------------------------

#[test]
fn an_own_message_is_edited_and_the_edit_is_idempotent() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let c = fake.channel_id();
    let id = post(&bot, &fake, "Summary", "s");
    let mentions = json!({"parse": []});
    let outcome = bot
        .edit_message(
            &c,
            &id,
            "Summary\n\n**Corrections**\n2026-11-14: due date moved",
            &mentions,
            &pacing(),
        )
        .unwrap();
    assert_eq!(outcome, EditOutcome::Edited);
    let stored = fake.messages(&c).remove(0);
    assert_eq!(
        stored.content,
        "Summary\n\n**Corrections**\n2026-11-14: due date moved"
    );
    assert_eq!(stored.allowed_mentions, Some(mentions.clone()));
    assert_eq!(stored.edits, 1);

    // The same edit again changes nothing visible.
    let content = stored.content.clone();
    bot.edit_message(&c, &id, &content, &mentions, &pacing())
        .unwrap();
    let after = fake.messages(&c).remove(0);
    assert_eq!(after.content, content);
    assert_eq!(fake.messages(&c).len(), 1);
    let request = fake.requests().pop().unwrap();
    assert_eq!(
        (
            request.method.as_str(),
            request.json().unwrap()["flags"].clone()
        ),
        ("PATCH", json!(4))
    );
}

#[test]
fn editing_a_deleted_message_is_an_outcome_not_an_error() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let c = fake.channel_id();
    let id = post(&bot, &fake, "Summary", "s");
    fake.delete_message(&c, &id);
    let outcome = bot
        .edit_message(&c, &id, "new text", &json!({"parse": []}), &pacing())
        .unwrap();
    assert_eq!(outcome, EditOutcome::Deleted);
    assert!(fake.messages(&c).is_empty());
    // A channel that is gone is an error.
    let err = bot
        .edit_message(
            "300000000000009999",
            &id,
            "x",
            &json!({"parse": []}),
            &pacing(),
        )
        .unwrap_err();
    assert!(
        err.message.contains("does not know this channel"),
        "{}",
        err.message
    );
}

#[test]
fn an_edit_over_the_limit_is_refused_and_leaves_the_message() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let c = fake.channel_id();
    let id = post(&bot, &fake, "Summary", "s");
    let err = bot
        .edit_message(&c, &id, &"a".repeat(2001), &json!({"parse": []}), &pacing())
        .unwrap_err();
    assert_eq!((err.status, err.failure), (Some(400), Failure::Permanent));
    assert_eq!(fake.messages(&c)[0].content, "Summary");
    assert_eq!(fake.messages(&c)[0].edits, 0);
}

#[test]
fn a_message_in_an_archived_thread_is_edited_after_unarchiving() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let c = fake.channel_id();
    let summary = post(&bot, &fake, "Summary", "s");
    let thread = bot.create_thread(&c, &summary, "T", &pacing()).unwrap();
    let inside = post_in(&bot, &thread, "details", "d");
    fake.archive(&thread);
    let outcome = bot
        .edit_message(
            &thread,
            &inside,
            "details v2",
            &json!({"parse": []}),
            &pacing(),
        )
        .unwrap();
    assert_eq!(outcome, EditOutcome::Edited);
    assert_eq!(fake.messages(&thread)[0].content, "details v2");
    assert!(!fake.threads()[0].archived);
}

// ----- rate limits, failures and secrecy -------------------------------------------------

#[test]
fn a_short_rate_limit_is_slept_through_and_a_long_one_is_handed_back() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let c = fake.channel_id();

    let sleeps = Sleeps::default();
    fake.inject_for(
        "POST",
        "/api/v10/channels",
        Fault::Reply(rate_limited(1.5)),
        1,
    );
    let posted = bot
        .post_message(&c, &msg("x"), "rl-1", &sleeps.pacing(5, 2))
        .unwrap();
    // Retry-After is whole seconds, rounded up.
    assert_eq!(sleeps.all(), [Duration::from_secs(2)]);
    assert_eq!(fake.messages(&c)[0].id, posted.id);

    let sleeps = Sleeps::default();
    fake.inject_for(
        "POST",
        "/api/v10/channels",
        Fault::Reply(rate_limited(30.0)),
        1,
    );
    let err = bot
        .post_message(&c, &msg("y"), "rl-2", &sleeps.pacing(5, 2))
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::RateLimited);
    assert_eq!(
        err.failure,
        Failure::Transient {
            retry_after_ms: Some(30_000)
        }
    );
    assert!(sleeps.all().is_empty());
    assert_eq!(fake.messages(&c).len(), 1);
}

#[test]
fn an_exhausted_bucket_is_reported_with_the_message() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    // The fake sends no bucket headers: nothing to wait for.
    let posted = bot
        .post_message(&fake.channel_id(), &msg("x"), "b", &pacing())
        .unwrap();
    assert_eq!(posted.wait, None);
}

#[test]
fn an_offline_server_is_transient_and_the_token_is_not_in_the_text() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    let token = fake.bot_token();
    fake.go_offline();
    let err = bot.current_user().unwrap_err();
    assert!(err.is_offline());
    assert!(matches!(err.failure, Failure::Transient { .. }));
    assert!(!err.message.contains(&token), "{}", err.message);
    fake.go_online();

    fake.inject(
        Fault::Reply(Reply::json(
            500,
            &json!({"message": format!("oops {token}")}),
        )),
        1,
    );
    let err = bot.current_user().unwrap_err();
    assert_eq!(err.status, Some(500));
    assert!(matches!(err.failure, Failure::Transient { .. }));
    assert!(!err.message.contains(&token), "{}", err.message);

    // An error body that echoes the token in a permanent answer is scrubbed as well.
    fake.inject(
        Fault::Reply(Reply::json(
            400,
            &json!({"message": format!("bad {token}"), "code": 50035}),
        )),
        1,
    );
    let err = bot.current_user().unwrap_err();
    assert_eq!(err.failure, Failure::Permanent);
    assert!(!err.message.contains(&token), "{}", err.message);
}

#[test]
fn a_redirect_is_refused_and_the_token_goes_nowhere_else() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    fake.inject(
        Fault::Reply(Reply::empty(302).with_header("Location", "http://127.0.0.1:1/elsewhere")),
        1,
    );
    let err = bot.current_user().unwrap_err();
    assert_eq!(err.kind, ErrorKind::Redirect);
    assert_eq!(fake.request_count(), 1);
}

#[test]
fn the_debug_form_of_a_client_error_never_holds_the_token() {
    let fake = FakeDiscordBot::start();
    let wrong = "wrong-Token_value.0123456789";
    let bot = DiscordBot::with_client(client(), &fake.api_base(), wrong).unwrap();
    let err = bot.channel(&fake.channel_id()).unwrap_err();
    assert!(!format!("{err:?}").contains(wrong));
}

#[test]
fn a_posted_message_carries_no_token_in_what_the_fake_stored() {
    let fake = FakeDiscordBot::start();
    let bot = bot_of(&fake);
    post(&bot, &fake, "Summary", "s");
    let token = fake.bot_token();
    let stored = fake.messages(&fake.channel_id()).remove(0);
    assert!(!stored.payload.to_string().contains(&token));
    assert!(!stored.content.contains(&token));
}

// ----- the fake itself ---------------------------------------------------------------------

fn raw(
    fake: &FakeDiscordBot,
    method: &str,
    path: &str,
    body: Option<Value>,
    auth: bool,
) -> crate::http::Response {
    let url = format!("{}{path}", fake.api_base());
    let m = match method {
        "POST" => Method::Post,
        "PATCH" => Method::Patch,
        _ => Method::Get,
    };
    let mut request = Request::new(m, url);
    if let Some(b) = &body {
        request = request.json(b);
    }
    if auth {
        request = request.header("Authorization", &format!("Bot {}", fake.bot_token()));
    }
    client().send_raw(&request).unwrap()
}

#[test]
fn the_fake_answers_in_discords_shapes() {
    let fake = FakeDiscordBot::start();
    // No authorization, or another scheme: 401.
    assert_eq!(raw(&fake, "GET", "/users/@me", None, false).status, 401);
    let bearer = client()
        .send_raw(&Request::get(format!("{}/users/@me", fake.api_base())).bearer(&fake.bot_token()))
        .unwrap();
    assert_eq!(bearer.status, 401);
    // Unknown route and wrong guild.
    assert_eq!(raw(&fake, "GET", "/nothing", None, true).status, 404);
    let g = raw(&fake, "GET", "/guilds/400000000000000777", None, true);
    assert_eq!(
        (g.status, g.json().unwrap()["code"].clone()),
        (404, json!(10004))
    );
    // The search needs a query and a sane limit.
    let g = fake.guild_id();
    let no_query = raw(
        &fake,
        "GET",
        &format!("/guilds/{g}/members/search"),
        None,
        true,
    );
    assert_eq!(
        (no_query.status, no_query.json().unwrap()["code"].clone()),
        (400, json!(50035))
    );
    let bad_limit = raw(
        &fake,
        "GET",
        &format!("/guilds/{g}/members/search?query=a&limit=0"),
        None,
        true,
    );
    assert_eq!(bad_limit.status, 400);
    let default_limit = raw(
        &fake,
        "GET",
        &format!("/guilds/{g}/members/search?query=al"),
        None,
        true,
    );
    assert_eq!(default_limit.json().unwrap().as_array().unwrap().len(), 1);
    // A thread name must be 1..100 characters.
    let summary = {
        let bot = bot_of(&fake);
        post(&bot, &fake, "Summary", "s")
    };
    let c = fake.channel_id();
    let bad = raw(
        &fake,
        "POST",
        &format!("/channels/{c}/messages/{summary}/threads"),
        Some(json!({"name": "x".repeat(101)})),
        true,
    );
    assert_eq!(bad.status, 400);
    assert!(fake.threads().is_empty());
    // A nonce is limited to 25 characters.
    let long_nonce = raw(
        &fake,
        "POST",
        &format!("/channels/{c}/messages"),
        Some(json!({"content": "x", "nonce": "n".repeat(26)})),
        true,
    );
    assert_eq!(long_nonce.status, 400);
    assert_eq!(fake.messages(&c).len(), 1);
    // Created: 201 with the thread as a channel object.
    let ok = raw(
        &fake,
        "POST",
        &format!("/channels/{c}/messages/{summary}/threads"),
        Some(json!({"name": "T", "auto_archive_duration": 60})),
        true,
    );
    assert_eq!(ok.status, 201);
    let v = ok.json().unwrap();
    assert_eq!(
        (v["id"].as_str(), v["type"].clone(), v["parent_id"].as_str()),
        (Some(summary.as_str()), json!(11), Some(c.as_str()))
    );
    assert_eq!(v["thread_metadata"]["auto_archive_duration"], 60);
}

#[test]
fn append_corrections_is_deterministic_defuses_mentions_and_respects_the_limit() {
    let c1 = cycle_store::Correction {
        id: "c1".into(),
        date: "2026-11-15".into(),
        text: "Ping @everyone and <@111111111111111111> about the deadline.".into(),
        item: Some("i1".into()),
        issue: None,
    };
    let c2 = cycle_store::Correction {
        id: "c2".into(),
        date: "2026-11-16".into(),
        text: "Mount moved to Tuesday.".into(),
        item: None,
        issue: None,
    };
    let a = append_corrections("Meeting summary", &[c1.clone(), c2.clone()]);
    let b = append_corrections("Meeting summary", &[c1.clone(), c2.clone()]);
    assert_eq!(a, b);
    assert!(a.contains("**Corrections**"));
    assert!(a.contains("- 2026-11-15:"));
    assert!(a.contains("- 2026-11-16: Mount moved to Tuesday."));
    assert!(!a.contains("@everyone"));
    assert!(!a.contains("<@111111111111111111>"));

    // An oversized summary is trimmed so the result stays within 2000 UTF-16 units.
    let huge = "a".repeat(2500);
    let trimmed = append_corrections(&huge, &[c2]);
    assert!(crate::discord::content_units(&trimmed) <= crate::discord::MAX_CONTENT_UNITS);
    assert!(trimmed.contains("**Corrections**"));

    let reply = format_correction_reply(&c1, Some("https://example.test/corrections.md"));
    assert!(reply.contains("Correction (2026-11-15):"));
    assert!(reply.contains("[Correction file](https://example.test/corrections.md)"));
    assert!(!reply.contains("@everyone"));
    assert!(!reply.contains("<@111111111111111111>"));
    assert!(crate::discord::content_units(&reply) <= crate::discord::MAX_CONTENT_UNITS);
}
