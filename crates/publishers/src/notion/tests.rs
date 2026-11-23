use std::time::Duration;

use cycle_store::{ItemType, NotionProperties, PublishMode};
use serde_json::{json, Value};

use super::*;
use crate::export::{upsert, UpsertOutcome};
use crate::testing::fixtures::{cycle, draft, item, task};
use crate::testing::notion::{
    conflict, rate_limited, FakeNotion, DATABASE_ID, DATA_SOURCE_ID, DEFAULT_TOKEN,
};
use crate::testing::{FakeServer, Fault, Reply};

const MEETING: &str = "20261113-140501";
const REPORT: &str =
    "https://github.test/owner/rover/blob/main/meetings/NOMAD/2026-11-13/report.md";

fn client() -> HttpClient {
    HttpClient::with_timeouts(Duration::from_secs(2), Duration::from_secs(5))
}

fn notion(fake: &FakeNotion) -> Notion {
    Notion::with_client(client(), &fake.api_base(), &fake.token()).unwrap()
}

fn record(id: &str) -> TaskRecord {
    let c = cycle(Some(PublishMode::OnApproval), None);
    let it = task(id, "Order the battery", Some("alex"), Some("2026-11-20"));
    let d = draft(MEETING, vec![it.clone()], vec![]);
    let mut r = TaskRecord::from_item(&c, &d, &it, MEETING);
    r.report_url = Some(REPORT.into());
    r
}

fn full_map() -> NotionProperties {
    NotionProperties {
        assignee: Some("Assignee".into()),
        due: Some("Due".into()),
        status: Some("Status".into()),
        status_value: Some("Not started".into()),
        meeting: Some("Meeting".into()),
        cycle: Some("Cycle".into()),
        ..NotionProperties::default()
    }
}

fn schema(properties: &[(&str, &str)]) -> Schema {
    properties
        .iter()
        .map(|(n, t)| ((*n).to_string(), PropertyType::from_api(t)))
        .collect()
}

fn default_schema() -> Schema {
    schema(&[
        ("Name", "title"),
        ("BBB key", "rich_text"),
        ("Assignee", "select"),
        ("Due", "date"),
        ("Status", "status"),
        ("Meeting", "url"),
        ("Cycle", "rich_text"),
    ])
}

// ===== the field mapping (pure) ==========================================================

mod mapping {
    use super::*;

    fn values(map: &NotionProperties, s: &Schema, t: &TaskRecord, create: bool) -> Value {
        Value::Object(property_values(map, s, t, create).unwrap())
    }

    #[test]
    fn a_new_page_gets_every_mapped_field_in_the_shape_of_its_type() {
        let got = values(&full_map(), &default_schema(), &record("i1"), true);
        assert_eq!(
            got,
            json!({
                "Name": {"title": [{"text": {"content": "Order the battery"}}]},
                "BBB key": {"rich_text": [{"text": {"content": "bbb:v1:NOMAD:20261113-140501:i1"}}]},
                "Assignee": {"select": {"name": "Alex Example"}},
                "Due": {"date": {"start": "2026-11-20"}},
                "Status": {"status": {"name": "Not started"}},
                "Meeting": {"url": REPORT},
                "Cycle": {"rich_text": [{"text": {"content": "Nomad rover"}}]},
            })
        );
    }

