use std::sync::{Arc, Mutex};

use cycle_store::{DiscordRef, Draft, Participant};
use serde_json::{json, Value};

use super::*;
use crate::testing::discord::{rate_limited, MAX_FILE_BYTES};
use crate::testing::{FakeDiscord, FakeServer, Fault, Reply};

const USER: &str = "111111111111111111";
const ROLE: &str = "222222222222222222";

fn people() -> Vec<Participant> {
    vec![
        Participant {
            id: "alex".into(),
            name: "Alex Example".into(),
            github: None,
            discord: Some(DiscordRef {
                user_id: Some(USER.into()),
                role_id: None,
            }),
        },
        Participant {
            id: "sam".into(),
            name: "Sam Sample".into(),
            github: None,
            discord: Some(DiscordRef {
                user_id: None,
                role_id: Some(ROLE.into()),
            }),
        },
        Participant {
            id: "kim".into(),
            name: "Kim Plain".into(),
            github: None,
            discord: None,
        },
    ]
}

fn draft(items: Value, summary: &str) -> Draft {
    Draft::from_value(&json!({
        "version": 1,
        "meeting_id": "20261113-140501",
        "cycle": "NOMAD",
        "date": "2026-11-13",
        "language": "en",
        "mode": "fixed",
        "template": null,
        "meeting_type": null,
        "sections": [],
        "summary": summary,
        "items": items,
        "warnings": [],
    }))
    .expect("valid draft")
}

fn item(id: &str, kind: &str, text: &str, assignee: Option<&str>) -> Value {
    json!({
        "id": id, "type": kind, "text": text, "assignee": assignee, "due": null,
        "certainty": {"assignee": if assignee.is_some() { "stated" } else { "unknown" }, "due": "unknown"},
        "section": null, "relates_to": null, "source": null, "edited": false,
    })
}

fn units(s: &str) -> usize {
    content_units(s)
}

fn spec<'a>(d: &'a Draft, people: &'a [Participant], report_link: &'a str) -> TextSpec<'a> {
    TextSpec {
        draft: d,
        participants: people,
        template_body: None,
        title: "Weekly sync",
        report_link,
    }
}

// ----- splitting -------------------------------------------------------------------------

#[test]
fn text_of_exactly_the_limit_is_one_piece_and_one_more_is_two() {
    let exact = "a".repeat(2000);
    let pieces = split_content(&exact, MAX_CONTENT_UNITS);
    assert_eq!(pieces, vec![exact.clone()]);

    let over = "a".repeat(2001);
    let pieces = split_content(&over, MAX_CONTENT_UNITS);
    assert_eq!(pieces.len(), 2);
    assert_eq!(units(&pieces[0]), 2000);
    assert_eq!(pieces[1], "a");
}

#[test]
fn pieces_break_at_a_line_then_at_a_word_and_never_exceed_the_limit() {
    // Lines of 600 characters: three fit in 2000 with their breaks, the fourth starts a piece.
    let line = "x".repeat(600);
    let text = [line.as_str(); 7].join("\n");
    let pieces = split_content(&text, MAX_CONTENT_UNITS);
    assert_eq!(pieces.len(), 3);
    assert_eq!(pieces[0], [line.as_str(); 3].join("\n"));
    assert_eq!(pieces[1], [line.as_str(); 3].join("\n"));
    assert_eq!(pieces[2], line);

    // One long line of words: cut at a space.
    let words = vec!["word"; 900].join(" ");
    let pieces = split_content(&words, MAX_CONTENT_UNITS);
    assert!(pieces.len() >= 2);
    for p in &pieces {
        assert!(units(p) <= 2000);
        assert!(p.split(' ').all(|w| w == "word"), "cut inside a word");
    }
    assert_eq!(pieces.join(" "), words);
}

