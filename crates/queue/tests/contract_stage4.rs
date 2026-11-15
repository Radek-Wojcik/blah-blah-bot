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