    #[test]
    fn only_the_title_and_the_key_are_written_when_nothing_else_is_mapped() {
        let got = values(
            &NotionProperties::default(),
            &default_schema(),
            &record("i1"),
            true,
        );
        let keys: Vec<&String> = got.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["BBB key", "Name"]);
    }

    #[test]
    fn other_types_take_a_text_field_in_their_own_shape() {
        let s = schema(&[
            ("Name", "title"),
            ("BBB key", "rich_text"),
            ("Assignee", "multi_select"),
            ("Due", "date"),
            ("Status", "select"),
            ("Meeting", "rich_text"),
            ("Cycle", "select"),
        ]);
        let got = values(&full_map(), &s, &record("i1"), true);
        assert_eq!(
            got["Assignee"],
            json!({"multi_select": [{"name": "Alex Example"}]})
        );
        assert_eq!(got["Status"], json!({"select": {"name": "Not started"}}));
        assert_eq!(
            got["Meeting"],
            json!({"rich_text": [{"text": {"content": format!("2026-11-13 {REPORT}")}}]})
        );
        assert_eq!(got["Cycle"], json!({"select": {"name": "Nomad rover"}}));

        let s = schema(&[
            ("Name", "title"),
            ("BBB key", "rich_text"),
            ("Assignee", "rich_text"),
        ]);
        let map = NotionProperties {
            assignee: Some("Assignee".into()),
            ..NotionProperties::default()
        };
        assert_eq!(
            values(&map, &s, &record("i1"), true)["Assignee"],
            json!({"rich_text": [{"text": {"content": "Alex Example"}}]})
        );
    }

    #[test]
    fn a_text_is_cut_at_2000_characters_on_a_character_boundary() {
        let mut t = record("i1");
        t.title = "ł".repeat(2100);
        let got = values(&NotionProperties::default(), &default_schema(), &t, true);
        let content = got["Name"]["title"][0]["text"]["content"].as_str().unwrap();
        assert_eq!(content.chars().count(), 2000);
        assert!(content.chars().all(|c| c == 'ł'));

        // Characters outside the BMP count twice in UTF-16: never above the limit either way.
        t.title = "😀".repeat(1500);
        let got = values(&NotionProperties::default(), &default_schema(), &t, true);
        let content = got["Name"]["title"][0]["text"]["content"].as_str().unwrap();
        assert_eq!(content.chars().count(), 1000);
        assert!(content.encode_utf16().count() <= 2000);

        // A text within the limit is kept whole.
        t.title = "x".repeat(2000);
        let got = values(&NotionProperties::default(), &default_schema(), &t, true);
        assert_eq!(
            got["Name"]["title"][0]["text"]["content"]
                .as_str()
                .unwrap()
                .len(),
            2000
        );
    }

    #[test]
    fn a_title_is_one_line() {
        let mut t = record("i1");
        t.title = "Order the\nbattery \t now".into();
        let got = values(&NotionProperties::default(), &default_schema(), &t, true);
        assert_eq!(
            got["Name"]["title"][0]["text"]["content"],
            "Order the battery now"
        );
    }

    #[test]
    fn a_select_name_has_no_comma_and_at_most_100_characters() {
        assert_eq!(
            option_name("Alex, Sam and Kim").unwrap(),
            "Alex Sam and Kim"
        );
        assert_eq!(option_name("  a,,b  ").unwrap(), "a b");
        assert_eq!(option_name(&"é".repeat(150)).unwrap().chars().count(), 100);
        assert_eq!(option_name(" , "), None);

        let mut t = record("i1");
        t.assignee.as_mut().unwrap().name = "Alex, the Great".into();
        let got = values(&full_map(), &default_schema(), &t, true);
        assert_eq!(
            got["Assignee"],
            json!({"select": {"name": "Alex the Great"}})
        );
    }

    #[test]
    fn a_field_without_a_value_is_left_out_on_create_and_emptied_on_update() {
        let mut t = record("i1");
        t.assignee = None;
        t.due = None;
        t.report_url = None;
        let created = values(&full_map(), &default_schema(), &t, true);
        let created = created.as_object().unwrap();
        for absent in ["Assignee", "Due", "Meeting"] {
            assert!(!created.contains_key(absent), "{absent}");
        }
        assert!(created.contains_key("Cycle") && created.contains_key("Name"));

        let updated = values(&full_map(), &default_schema(), &t, false);
        assert_eq!(updated["Assignee"], json!({"select": null}));
        assert_eq!(updated["Due"], json!({"date": null}));
        assert_eq!(updated["Meeting"], json!({"url": null}));

        let s = schema(&[
            ("Name", "title"),
            ("BBB key", "rich_text"),
            ("Assignee", "multi_select"),
            ("Due", "date"),
        ]);
        let map = NotionProperties {
            assignee: Some("Assignee".into()),
            due: Some("Due".into()),
            ..NotionProperties::default()
        };
        let updated = values(&map, &s, &t, false);
        assert_eq!(updated["Assignee"], json!({"multi_select": []}));
        assert_eq!(updated["Due"], json!({"date": null}));
    }

    #[test]
    fn the_status_is_set_on_create_only() {
        let created = values(&full_map(), &default_schema(), &record("i1"), true);
        assert!(created.as_object().unwrap().contains_key("Status"));
        let updated = values(&full_map(), &default_schema(), &record("i1"), false);
        assert!(
            !updated.as_object().unwrap().contains_key("Status"),
            "an export again never resets what a person moved"
        );
        // The key stays in the update: it is what the page is found by.
        assert!(updated.as_object().unwrap().contains_key("BBB key"));
    }

    #[test]
    fn problems_are_found_before_anything_is_written() {
        let ok = check(&full_map(), &default_schema());
        assert!(ok.is_empty(), "{ok:?}");

        let missing = NotionProperties {
            due: Some("Deadline".into()),
            ..NotionProperties::default()
        };
        let problems = check(&missing, &default_schema());
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].field, "due");
        assert!(problems[0].message.contains("\"Deadline\" does not exist"));

        let wrong = NotionProperties {
            assignee: Some("Due".into()),
            due: Some("Assignee".into()),
            ..NotionProperties::default()
        };
        let fields: Vec<&str> = check(&wrong, &default_schema())
            .iter()
            .map(|p| p.field)
            .collect();
        assert_eq!(fields, ["assignee", "due"]);

        let key_not_text = schema(&[("Name", "title"), ("BBB key", "number")]);
        let problems = check(&NotionProperties::default(), &key_not_text);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].field, "key");
        assert!(problems[0].message.contains("rich_text"));

        let title_not_title = schema(&[("Name", "rich_text"), ("BBB key", "rich_text")]);
        let problems = check(&NotionProperties::default(), &title_not_title);
        assert_eq!(problems[0].field, "title");

        let no_value = NotionProperties {
            status: Some("Status".into()),
            ..NotionProperties::default()
        };
        let problems = check(&no_value, &default_schema());
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].field, "status_value");

        let twice = NotionProperties {
            cycle: Some("BBB key".into()),
            ..NotionProperties::default()
        };
        let problems = check(&twice, &default_schema());
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].field, "cycle");
        assert!(problems[0].message.contains("used for the key as well"));

        // `property_values` refuses a mapping that has problems and says which.
        let err = property_values(&missing, &default_schema(), &record("i1"), true).unwrap_err();
        assert!(err.to_string().contains("Deadline"), "{err}");
    }

    #[test]
    fn a_people_property_is_not_a_target() {
        let s = schema(&[
            ("Name", "title"),
            ("BBB key", "rich_text"),
            ("Who", "people"),
        ]);
        let map = NotionProperties {
            assignee: Some("Who".into()),
            ..NotionProperties::default()
        };
        let problems = check(&map, &s);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].message.contains("type people"));
    }

    #[test]
    fn a_mapping_is_proposed_from_names_and_types() {
        let proposed = propose(&default_schema());
        assert_eq!(proposed.title, "Name");
        assert_eq!(proposed.key, "BBB key");
        assert_eq!(proposed.assignee.as_deref(), Some("Assignee"));
        assert_eq!(proposed.due.as_deref(), Some("Due"));
        assert_eq!(proposed.meeting.as_deref(), Some("Meeting"));
        assert_eq!(proposed.cycle.as_deref(), Some("Cycle"));
        // The starting status needs the options.
        assert_eq!(
            (proposed.status.as_deref(), proposed.status_value),
            (None, None)
        );

        let mut options = Options::new();
        options.insert(
            "Status".into(),
            vec!["In progress".into(), "Not started".into(), "Done".into()],
        );
        let proposed = propose_with_options(&default_schema(), &options);
        assert_eq!(proposed.status.as_deref(), Some("Status"));
        assert_eq!(proposed.status_value.as_deref(), Some("Not started"));
        assert!(check(&proposed, &default_schema()).is_empty());
    }

    #[test]
    fn a_proposal_ignores_case_and_wrong_types_and_keeps_defaults() {
        let s = schema(&[
            ("Task", "title"),
            ("key", "rich_text"),
            ("assigned TO", "multi_select"),
            ("DUE DATE", "date"),
            ("Due", "number"),
            ("Owner", "people"),
            ("Link", "url"),
        ]);
        let p = propose(&s);
        assert_eq!(p.title, "Task");
        assert_eq!(p.key, "key");
        assert_eq!(p.assignee.as_deref(), Some("assigned TO"));
        assert_eq!(
            p.due.as_deref(),
            Some("DUE DATE"),
            "the number is not a date"
        );
        assert_eq!(p.meeting, None);
        assert_eq!(p.cycle, None);

        // No key property: the default name is proposed (and `check` will say it is missing).
        let bare = schema(&[("Name", "title")]);
        let p = propose(&bare);
        assert_eq!(p.key, "BBB key");
        assert_eq!(check(&p, &bare).len(), 1);

        // A property serves one field only.
        let one = schema(&[
            ("Name", "title"),
            ("Key", "rich_text"),
            ("Cycle", "rich_text"),
        ]);
        let p = propose(&one);
        assert_eq!(p.key, "Key");
        assert_eq!(p.cycle.as_deref(), Some("Cycle"));
        let p = propose(&schema(&[("Name", "title"), ("Key", "rich_text")]));
        assert_eq!(p.cycle, None);
    }

    #[test]
    fn a_schema_is_read_from_the_properties_of_a_data_source() {
        let read = DataSourceSchema::from_properties(&json!({
            "Name": {"id": "title", "name": "Name", "type": "title", "title": {}},
            "Status": {"type": "status", "status": {"options": [{"name": "Open"}, {"name": "Done"}]}},
            "Tags": {"type": "multi_select", "multi_select": {"options": [{"name": "a"}]}},
            "Broken": {"name": "no type"},
            "Odd": {"type": "formula", "formula": {}},
        }));
        assert_eq!(read.properties.len(), 4);
        assert_eq!(read.properties["Name"], PropertyType::Title);
        assert_eq!(
            read.properties["Odd"],
            PropertyType::Other("formula".into())
        );
        assert_eq!(read.options["Status"], ["Open", "Done"]);
        assert_eq!(read.options["Tags"], ["a"]);
        assert!(DataSourceSchema::from_properties(&json!(null))
            .properties
            .is_empty());
        assert_eq!(PropertyType::from_api("date").api_name(), "date");
        assert_eq!(PropertyType::Other("formula".into()).api_name(), "formula");
    }

    #[test]
    fn a_task_without_an_item_type_matters_not() {
        // The mapping looks at the record only: an item of any type that reached it.
        let c = cycle(None, None);
        let it = item("i9", ItemType::Task, "Plain");
        let d = draft(MEETING, vec![it.clone()], vec![]);
        let r = TaskRecord::from_item(&c, &d, &it, MEETING);
        let got = values(&full_map(), &default_schema(), &r, true);
        let keys: Vec<&str> = got
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert!(!keys.contains(&"Assignee") && !keys.contains(&"Due"));
        assert!(keys.contains(&"Cycle") && keys.contains(&"Status"));
    }
}