#[test]
fn a_long_single_word_is_cut_at_the_limit() {
    let text = "z".repeat(4500);
    let pieces = split_content(&text, MAX_CONTENT_UNITS);
    assert_eq!(
        pieces.iter().map(|p| units(p)).collect::<Vec<_>>(),
        vec![2000, 2000, 500]
    );
    assert_eq!(pieces.concat(), text);
}

#[test]
fn length_is_counted_in_utf16_units_and_pieces_end_on_character_boundaries() {
    // Each of these is one character but two UTF-16 units: 1000 fit, 1001 do not.
    let emoji = "\u{1F600}";
    let fits = emoji.repeat(1000);
    assert_eq!(units(&fits), 2000);
    assert_eq!(split_content(&fits, MAX_CONTENT_UNITS), vec![fits.clone()]);
    let over = emoji.repeat(1001);
    let pieces = split_content(&over, MAX_CONTENT_UNITS);
    assert_eq!(pieces.len(), 2);
    assert_eq!(pieces[0].chars().count(), 1000);
    assert_eq!(pieces[1].chars().count(), 1);

    // Two-byte characters (Polish) are one unit each.
    let pl = "\u{142}".repeat(2000);
    assert_eq!(split_content(&pl, MAX_CONTENT_UNITS).len(), 1);
    assert_eq!(
        split_content(&format!("{pl}\u{142}"), MAX_CONTENT_UNITS).len(),
        2
    );
}

#[test]
fn a_mention_is_not_cut_in_two() {
    let mention = format!("<@{USER}>");
    let text = format!("{}{mention}{}", "a".repeat(1990), "b".repeat(50));
    let pieces = split_content(&text, MAX_CONTENT_UNITS);
    assert_eq!(pieces.len(), 2);
    assert_eq!(pieces[0], "a".repeat(1990));
    assert!(pieces[1].starts_with(&mention));
    assert_eq!(pieces.concat(), text);
}

#[test]
fn blank_text_gives_no_pieces_and_no_piece_is_blank() {
    assert!(split_content("", MAX_CONTENT_UNITS).is_empty());
    assert!(split_content(" \n\n ", MAX_CONTENT_UNITS).is_empty());
    let text = format!("{}\n\n\n\n{}", "a".repeat(1999), "b".repeat(10));
    for p in split_content(&text, MAX_CONTENT_UNITS) {
        assert!(!p.trim().is_empty());
        assert!(units(&p) <= 2000);
    }
}

// ----- mentions --------------------------------------------------------------------------

#[test]
fn only_ids_from_the_cycle_that_the_content_contains_are_allowed() {
    let content = format!("Do it <@{USER}> and <@&{ROLE}> and <@{USER}> again");
    let allowed = allowed_mentions(&content, &people());
    assert_eq!(
        allowed,
        json!({"parse": [], "users": [USER], "roles": [ROLE]})
    );
}

#[test]
fn unknown_ids_and_mass_mentions_are_never_allowed() {
    let content = "<@999999999999999999> <@&888888888888888888> @everyone @here";
    assert_eq!(allowed_mentions(content, &people()), json!({"parse": []}));
    // A role id written as a user mention (and the reverse) does not match.
    let swapped = format!("<@{ROLE}> <@&{USER}>");
    assert_eq!(allowed_mentions(&swapped, &people()), json!({"parse": []}));
    // The nickname form of a user mention counts as the user.
    let nick = format!("<@!{USER}>");
    assert_eq!(
        allowed_mentions(&nick, &people()),
        json!({"parse": [], "users": [USER]})
    );
    // Broken syntax is not a mention.
    let broken = format!("<@{USER} <@>{USER}> <@&> <@x>");
    assert_eq!(allowed_mentions(&broken, &people()), json!({"parse": []}));
}

#[test]
fn parse_is_always_empty_so_nothing_is_parsed_from_the_text() {
    for content in ["", "hello", "@everyone <@&1> <@2>"] {
        let v = allowed_mentions(content, &people());
        assert_eq!(v["parse"], json!([]), "{content}");
    }
}

