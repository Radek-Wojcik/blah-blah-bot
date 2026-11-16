//! Contract tests for the Stage 4 documents. The fixtures under `worker/tests/fixtures` are
//! shared with the Python tests: both sides validate them against the same schemas, so a
//! change to a schema or to a type that breaks one side fails here or there.

use cycle_store::{schema, Draft, Schema};
use queue::draft::DraftFiles;
use serde_json::{json, Value};

const SCHEMA_EXTRACTION: &str = include_str!("../../../schemas/extraction.schema.json");
const SCHEMA_MANIFEST: &str = include_str!("../../../schemas/job-manifest.schema.json");

const EXTRACTION_VALID: &str = include_str!("../../../worker/tests/fixtures/extraction.valid.json");
const EXTRACTION_INVALID: &str =
    include_str!("../../../worker/tests/fixtures/extraction.invalid.json");
const EXTRACTION_STATUS: &str =
    include_str!("../../../worker/tests/fixtures/extraction.status.valid.json");
const MEETING_PROPOSALS: &str =
    include_str!("../../../worker/tests/fixtures/meeting.proposals.valid.json");
const CONTEXT_ISSUE: &str =
    include_str!("../../../worker/tests/fixtures/cycle-context.issue.valid.json");
const CONTEXT_CORRECTIONS: &str =
    include_str!("../../../worker/tests/fixtures/cycle-context.corrections.valid.json");
const MEETING_VALID: &str = include_str!("../../../worker/tests/fixtures/meeting.valid.json");
const MEETING_INVALID: &str = include_str!("../../../worker/tests/fixtures/meeting.invalid.json");
const CONTEXT_VALID: &str = include_str!("../../../worker/tests/fixtures/cycle-context.valid.json");
const CONTEXT_INVALID: &str =
    include_str!("../../../worker/tests/fixtures/cycle-context.invalid.json");
const MANIFEST: &str = include_str!("../../../worker/tests/fixtures/manifest.json");

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

fn validator(source: &str) -> jsonschema::Validator {
    jsonschema::validator_for(&json_of(source)).unwrap()
}

// ----- extraction (model output, validated by the worker; Rust has no type for it) ------

#[test]
fn the_extraction_fixtures_match_the_schema() {
    let v = validator(SCHEMA_EXTRACTION);
    assert!(
        v.is_valid(&json_of(EXTRACTION_VALID)),
        "valid fixture rejected"
    );
    assert!(
        !v.is_valid(&json_of(EXTRACTION_INVALID)),
        "invalid fixture accepted"
    );
}

#[test]
fn the_extraction_schema_pins_the_fields_the_application_relies_on() {
    let v = validator(SCHEMA_EXTRACTION);
    let mut doc = json_of(EXTRACTION_VALID);
    // A free-text due date and an unknown item type are what the schema must refuse.
    doc["items"][0]["due"] = json!("next Friday");
    assert!(!v.is_valid(&doc));
    let mut doc = json_of(EXTRACTION_VALID);
    doc["items"][0]["type"] = json!("rumor");
    assert!(!v.is_valid(&doc));
    let mut doc = json_of(EXTRACTION_VALID);
    doc["items"][0].as_object_mut().unwrap().remove("quote");
    assert!(!v.is_valid(&doc));
}

// ----- the meeting draft (worker output, edited by the application) ----------------------

#[test]
fn the_meeting_fixtures_match_the_schema() {
    assert!(
        schema::validate(Schema::Meeting, &json_of(MEETING_VALID)).is_empty(),
        "valid fixture rejected"
    );
    let issues = schema::validate(Schema::Meeting, &json_of(MEETING_INVALID));
    assert!(!issues.is_empty(), "invalid fixture accepted");
    assert!(
        Draft::from_json_str(MEETING_INVALID).is_err(),
        "the typed loader accepted the invalid fixture"
    );
}

/// `null` for a missing optional time of a warning is the same document as leaving the key
/// out; the Rust type leaves it out.
fn without_null_warning_times(mut doc: Value) -> Value {
    for w in doc["warnings"].as_array_mut().unwrap() {
        let w = w.as_object_mut().unwrap();
        for key in ["t_start_ms", "t_end_ms"] {
            if w.get(key) == Some(&Value::Null) {
                w.remove(key);
            }
        }
    }
    doc
}