// ===== the API base and the headers ======================================================

#[test]
fn the_api_base_is_the_notion_api_or_this_machine() {
    assert_eq!(
        normalize_api_base("https://api.notion.com/v1/").unwrap(),
        DEFAULT_API
    );
    assert_eq!(
        normalize_api_base(" https://API.notion.com:443/v1 ").unwrap(),
        "https://API.notion.com:443/v1"
    );
    assert!(normalize_api_base("http://127.0.0.1:8080/v1").is_ok());
    assert!(normalize_api_base("http://localhost:1/anything").is_ok());
    for bad in [
        "http://api.notion.com/v1",
        "https://api.notion.com",
        "https://api.notion.com/v2",
        "https://api.notion.com/v1/pages",
        "https://notion.example.com/v1",
        "https://api.notion.com.evil.example/v1",
        "https://user:pw@api.notion.com/v1",
        "https://api.notion.com:8443/v1",
        "https://api.notion.com/v1?x=1",
        "api.notion.com/v1",
        "",
    ] {
        assert!(normalize_api_base(bad).is_err(), "{bad}");
    }
    assert!(Notion::new("https://evil.example/v1", "ntn_x").is_err());
    assert_eq!(NOTION_VERSION, "2026-03-11");
    assert_eq!(DEFAULT_API, "https://api.notion.com/v1");
}