#[test]
fn allowed_lists_are_capped() {
    let mut many = Vec::new();
    let mut text = String::new();
    for i in 0..150u64 {
        let id = format!("{}", 300_000_000_000_000_000u64 + i);
        text.push_str(&format!("<@{id}> "));
        many.push(Participant {
            id: format!("p{i}"),
            name: format!("P{i}"),
            github: None,
            discord: Some(DiscordRef {
                user_id: Some(id),
                role_id: None,
            }),
        });
    }
    let v = allowed_mentions(&text, &many);
    assert_eq!(v["users"].as_array().unwrap().len(), MAX_ALLOWED_IDS);
}

// ----- rendering -------------------------------------------------------------------------

#[test]
fn the_short_template_mentions_assignees_and_links_the_report() {
    let d = draft(
        json!([
            item("i1", "decision", "Use aluminium", None),
            item("i2", "task", "Design the mount", Some("alex")),
            item("i3", "task", "Order parts", Some("sam")),
            item("i4", "task", "Check wiring", Some("kim")),
            item("i5", "note", "Wheels are 20 cm", None),
        ]),
        "We agreed on the mount.",
    );
    let people = people();
    let text = render_text(&spec(&d, &people, "https://example.test/report.md")).unwrap();
    assert_eq!(
        text,
        format!(
            "**Weekly sync** \u{b7} 2026-11-13\n\nWe agreed on the mount.\n\n## Decisions\n- Use aluminium\n\n## Tasks\n- Design the mount (<@{USER}>, due ?)\n- Order parts (<@&{ROLE}>, due ?)\n- Check wiring (@Kim Plain, due ?)\n\n[Full report](https://example.test/report.md)\n"
        )
    );
    // Notes are not part of the short message.
    assert!(!text.contains("Wheels"));
}

#[test]
fn text_from_the_model_cannot_notify_anybody() {
    let d = draft(
        json!([
            item("i1", "task", "Tell @everyone and @here now", Some("alex")),
            item(
                "i2",
                "decision",
                &format!("cc <@&{ROLE}> and <@{USER}>"),
                None
            ),
        ]),
        &format!("Ping <@{ROLE}> @everyone"),
    );
    let people = people();
    let text = render_text(&spec(&d, &people, "x")).unwrap();
    assert!(
        !text.contains("@everyone") && !text.contains("@here"),
        "{text}"
    );
    // Only the mention written from the cycle file is left, so only it is allowed.
    assert_eq!(text.matches(&format!("<@{USER}>")).count(), 1, "{text}");
    assert!(!text.contains(&format!("<@&{ROLE}>")), "{text}");
    let plan = build_plan(&text, vec![], vec![], &people);
    assert_eq!(plan.messages.len(), 1);
    assert_eq!(
        plan.messages[0].allowed_mentions,
        json!({"parse": [], "users": [USER]})
    );
}

#[test]
fn a_cycle_template_replaces_the_short_one() {
    let d = draft(json!([]), "Short.");
    let people = people();
    let mut s = spec(&d, &people, "https://example.test/r");
    s.template_body = Some("{date}: {summary} {report_link}");
    assert_eq!(
        render_text(&s).unwrap(),
        "2026-11-13: Short. https://example.test/r\n"
    );
    s.template_body = Some("{nope}");
    assert!(render_text(&s).is_err());
}

// ----- attachments -----------------------------------------------------------------------

fn file(name: &str, size: usize) -> Attachment {
    Attachment::new(name, vec![7u8; size])
}

#[test]
fn ten_files_are_one_message_and_eleven_are_two() {
    let ten: Vec<_> = (0..10).map(|i| file(&format!("s{i}.png"), 10)).collect();
    let (batches, skipped) = plan_attachments(ten);
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].len(), 10);
    assert!(skipped.is_empty());

    let eleven: Vec<_> = (0..11).map(|i| file(&format!("s{i}.png"), 10)).collect();
    let (batches, _) = plan_attachments(eleven);
    assert_eq!(
        batches.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![10, 1]
    );
}