#[test]
fn the_meeting_fixture_loads_into_the_draft_type_without_loss() {
    let original = json_of(MEETING_VALID);
    let draft = Draft::from_json_str(MEETING_VALID).unwrap();

    // Every field of the document is read: writing the draft out gives the same document.
    assert_eq!(
        draft.to_value(),
        without_null_warning_times(original.clone())
    );

    // And a second round trip is exact, including the typed values.
    let again = Draft::from_value(&draft.to_value()).unwrap();
    assert_eq!(again, draft);
    assert_eq!(again.to_value(), draft.to_value());

    // Spot checks on the values that matter to the editor.
    assert_eq!(draft.items.len(), 2);
    assert_eq!(draft.items[0].source.as_ref().unwrap().match_score, 1.0);
    assert!(draft.items[1].source.is_none() && draft.items[1].edited);
    assert_eq!(draft.warnings.len(), 2);
}

#[test]
fn the_application_writes_drafts_the_worker_side_accepts() {
    // A draft that went through the editor still validates against the shared schema.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("work")).unwrap();
    std::fs::write(dir.path().join("work/draft.json"), MEETING_VALID).unwrap();
    let files = DraftFiles::new(dir.path());
    files
        .edit(|d| {
            queue::draft::update_item(
                d,
                "i001",
                &queue::draft::ItemPatch {
                    text: Some("Prepare the mount and the cables".into()),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    let written: Value =
        json_of(&std::fs::read_to_string(dir.path().join("work/draft.json")).unwrap());
    assert!(schema::validate(Schema::Meeting, &written).is_empty());
    assert_eq!(written["items"][0]["edited"], true);
}

// ----- the cycle context (application output, worker input) ------------------------------

#[test]
fn the_context_fixtures_match_the_schema() {
    assert!(
        schema::validate(Schema::Context, &json_of(CONTEXT_VALID)).is_empty(),
        "valid fixture rejected"
    );
    assert!(
        !schema::validate(Schema::Context, &json_of(CONTEXT_INVALID)).is_empty(),
        "invalid fixture accepted"
    );
}

// ----- Stage 5 additions: optional fields, compatible with the documents above -------------

#[test]
fn the_extraction_schema_accepts_status_changes_and_pins_their_fields() {
    let v = validator(SCHEMA_EXTRACTION);
    assert!(v.is_valid(&json_of(EXTRACTION_STATUS)), "fixture rejected");
    // The model answer must carry the field, an empty list when nothing was reported.
    assert!(v.is_valid(&json_of(EXTRACTION_VALID)));
    assert_eq!(json_of(EXTRACTION_VALID)["status_changes"], json!([]));
    let mut doc = json_of(EXTRACTION_VALID);
    doc.as_object_mut().unwrap().remove("status_changes");
    assert!(!v.is_valid(&doc), "an answer without status_changes");
    for (key, bad) in [
        ("status", json!("open")),
        ("time", json!("13:32")),
        ("quote", json!("")),
        ("reason", json!("")),
    ] {
        let mut doc = json_of(EXTRACTION_STATUS);
        doc["status_changes"][0][key] = bad;
        assert!(!v.is_valid(&doc), "{key}");
    }
    let mut doc = json_of(EXTRACTION_STATUS);
    doc["status_changes"][0]
        .as_object_mut()
        .unwrap()
        .remove("target");
    assert!(!v.is_valid(&doc));
}

#[test]
fn drafts_with_status_proposals_load_into_the_draft_type_without_loss() {
    let original = json_of(MEETING_PROPOSALS);
    assert!(schema::validate(Schema::Meeting, &original).is_empty());
    let draft = Draft::from_json_str(MEETING_PROPOSALS).unwrap();
    assert_eq!(draft.status_proposals.len(), 2);
    assert_eq!(
        draft.status_proposals[0].decision,
        cycle_store::Decision::Approved
    );
    assert!(draft.status_proposals[1].source.is_none());
    assert_eq!(draft.to_value(), without_null_warning_times(original));
    assert_eq!(Draft::from_value(&draft.to_value()).unwrap(), draft);
}

#[test]
fn a_draft_without_proposals_is_written_with_exactly_the_old_bytes() {
    // Loading and saving a Stage 4 draft must not add a key: `draft.base.sha256` and the
    // edited-draft protection compare bytes.
    let draft = Draft::from_json_str(MEETING_VALID).unwrap();
    assert!(draft.to_value().get("status_proposals").is_none());
    assert_eq!(
        serde_json::to_string_pretty(&draft.to_value()).unwrap(),
        serde_json::to_string_pretty(&without_null_warning_times(json_of(MEETING_VALID))).unwrap()
    );
}

#[test]
fn edits_keep_the_proposals_of_a_draft() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("work")).unwrap();
    std::fs::write(dir.path().join("work/draft.json"), MEETING_PROPOSALS).unwrap();
    let files = DraftFiles::new(dir.path());
    files
        .edit(|d| {
            queue::draft::update_item(
                d,
                "i001",
                &queue::draft::ItemPatch {
                    text: Some("Prepare the mount and the cables".into()),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    let written: Value =
        json_of(&std::fs::read_to_string(dir.path().join("work/draft.json")).unwrap());
    assert!(schema::validate(Schema::Meeting, &written).is_empty());
    assert_eq!(written["status_proposals"].as_array().unwrap().len(), 2);
    assert_eq!(written["status_proposals"][0]["decision"], "approved");
}

#[test]
fn the_context_schema_accepts_the_issue_of_a_previous_item() {
    assert!(schema::validate(Schema::Context, &json_of(CONTEXT_VALID)).is_empty());
    let doc = json_of(CONTEXT_ISSUE);
    assert!(schema::validate(Schema::Context, &doc).is_empty());
    let mut bad = doc.clone();
    bad["previous_items"][0]["issue"] = json!(14);
    assert!(!schema::validate(Schema::Context, &bad).is_empty());
    let mut bad = doc;
    bad["previous_items"][0]["unknown"] = json!("x");
    assert!(!schema::validate(Schema::Context, &bad).is_empty());
}

#[test]
fn the_context_schema_accepts_corrections_and_the_stage_4_context_has_none() {
    assert!(json_of(CONTEXT_VALID).get("corrections").is_none());
    let doc = json_of(CONTEXT_CORRECTIONS);
    assert!(schema::validate(Schema::Context, &doc).is_empty());
    let mut bad = doc.clone();
    bad["corrections"][0]["issue"] = json!(12);
    assert!(!schema::validate(Schema::Context, &bad).is_empty());
    let mut bad = doc.clone();
    bad["corrections"][0]
        .as_object_mut()
        .unwrap()
        .remove("text");
    assert!(!schema::validate(Schema::Context, &bad).is_empty());
    let mut bad = doc;
    bad["corrections"][0]["unknown"] = json!("x");
    assert!(!schema::validate(Schema::Context, &bad).is_empty());
}

// ----- the corrections file ---------------------------------------------------------------

#[test]
fn the_corrections_file_validates_against_its_schema() {
    let good = json!({
        "version": 1,
        "meeting_id": "20261113-140501",
        "corrections": [
            {"id": "c1", "date": "2026-11-16", "text": "Moved.", "item": "i1", "issue": "#12"},
            {"id": "c2", "date": "2026-11-17", "text": "Dropped.", "item": null, "issue": null}
        ]
    });
    assert!(schema::validate(Schema::Corrections, &good).is_empty());
    for mutate in [
        |d: &mut serde_json::Value| d["corrections"][0]["id"] = json!("x1"),
        |d: &mut serde_json::Value| d["corrections"][0]["date"] = json!("16.11.2026"),
        |d: &mut serde_json::Value| d["corrections"][0]["text"] = json!(""),
        |d: &mut serde_json::Value| d["corrections"][0]["item"] = json!("3"),
        |d: &mut serde_json::Value| d["corrections"][0]["issue"] = json!("12"),
        |d: &mut serde_json::Value| d["corrections"][0]["extra"] = json!(true),
        |d: &mut serde_json::Value| d["version"] = json!(2),
        |d: &mut serde_json::Value| {
            d.as_object_mut().unwrap().remove("meeting_id");
        },
    ] {
        let mut bad = good.clone();
        mutate(&mut bad);
        assert!(
            !schema::validate(Schema::Corrections, &bad).is_empty(),
            "{bad}"
        );
    }
}

// ----- the job manifest ------------------------------------------------------------------

#[test]
fn the_manifest_fixture_and_the_rust_options_match_the_schema() {
    let v = validator(SCHEMA_MANIFEST);
    assert!(v.is_valid(&json_of(MANIFEST)), "fixture rejected");

    // What Rust writes with a backend chosen is accepted too.
    let options = queue::worker::JobOptions {
        llm: Some(queue::worker::LlmOptions {
            kind: store::LlmKind::External,
            model_path: None,
            ctx: Some(16_384),
            base_url: Some("http://127.0.0.1:11434/v1".into()),
            model: Some("gemma".into()),
            cloud_confirmed: false,
        }),
        ..Default::default()
    };
    let mut doc = json_of(MANIFEST);
    doc["options"] = serde_json::to_value(&options).unwrap();
    assert!(v.is_valid(&doc), "{doc}");

    // The schema refuses what the application must never write: a key in the manifest.
    doc["options"]["llm"]["api_key"] = json!("sk-should-not-be-here");
    assert!(!v.is_valid(&doc));
}