#[test]
fn every_request_carries_the_token_the_version_and_json() {
    let fake = FakeNotion::start();
    let api = notion(&fake);
    api.current_user().unwrap();
    api.database(DATABASE_ID).unwrap();
    api.data_source_schema(DATA_SOURCE_ID).unwrap();
    let schema = api.data_source_schema(DATA_SOURCE_ID).unwrap().properties;
    let map = NotionProperties::default();
    let properties = property_values(&map, &schema, &record("i1"), true).unwrap();
    let page = api.create_page(DATA_SOURCE_ID, &properties).unwrap();
    api.get_page(&page.id).unwrap();
    api.update_page(&page.id, &properties).unwrap();
    api.query_by_key(DATA_SOURCE_ID, "BBB key", "x").unwrap();

    let requests = fake.requests();
    assert_eq!(requests.len(), 9);
    for r in &requests {
        assert_eq!(
            r.header("authorization"),
            Some(format!("Bearer {DEFAULT_TOKEN}").as_str()),
            "{} {}",
            r.method,
            r.path
        );
        assert_eq!(r.header("notion-version"), Some("2026-03-11"));
        assert_eq!(r.header("content-type"), Some("application/json"));
        assert!(r.path.starts_with("/v1/"), "{}", r.path);
    }
    let paths: Vec<String> = requests
        .iter()
        .map(|r| format!("{} {}", r.method, r.path))
        .collect();
    assert_eq!(paths[0], "GET /v1/users/me");
    assert_eq!(paths[1], format!("GET /v1/databases/{DATABASE_ID}"));
    assert!(paths[2].starts_with("GET /v1/data_sources/fedcba9876543210fedcba9876543210"));
    assert!(paths[4].starts_with("POST /v1/pages"));
    assert!(paths[7].ends_with("/query"));
}

#[test]
fn a_request_without_the_version_is_refused_by_the_fake() {
    let fake = FakeNotion::start();
    let request = Request::get(format!("{}/users/me", fake.api_base())).bearer(DEFAULT_TOKEN);
    let answer = client().send_raw(&request).unwrap();
    assert_eq!(answer.status, 400);
    assert_eq!(answer.json().unwrap()["code"], "missing_version");
    let request = request.header("Notion-Version", "2022-06-28");
    assert_eq!(client().send_raw(&request).unwrap().status, 400);
}

#[test]
fn the_integration_is_read_from_users_me() {
    let fake = FakeNotion::start();
    let me = notion(&fake).current_user().unwrap();
    assert_eq!(me.name.as_deref(), Some("Blah Blah Bot test"));
    assert_eq!(me.workspace.as_deref(), Some("Nomad workspace"));
    assert!(!me.id.is_empty());
}

// ===== errors =============================================================================

fn response(status: u16, body: &str) -> Response {
    Response {
        status,
        headers: vec![],
        body: body.as_bytes().to_vec(),
    }
}

fn classified(status: u16, body: &str) -> HttpError {
    let r = response(status, body);
    let base = crate::http::classify_status(&r, 0).unwrap();
    notion_error(base, &r)
}