#[test]
fn the_byte_limit_of_a_message_is_inclusive() {
    let half = MAX_BATCH_BYTES / 2;
    let (batches, skipped) = plan_attachments(vec![file("a.png", half), file("b.png", half)]);
    assert_eq!(batches.len(), 1, "exactly the limit fits");
    assert!(skipped.is_empty());

    let (batches, _) = plan_attachments(vec![file("a.png", half), file("b.png", half + 1)]);
    assert_eq!(batches.len(), 2, "one byte more starts a new message");
}

#[test]
fn a_file_over_the_limit_or_empty_is_skipped_and_the_rest_is_kept() {
    let (batches, skipped) = plan_attachments(vec![
        file("big.png", MAX_BATCH_BYTES + 1),
        file("ok.png", 100),
        file("empty.png", 0),
    ]);
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0][0].filename, "ok.png");
    assert_eq!(
        skipped,
        vec![
            SkippedFile {
                filename: "big.png".into(),
                reason: SkipReason::TooLarge
            },
            SkippedFile {
                filename: "empty.png".into(),
                reason: SkipReason::Empty
            },
        ]
    );
    // A file of exactly the limit is attached.
    let (batches, skipped) = plan_attachments(vec![file("max.png", MAX_BATCH_BYTES)]);
    assert_eq!((batches.len(), skipped.len()), (1, 0));
}

#[test]
fn at_most_three_messages_carry_files_and_the_rest_is_noted() {
    let files: Vec<_> = (0..35).map(|i| file(&format!("s{i}.png"), 10)).collect();
    let (batches, skipped) = plan_attachments(files);
    assert_eq!(
        batches.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![10, 10, 10]
    );
    assert_eq!(skipped.len(), 5);
    assert!(skipped.iter().all(|s| s.reason == SkipReason::TooMany));

    let files: Vec<_> = (0..31).map(|i| file(&format!("s{i}.png"), 10)).collect();
    let plan = build_plan("Meeting", vec![], files, &people());
    let text = &plan.messages[0].content;
    assert!(text.contains("1 screenshot not attached"), "{text}");
    assert_eq!(plan.messages.len(), 1 + 3);
    assert_eq!(plan.messages[1].content, "Screenshots 1/3");
    assert_eq!(plan.messages[3].content, "Screenshots 3/3");
}

#[test]
fn file_names_cannot_break_the_headers() {
    assert_eq!(
        Attachment::new("shot 1.png", vec![1]).filename,
        "shot_1.png"
    );
    let evil = Attachment::new("a\"; name=\"x\r\nEvil: 1.png", vec![1]).filename;
    assert!(!evil.contains(['"', '\r', '\n', ':', ';', ' ']), "{evil}");
    assert_eq!(
        Attachment::new("../../etc/passwd", vec![1]).filename,
        "passwd"
    );
    assert_eq!(Attachment::new("..\\x\\y.png", vec![1]).filename, "y.png");
    assert_eq!(Attachment::new("", vec![1]).filename, "file");
    assert_eq!(Attachment::new("...", vec![1]).filename, "file");
}

// ----- plan ------------------------------------------------------------------------------

#[test]
fn every_message_of_a_plan_respects_the_limits() {
    let text = (0..80)
        .map(|i| format!("- task {i} {}", "w ".repeat(60)))
        .collect::<Vec<_>>()
        .join("\n");
    let embeds: Vec<Value> = (0..25).map(|i| json!({"title": format!("e{i}")})).collect();
    let files: Vec<_> = (0..14).map(|i| file(&format!("s{i}.png"), 1000)).collect();
    let plan = build_plan(&text, embeds, files, &people());
    assert!(plan.skipped.is_empty());
    let mut embed_total = 0;
    let mut file_total = 0;
    for m in &plan.messages {
        assert!(units(&m.content) <= MAX_CONTENT_UNITS);
        assert!(m.embeds.len() <= MAX_EMBEDS);
        assert!(m.files.len() <= MAX_FILES);
        assert!(m.files.iter().map(|f| f.bytes.len()).sum::<usize>() <= MAX_BATCH_BYTES);
        assert!(!m.content.is_empty() || !m.embeds.is_empty() || !m.files.is_empty());
        assert_eq!(m.allowed_mentions["parse"], json!([]));
        embed_total += m.embeds.len();
        file_total += m.files.len();
    }
    assert_eq!(embed_total, 25);
    assert_eq!(file_total, 14);
}

#[test]
fn eleven_embeds_need_two_messages() {
    let embeds: Vec<Value> = (0..11).map(|i| json!({"title": format!("e{i}")})).collect();
    let plan = build_plan("Hi", embeds.clone(), vec![], &people());
    assert_eq!(plan.messages.len(), 2);
    assert_eq!(plan.messages[0].embeds.len(), 10);
    assert_eq!(plan.messages[0].content, "Hi");
    assert_eq!(plan.messages[1].embeds.len(), 1);
    assert_eq!(plan.messages[1].content, "");
    let exactly = build_plan("Hi", embeds[..10].to_vec(), vec![], &people());
    assert_eq!(exactly.messages.len(), 1);
}

#[test]
fn the_plan_is_deterministic() {
    let text = "line\n".repeat(900);
    let a = build_plan(&text, vec![], vec![file("a.png", 5)], &people());
    let b = build_plan(&text, vec![], vec![file("a.png", 5)], &people());
    assert_eq!(a, b);
    assert!(a.messages.len() > 2);
}

#[test]
fn the_payload_suppresses_link_previews_and_lists_attachments() {
    let plan = build_plan("Hello", vec![], vec![file("a.png", 5)], &people());
    let text = plan.messages[0].payload();
    assert_eq!(text["content"], "Hello");
    assert_eq!(text["flags"], 4);
    assert!(text.get("attachments").is_none());
    let files = plan.messages[1].payload();
    assert_eq!(
        files["attachments"],
        json!([{"id": 0, "filename": "a.png"}])
    );
}

// ----- multipart -------------------------------------------------------------------------

#[test]
fn multipart_body_golden() {
    let files = vec![
        Attachment::new("one.png", b"PNGDATA".to_vec()),
        Attachment::new("two.jpg", vec![0, 1, 2]),
    ];
    let payload = json!({"content": "hi"});
    let body = multipart_body("B", &payload, &files);
    let mut expected: Vec<u8> = Vec::new();
    expected.extend_from_slice(b"--B\r\nContent-Disposition: form-data; name=\"payload_json\"\r\nContent-Type: application/json\r\n\r\n{\"content\":\"hi\"}\r\n");
    expected.extend_from_slice(b"--B\r\nContent-Disposition: form-data; name=\"files[0]\"; filename=\"one.png\"\r\nContent-Type: image/png\r\n\r\nPNGDATA\r\n");
    expected.extend_from_slice(b"--B\r\nContent-Disposition: form-data; name=\"files[1]\"; filename=\"two.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n");
    expected.extend_from_slice(&[0, 1, 2]);
    expected.extend_from_slice(b"\r\n--B--\r\n");
    assert_eq!(body, expected);
}

#[test]
fn the_boundary_does_not_occur_in_the_data() {
    let first = "bbb-boundary-0000";
    let files = vec![Attachment::new("a.png", first.as_bytes().to_vec())];
    let b = pick_boundary(b"{}", &files);
    assert_ne!(b, first);
    assert_eq!(pick_boundary(b"{}", &[]), first);
    let in_payload = pick_boundary(first.as_bytes(), &[]);
    assert_ne!(in_payload, first);
}

// ----- sending, against the fake ---------------------------------------------------------