#[test]
fn errors_are_classified_by_status_and_code() {
    let e = classified(
        409,
        r#"{"code":"conflict_error","message":"Conflict occurred"}"#,
    );
    assert_eq!(
        e.failure,
        store::Failure::Transient {
            retry_after_ms: None
        }
    );
    assert_eq!(e.status, Some(409));

    let e = classified(
        404,
        r#"{"code":"object_not_found","message":"Could not find database"}"#,
    );
    assert_eq!(e.failure, store::Failure::Permanent);
    assert!(
        e.message.contains("not shared with the integration"),
        "{}",
        e.message
    );
    assert!(e.message.contains("Connections"), "{}", e.message);

    let e = classified(
        401,
        r#"{"code":"unauthorized","message":"API token is invalid."}"#,
    );
    assert_eq!(e.failure, store::Failure::Permanent);
    assert_eq!(e.message, "Notion rejected the token");

    let e = classified(
        400,
        r#"{"code":"validation_error","message":"Assignee is expected to be select."}"#,
    );
    assert_eq!(e.failure, store::Failure::Permanent);
    assert_eq!(
        e.message,
        "Notion refused the values: Assignee is expected to be select."
    );

    let e = classified(403, r#"{"code":"restricted_resource","message":"x"}"#);
    assert_eq!(e.failure, store::Failure::Permanent);
    assert!(e.message.contains("capability"));

    // Others keep the generic classification.
    let e = classified(500, r#"{"code":"internal_server_error","message":"oops"}"#);
    assert!(matches!(e.failure, store::Failure::Transient { .. }));
    assert_eq!(e.message, "HTTP 500: oops");
    let e = classified(400, "not json");
    assert_eq!(e.failure, store::Failure::Permanent);
    let e = classified(
        404,
        r#"{"code":"invalid_request_url","message":"Invalid request URL."}"#,
    );
    assert_eq!(e.message, "HTTP 404: Invalid request URL.");
}

#[test]
fn a_wrong_token_is_a_permanent_error_and_never_appears_in_it() {
    let fake = FakeNotion::start();
    let token = concat!("ntn_", "WrongTokenValue0123456789abcdefghij");
    let api = Notion::with_client(client(), &fake.api_base(), token).unwrap();
    let err = api.current_user().unwrap_err();
    assert_eq!(err.status, Some(401));
    assert_eq!(err.failure(), store::Failure::Permanent);
    assert_eq!(err.message, "Notion rejected the token");
    assert!(!err.message.contains(token));
}

#[test]
fn the_message_of_a_failing_answer_is_redacted() {
    let token = concat!("ntn_", "EchoedTokenValue0123456789abcdefghij");
    let server = FakeServer::start(move |_| {
        Reply::json(
            400,
            &json!({"code": "invalid_request", "message": format!("bad header Bearer {token} sent")}),
        )
    });
    let api = Notion::with_client(client(), &format!("{}/v1", server.base_url()), token).unwrap();
    let err = api.current_user().unwrap_err();
    assert!(err.message.contains("bad header"), "{}", err.message);
    assert!(!err.message.contains(token), "{}", err.message);
}

#[test]
fn a_conflict_is_transient_and_a_retry_succeeds() {
    let fake = FakeNotion::start();
    let api = notion(&fake);
    fake.inject_for("POST", "/v1/pages", Fault::Reply(conflict()), 1);
    let schema = api.data_source_schema(DATA_SOURCE_ID).unwrap().properties;
    let properties =
        property_values(&NotionProperties::default(), &schema, &record("i1"), true).unwrap();
    let err = api.create_page(DATA_SOURCE_ID, &properties).unwrap_err();
    assert_eq!(
        err.failure(),
        store::Failure::Transient {
            retry_after_ms: None
        }
    );
    assert!(fake.pages().is_empty());
    api.create_page(DATA_SOURCE_ID, &properties).unwrap();
    assert_eq!(fake.pages().len(), 1);
}

#[test]
fn a_rate_limit_names_the_wait() {
    let fake = FakeNotion::start();
    let api = notion(&fake);
    fake.inject(Fault::Reply(rate_limited(7)), 1);
    let err = api.current_user().unwrap_err();
    assert_eq!(err.kind, ErrorKind::RateLimited);
    assert_eq!(
        err.failure(),
        store::Failure::Transient {
            retry_after_ms: Some(7000)
        }
    );
    api.current_user().unwrap();
}

#[test]
fn a_value_notion_refuses_is_a_permanent_error_that_names_the_property() {
    let fake = FakeNotion::start();
    let api = notion(&fake);
    let mut properties = serde_json::Map::new();
    properties.insert("Nope".into(), json!({"select": {"name": "x"}}));
    let err = api.create_page(DATA_SOURCE_ID, &properties).unwrap_err();
    assert_eq!(err.failure(), store::Failure::Permanent);
    assert!(
        err.message.contains("Nope is not a property that exists"),
        "{}",
        err.message
    );

    let mut properties = serde_json::Map::new();
    properties.insert("Assignee".into(), json!({"rich_text": []}));
    let err = api.create_page(DATA_SOURCE_ID, &properties).unwrap_err();
    assert!(
        err.message.contains("Assignee is expected to be select"),
        "{}",
        err.message
    );

    // The fake enforces the limits the mapping respects.
    let mut properties = serde_json::Map::new();
    properties.insert(
        "Name".into(),
        json!({"title": [{"text": {"content": "x".repeat(2001)}}]}),
    );
    let err = api.create_page(DATA_SOURCE_ID, &properties).unwrap_err();
    assert!(err.message.contains("Name"), "{}", err.message);
    assert!(err.message.contains("2000"), "{}", err.message);
    let mut properties = serde_json::Map::new();
    properties.insert("Assignee".into(), json!({"select": {"name": "a, b"}}));
    let err = api.create_page(DATA_SOURCE_ID, &properties).unwrap_err();
    assert!(err.message.contains("commas"), "{}", err.message);
    let mut properties = serde_json::Map::new();
    properties.insert("Status".into(), json!({"status": {"name": "Nonsense"}}));
    let err = api.create_page(DATA_SOURCE_ID, &properties).unwrap_err();
    assert!(err.message.contains("Nonsense"), "{}", err.message);
    assert!(fake.pages().is_empty());
}

#[test]
fn a_database_that_is_not_shared_gives_the_hint() {
    let fake = FakeNotion::start();
    fake.share(false);
    let api = notion(&fake);
    api.current_user().unwrap();
    let err = api.database(DATABASE_ID).unwrap_err();
    assert_eq!(err.status, Some(404));
    assert_eq!(err.failure(), store::Failure::Permanent);
    assert!(err.message.contains("Connections"), "{}", err.message);
    let err = api.data_source_schema(DATA_SOURCE_ID).unwrap_err();
    assert!(err.message.contains("not shared"), "{}", err.message);
    let map = NotionProperties::default();
    assert!(NotionTasks::prepare(&api, DATABASE_ID, None, &map).is_err());
    assert_eq!(
        api.get_page("c0ffee00-0000-4000-8000-000000000001")
            .unwrap(),
        None
    );

    fake.share(true);
    api.database(DATABASE_ID).unwrap();
}

#[test]
fn a_malformed_id_is_refused_before_any_request() {
    let fake = FakeNotion::start();
    let api = notion(&fake);
    for bad in ["", "not-an-id", "../users/me", "0123"] {
        let err = api.database(bad).unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidRequest, "{bad}");
        assert!(api.get_page(bad).is_err());
    }
    assert_eq!(fake.request_count(), 0);
}

// ===== data sources ======================================================================

#[test]
fn the_only_data_source_is_used_and_remembered() {
    let fake = FakeNotion::start();
    let api = notion(&fake);
    let info = api.database(DATABASE_ID).unwrap();
    assert_eq!(info.title, "Tasks");
    assert_eq!(info.data_sources.len(), 1);
    fake.clear_requests();

    let id = api.resolve_data_source(DATABASE_ID, None).unwrap();
    assert_eq!(id, "fedcba9876543210fedcba9876543210");
    assert_eq!(api.resolve_data_source(DATABASE_ID, None).unwrap(), id);
    assert_eq!(fake.request_count(), 1, "the answer is kept");
}

#[test]
fn a_configured_data_source_needs_no_request() {
    let fake = FakeNotion::start();
    let api = notion(&fake);
    let second = fake.add_data_source("Archive");
    let id = api
        .resolve_data_source(DATABASE_ID, Some(&second.to_uppercase()))
        .unwrap();
    assert_eq!(id, second.replace('-', ""));
    assert_eq!(fake.request_count(), 0);
    // A blank setting counts as none.
    let err = api
        .resolve_data_source(DATABASE_ID, Some("  "))
        .unwrap_err();
    assert!(err.message.contains("2 data sources"), "{}", err.message);
}

#[test]
fn several_or_no_data_sources_ask_the_user_to_choose() {
    let fake = FakeNotion::start();
    let api = notion(&fake);
    fake.add_data_source("Archive");
    let err = api.resolve_data_source(DATABASE_ID, None).unwrap_err();
    assert_eq!(err.failure(), store::Failure::Permanent);
    assert!(err.message.contains("2 data sources"), "{}", err.message);
    assert!(err.message.contains("Notion mapping"), "{}", err.message);

    let empty = FakeNotion::start();
    empty.remove_data_source(DATA_SOURCE_ID);
    let api = notion(&empty);
    let err = api.resolve_data_source(DATABASE_ID, None).unwrap_err();
    assert_eq!(err.failure(), store::Failure::Permanent);
    assert!(err.message.contains("no data source"), "{}", err.message);
}

#[test]
fn the_schema_is_read_with_the_options() {
    let fake = FakeNotion::start();
    let read = notion(&fake).data_source_schema(DATA_SOURCE_ID).unwrap();
    assert_eq!(read.properties, default_schema());
    assert_eq!(
        read.options["Status"],
        ["Not started", "In progress", "Done"]
    );
    assert!(check(&propose(&read.properties), &read.properties).is_empty());
}

// ===== exporting tasks ===================================================================

fn prepared<'a>(api: &'a Notion, map: &'a NotionProperties) -> NotionTasks<'a> {
    NotionTasks::prepare(api, DATABASE_ID, None, map).unwrap()
}

#[test]
fn a_task_becomes_one_page_with_the_mapped_values_and_the_key() {
    let fake = FakeNotion::start();
    let api = notion(&fake);
    let map = full_map();
    let tasks = prepared(&api, &map);
    let mut stored = Vec::new();
    let done = upsert(&tasks, &record("i1"), None, true, &mut |t| {
        stored.push(t.clone())
    })
    .unwrap();
    let UpsertOutcome::Created(page) = done.outcome else {
        panic!("{:?}", done.outcome);
    };
    assert_eq!(stored, [page.clone()], "the id is handed over at once");
    assert!(page
        .url
        .as_deref()
        .unwrap()
        .starts_with("https://www.notion.so/"));

    let pages = fake.pages();
    assert_eq!(pages.len(), 1);
    let p = &pages[0];
    assert_eq!(p.text("Name").as_deref(), Some("Order the battery"));
    assert_eq!(
        p.text("BBB key").as_deref(),
        Some("bbb:v1:NOMAD:20261113-140501:i1")
    );
    assert_eq!(p.option("Assignee").as_deref(), Some("Alex Example"));
    assert_eq!(p.date("Due").as_deref(), Some("2026-11-20"));
    assert_eq!(p.option("Status").as_deref(), Some("Not started"));
    assert_eq!(p.url_value("Meeting").as_deref(), Some(REPORT));
    assert_eq!(p.text("Cycle").as_deref(), Some("Nomad rover"));
    assert!(!p.in_trash);
}

#[test]
fn exporting_again_finds_the_page_by_its_key_and_does_not_duplicate() {
    let fake = FakeNotion::start();
    let api = notion(&fake);
    let map = full_map();
    let tasks = prepared(&api, &map);
    upsert(&tasks, &record("i1"), None, true, &mut |_| {}).unwrap();
    upsert(&tasks, &record("i2"), None, true, &mut |_| {}).unwrap();

    let mut found = Vec::new();
    let again = upsert(&tasks, &record("i1"), None, true, &mut |t| {
        found.push(t.id.clone())
    })
    .unwrap();
    assert!(again.found_by_key);
    assert!(matches!(again.outcome, UpsertOutcome::Updated(_)));
    assert_eq!(found.len(), 1);
    assert_eq!(fake.pages().len(), 2, "one page per task");
}

#[test]
fn an_update_changes_the_values_but_never_the_status_a_person_set() {
    let fake = FakeNotion::start();
    let api = notion(&fake);
    let map = full_map();
    let tasks = prepared(&api, &map);
    let created = upsert(&tasks, &record("i1"), None, true, &mut |_| {}).unwrap();
    let id = created.outcome.id().to_string();

    fake.set_property(&id, "Status", json!({"status": {"name": "Done"}}));
    let mut changed = record("i1");
    changed.title = "Order the battery pack".into();
    changed.due = None;
    changed.assignee = None;
    let done = upsert(&tasks, &changed, Some(&id), true, &mut |_| {}).unwrap();
    assert!(matches!(done.outcome, UpsertOutcome::Updated(_)));
    assert!(!done.found_by_key);

    let pages = fake.pages();
    assert_eq!(pages.len(), 1);
    let p = &pages[0];
    assert_eq!(p.text("Name").as_deref(), Some("Order the battery pack"));
    assert_eq!(p.option("Status").as_deref(), Some("Done"), "status kept");
    assert_eq!(
        p.date("Due"),
        None,
        "a cleared due date clears the property"
    );
    assert_eq!(p.option("Assignee"), None);
    assert_eq!(
        p.text("BBB key").as_deref(),
        Some("bbb:v1:NOMAD:20261113-140501:i1")
    );
}

#[test]
fn a_page_in_the_trash_is_removed_not_recreated() {
    let fake = FakeNotion::start();
    let api = notion(&fake);
    let map = full_map();
    let tasks = prepared(&api, &map);
    let created = upsert(&tasks, &record("i1"), None, true, &mut |_| {}).unwrap();
    let id = created.outcome.id().to_string();
    fake.trash(&id);

    let done = upsert(&tasks, &record("i1"), Some(&id), true, &mut |_| {}).unwrap();
    assert_eq!(done.outcome, UpsertOutcome::Removed { id: id.clone() });
    assert_eq!(fake.pages().len(), 1);
    assert!(fake.pages()[0].in_trash);
    // The page itself says so.
    assert!(api.get_page(&id).unwrap().unwrap().in_trash);
    // Trash cannot be edited, and a query does not list it.
    let err = tasks.update(&id, &record("i1")).unwrap_err();
    assert_eq!(err.failure(), store::Failure::Permanent);
    assert!(tasks.find(&record("i1").key).unwrap().is_empty());
}

#[test]
fn a_page_that_is_gone_is_a_permanent_error_not_a_new_page() {
    let fake = FakeNotion::start();
    let api = notion(&fake);
    let map = full_map();
    let tasks = prepared(&api, &map);
    let err = upsert(
        &tasks,
        &record("i1"),
        Some("c0ffee00-0000-4000-8000-0000000000ff"),
        true,
        &mut |_| {},
    )
    .unwrap_err();
    assert_eq!(err.failure(), store::Failure::Permanent);
    assert!(
        err.message
            .contains("page was deleted or is no longer shared"),
        "{}",
        err.message
    );
    assert!(fake.pages().is_empty());
    assert_eq!(tasks.name(), "notion");
}

#[test]
fn a_lost_answer_after_a_creation_does_not_lead_to_a_second_page() {
    let fake = FakeNotion::start();
    let api = notion(&fake);
    let map = full_map();
    let tasks = prepared(&api, &map);
    fake.inject_for("POST", "/v1/pages", Fault::DropAfter, 1);
    let err = upsert(&tasks, &record("i1"), None, true, &mut |_| {}).unwrap_err();
    assert!(err.is_offline() || matches!(err.failure(), store::Failure::Transient { .. }));
    assert_eq!(fake.pages().len(), 1, "the page was made");

    let mut found = None;
    let done = upsert(&tasks, &record("i1"), None, true, &mut |t| {
        found = Some(t.id.clone())
    })
    .unwrap();
    assert!(done.found_by_key);
    assert_eq!(found.as_deref(), Some(fake.pages()[0].id.as_str()));
    assert_eq!(fake.pages().len(), 1);
}

#[test]
fn a_query_that_trails_a_creation_is_covered_by_the_stored_id() {
    let fake = FakeNotion::start();
    fake.lag_queries(1);
    let api = notion(&fake);
    let map = full_map();
    let tasks = prepared(&api, &map);
    let created = upsert(&tasks, &record("i1"), None, true, &mut |_| {}).unwrap();
    let id = created.outcome.id().to_string();

    // With the stored id there is no query, so the lag does not matter.
    let known = upsert(&tasks, &record("i1"), Some(&id), true, &mut |_| {}).unwrap();
    assert!(matches!(known.outcome, UpsertOutcome::Updated(_)));
    assert_eq!(fake.pages().len(), 1);

    // The first query does not see the new page, the next one does.
    let key = record("i1").key;
    assert!(tasks.find(&key).unwrap().is_empty());
    let hits = tasks.find(&key).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, id);
}