fn client(hook: &Webhook) -> HttpClient {
    HttpClient::with_timeouts(Duration::from_millis(800), Duration::from_secs(20))
        .with_secret(&hook.url())
        .with_secret(hook.token())
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

fn hook_of(dc: &FakeDiscord) -> Webhook {
    Webhook::parse(&dc.webhook_url()).expect("fake webhook")
}

fn three_messages() -> Vec<OutMessage> {
    build_plan(
        &format!(
            "{}\n{}\n{}",
            "a".repeat(1500),
            "b".repeat(1500),
            "c".repeat(1500)
        ),
        vec![],
        vec![],
        &people(),
    )
    .messages
}

fn run(
    dc: &FakeDiscord,
    messages: &[OutMessage],
    start: usize,
    pacing: &Pacing,
) -> Result<SendReport, SendFailure> {
    let hook = hook_of(dc);
    send_messages(
        &client(&hook),
        &hook,
        messages,
        start,
        pacing,
        &mut |_, _| {},
    )
}

#[test]
fn a_meeting_is_posted_as_messages_with_files_and_mentions() {
    let dc = FakeDiscord::start();
    let d = draft(
        json!([item("i1", "task", "Design the mount", Some("alex"))]),
        "We agreed.",
    );
    let people = people();
    let text = render_text(&spec(&d, &people, "https://example.test/report.md")).unwrap();
    let plan = build_plan(
        &text,
        vec![],
        vec![file("s1.png", 2000), file("s2.png", 3000)],
        &people,
    );
    assert_eq!(plan.messages.len(), 2);

    let hook = hook_of(&dc);
    let mut progress = Vec::new();
    let report = send_messages(
        &client(&hook),
        &hook,
        &plan.messages,
        0,
        &Pacing::default(),
        &mut |n, id| progress.push((n, id.to_string())),
    )
    .unwrap();
    assert_eq!(report.sent, 2);
    assert_eq!(report.message_ids.len(), 2);
    assert!(report.message_ids.iter().all(|id| !id.is_empty()));
    assert_eq!(
        progress.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        vec![1, 2]
    );

    let stored = dc.messages();
    assert_eq!(stored.len(), 2);
    assert!(stored[0].content.contains(&format!("<@{USER}>")));
    assert_eq!(
        stored[0].allowed_mentions,
        Some(json!({"parse": [], "users": [USER]}))
    );
    assert_eq!(stored[0].files.len(), 0);
    assert_eq!(stored[1].files.len(), 2);
    assert_eq!(stored[1].files[0].filename, "s1.png");
    assert_eq!(stored[1].files[0].size, 2000);
    assert_eq!(
        stored[1].files[0].content_type.as_deref(),
        Some("image/png")
    );
    assert_eq!(stored[1].allowed_mentions, Some(json!({"parse": []})));
    // The first message is JSON, the second multipart, both asked for the answer.
    let posts = dc.requests_to("POST", "/api/webhooks");
    assert!(posts[0]
        .header("content-type")
        .unwrap()
        .starts_with("application/json"));
    assert!(posts[1]
        .header("content-type")
        .unwrap()
        .starts_with("multipart/form-data; boundary="));
    assert!(posts.iter().all(|r| r.query_param("wait") == Some("true")));
}

#[test]
fn a_long_report_is_split_into_messages_the_fake_accepts() {
    let dc = FakeDiscord::start();
    let items: Vec<Value> = (0..60)
        .map(|i| {
            item(
                &format!("i{i}"),
                "decision",
                &format!("Decision number {i} {}", "word ".repeat(30)),
                None,
            )
        })
        .collect();
    let d = draft(json!(items), "Summary");
    let people = people();
    let text = render_text(&spec(&d, &people, "https://example.test/r")).unwrap();
    let plan = build_plan(&text, vec![], vec![], &people);
    assert!(plan.messages.len() > 1);
    let report = run(&dc, &plan.messages, 0, &Pacing::default()).unwrap();
    assert_eq!(report.sent, plan.messages.len());
    assert_eq!(dc.messages().len(), plan.messages.len());
    let joined: String = dc
        .messages()
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("Decision number 59"));
    assert!(joined.contains("[Full report](https://example.test/r)"));
}

#[test]
fn files_up_to_the_conservative_limit_are_accepted_by_the_fake() {
    let dc = FakeDiscord::start();
    const { assert!(MAX_BATCH_BYTES <= MAX_FILE_BYTES) };
    let plan = build_plan(
        "Shots",
        vec![],
        vec![file("big.png", MAX_BATCH_BYTES)],
        &people(),
    );
    let report = run(&dc, &plan.messages, 0, &Pacing::default()).unwrap();
    assert_eq!(report.sent, 2);
    assert_eq!(dc.messages()[1].files[0].size, MAX_BATCH_BYTES);
}

#[test]
fn a_failed_message_returns_the_progress_and_a_resume_sends_only_the_rest() {
    let dc = FakeDiscord::start();
    let messages = three_messages();
    assert_eq!(messages.len(), 3);
    // The first message goes through, then the server errs once.
    let hook = hook_of(&dc);
    let failure = send_messages(
        &client(&hook),
        &hook,
        &messages,
        0,
        &Pacing::default(),
        &mut |n, _| {
            if n == 1 {
                dc.inject(
                    Fault::Reply(Reply::json(500, &json!({"message": "boom"}))),
                    1,
                );
            }
        },
    )
    .unwrap_err();
    assert_eq!(failure.sent, 1);
    assert_eq!(failure.error.status, Some(500));
    assert_eq!(
        failure.error.failure,
        Failure::Transient {
            retry_after_ms: None
        }
    );
    assert_eq!(dc.messages().len(), 1);

    // Resume from the stored count: no duplicate of the first message.
    let report = run(&dc, &messages, failure.sent, &Pacing::default()).unwrap();
    assert_eq!(report.sent, 3);
    assert_eq!(report.message_ids.len(), 2);
    let stored = dc.messages();
    assert_eq!(stored.len(), 3);
    assert!(stored[0].content.starts_with('a'));
    assert!(stored[1].content.starts_with('b'));
    assert!(stored[2].content.starts_with('c'));

    // A resume point at or past the end sends nothing.
    let none = run(&dc, &messages, 3, &Pacing::default()).unwrap();
    assert_eq!((none.sent, none.message_ids.len()), (3, 0));
    assert_eq!(dc.messages().len(), 3);
}

#[test]
fn a_short_rate_limit_is_slept_through_in_place() {
    let dc = FakeDiscord::start();
    let messages = three_messages();
    dc.inject(Fault::Reply(rate_limited(2.0)), 1);
    let sleeps = Sleeps::default();
    let report = run(&dc, &messages, 0, &sleeps.pacing(5, 2)).unwrap();
    assert_eq!(report.sent, 3);
    assert_eq!(sleeps.all(), vec![Duration::from_secs(2)]);
    assert_eq!(dc.messages().len(), 3);
}

#[test]
fn a_long_rate_limit_is_handed_back_with_the_wait_and_nothing_is_slept() {
    let dc = FakeDiscord::start();
    let messages = three_messages();
    dc.inject(Fault::Reply(rate_limited(60.0)), 1);
    let sleeps = Sleeps::default();
    let err = run(&dc, &messages, 0, &sleeps.pacing(5, 2)).unwrap_err();
    assert_eq!(err.sent, 0);
    assert_eq!(err.error.kind, ErrorKind::RateLimited);
    assert_eq!(
        err.error.failure,
        Failure::Transient {
            retry_after_ms: Some(60_000)
        }
    );
    assert!(sleeps.all().is_empty());
    assert!(dc.messages().is_empty());
}