#[test]
fn duplicates_are_found_oldest_first_and_counted() {
    let fake = FakeNotion::start();
    let api = notion(&fake);
    let map = full_map();
    let tasks = prepared(&api, &map);
    let first = tasks.create(&record("i1")).unwrap();
    let second = tasks.create(&record("i1")).unwrap();
    let hits = tasks.find(&record("i1").key).unwrap();
    assert_eq!(
        hits.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        [first.id.as_str(), second.id.as_str()]
    );
    let done = upsert(&tasks, &record("i1"), None, true, &mut |_| {}).unwrap();
    assert_eq!(done.outcome.id(), first.id);
    assert_eq!(done.duplicates, 1);
    assert_eq!(fake.pages().len(), 2, "nothing is deleted");
}

#[test]
fn a_mapping_that_does_not_fit_fails_before_anything_is_written() {
    let fake = FakeNotion::start();
    let api = notion(&fake);
    let map = NotionProperties {
        due: Some("Deadline".into()),
        ..NotionProperties::default()
    };
    let err = NotionTasks::prepare(&api, DATABASE_ID, None, &map)
        .err()
        .unwrap();
    assert_eq!(err.failure(), store::Failure::Permanent);
    assert!(err.message.contains("\"Deadline\""), "{}", err.message);
    assert!(fake.requests_to("POST", "/v1/pages").is_empty());
}