#[test]
fn rate_limit_retries_in_place_are_bounded() {
    let dc = FakeDiscord::start();
    let messages = three_messages();
    dc.inject(Fault::Reply(rate_limited(1.0)), 10);
    let sleeps = Sleeps::default();
    let err = run(&dc, &messages, 0, &sleeps.pacing(5, 2)).unwrap_err();
    assert_eq!(err.sent, 0);
    assert_eq!(sleeps.all().len(), 2);
    assert_eq!(dc.request_count(), 3);
}

#[test]
fn an_exhausted_bucket_is_waited_out_between_messages() {
    let server = FakeServer::start(|_| {
        Reply::json(200, &json!({"id": "1"}))
            .with_header("x-ratelimit-remaining", "0")
            .with_header("x-ratelimit-reset-after", "0.5")
    });
    let url = format!(
        "{}/api/webhooks/100000000000000001/tok_ABCDEFGH",
        server.base_url()
    );
    let hook = Webhook::parse(&url).unwrap();
    let sleeps = Sleeps::default();
    let report = send_messages(
        &client(&hook),
        &hook,
        &three_messages(),
        0,
        &sleeps.pacing(5, 2),
        &mut |_, _| {},
    )
    .unwrap();
    assert_eq!(report.sent, 3);
    // Between the three messages, not after the last.
    assert_eq!(sleeps.all(), vec![Duration::from_millis(500); 2]);
}

#[test]
fn an_offline_host_is_a_transient_failure_with_no_progress() {
    let dc = FakeDiscord::start();
    let messages = three_messages();
    dc.go_offline();
    let err = run(&dc, &messages, 0, &Pacing::default()).unwrap_err();
    assert!(err.error.is_offline(), "{:?}", err.error);
    assert_eq!(err.sent, 0);
    assert!(matches!(err.error.failure, Failure::Transient { .. }));
    dc.go_online();
    let report = run(&dc, &messages, err.sent, &Pacing::default()).unwrap();
    assert_eq!(report.sent, 3);
}

#[test]
fn a_deleted_webhook_is_permanent_and_the_error_never_shows_the_secret() {
    let dc = FakeDiscord::start();
    dc.delete_webhook();
    let err = run(&dc, &three_messages(), 0, &Pacing::default()).unwrap_err();
    assert_eq!(err.error.status, Some(404));
    assert_eq!(err.error.failure, Failure::Permanent);
    let text = format!("{err} {:?}", err.error);
    assert!(!text.contains(&dc.webhook_token()), "{text}");
    assert!(!text.contains(&dc.webhook_url()), "{text}");
}

#[test]
fn an_error_body_that_echoes_the_webhook_is_redacted() {
    let token = "Secret_token-ABC123";
    let server = FakeServer::start(move |req| {
        Reply::json(
            400,
            &json!({"message": format!("bad request to {} with {token}", req.path)}),
        )
    });
    let url = format!(
        "{}/api/webhooks/100000000000000001/{token}",
        server.base_url()
    );
    let hook = Webhook::parse(&url).unwrap();
    let err = send_messages(
        &client(&hook),
        &hook,
        &three_messages(),
        0,
        &Pacing::default(),
        &mut |_, _| {},
    )
    .unwrap_err();
    let text = format!("{err} {:?}", err.error);
    assert!(!text.contains(token), "{text}");
    assert_eq!(err.error.failure, Failure::Permanent);
}

#[test]
fn a_redirect_is_refused_and_not_followed() {
    let server = FakeServer::start(|_| {
        Reply::empty(307).with_header("Location", "https://elsewhere.example/steal")
    });
    let url = format!(
        "{}/api/webhooks/100000000000000001/tok_ABCDEFGH",
        server.base_url()
    );
    let hook = Webhook::parse(&url).unwrap();
    let err = send_messages(
        &client(&hook),
        &hook,
        &three_messages(),
        0,
        &Pacing::default(),
        &mut |_, _| {},
    )
    .unwrap_err();
    assert_eq!(err.error.kind, ErrorKind::Redirect);
    assert_eq!(server.request_count(), 1);
}