#[test]
fn two_data_sources_need_the_choice_and_then_work() {
    let fake = FakeNotion::start();
    let second = fake.add_data_source("Archive");
    let api = notion(&fake);
    let map = NotionProperties::default();
    let err = NotionTasks::prepare(&api, DATABASE_ID, None, &map)
        .err()
        .unwrap();
    assert!(
        err.message.contains("choose the data source"),
        "{}",
        err.message
    );
    let tasks = NotionTasks::prepare(&api, DATABASE_ID, Some(&second), &map).unwrap();
    assert_eq!(tasks.data_source(), second.replace('-', ""));
    tasks.create(&record("i1")).unwrap();
    assert_eq!(fake.pages()[0].data_source, second);
}

#[test]
fn long_and_awkward_texts_are_accepted_by_notion_after_the_cut() {
    let fake = FakeNotion::start();
    let api = notion(&fake);
    let map = full_map();
    let tasks = prepared(&api, &map);
    let mut t = record("i1");
    t.title = "ł".repeat(2500);
    t.assignee.as_mut().unwrap().name = format!("Alex, {}", "é".repeat(150));
    t.cycle_name = "Line one\nline two".into();
    let page = tasks.create(&t).unwrap();
    let stored = fake.pages().into_iter().find(|p| p.id == page.id).unwrap();
    assert_eq!(stored.text("Name").unwrap().chars().count(), 2000);
    assert!(stored.option("Assignee").unwrap().chars().count() <= 100);
    assert!(!stored.option("Assignee").unwrap().contains(','));
}

#[test]
fn notion_offline_is_an_offline_error() {
    let fake = FakeNotion::start();
    let api = notion(&fake);
    fake.go_offline();
    let err = api.current_user().unwrap_err();
    assert!(err.is_offline(), "{err:?}");
    fake.go_online();
    api.current_user().unwrap();
}
