//! Contract tests on the shared fixtures: the same files are read by the extension tests, so
//! both sides agree on the JSON messages and on the binary frame layout.

use bridge::protocol::{parse_frame, ClientMessage, ServerMessage};
use serde_json::Value;

const SCHEMA: &str = include_str!("../../../schemas/bridge-message.schema.json");
const MESSAGES: &str = include_str!("fixtures/messages.json");
const FRAMES: &str = include_str!("fixtures/frames.json");

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn entries<'a>(doc: &'a Value, key: &str) -> &'a Vec<Value> {
    doc[key].as_array().unwrap()
}

#[test]
fn every_valid_message_matches_the_schema() {
    let schema = jsonschema::validator_for(&json(SCHEMA)).unwrap();
    let doc = json(MESSAGES);
    for side in ["client", "server"] {
        for e in entries(&doc, side) {
            let errors: Vec<String> = schema
                .iter_errors(&e["message"])
                .map(|err| err.to_string())
                .collect();
            assert!(errors.is_empty(), "{}: {errors:?}", e["name"]);
        }
    }
}

#[test]
fn client_messages_round_trip_unchanged() {
    let doc = json(MESSAGES);
    for e in entries(&doc, "client") {
        let text = e["message"].to_string();
        let msg = ClientMessage::parse(&text).unwrap_or_else(|err| panic!("{}: {err}", e["name"]));
        assert_eq!(json(&msg.to_text()), e["message"], "{}", e["name"]);
    }
}

#[test]
fn server_messages_round_trip_unchanged() {
    let doc = json(MESSAGES);
    for e in entries(&doc, "server") {
        let text = e["message"].to_string();
        let msg = ServerMessage::parse(&text).unwrap_or_else(|err| panic!("{}: {err}", e["name"]));
        assert_eq!(json(&msg.to_text()), e["message"], "{}", e["name"]);
    }
}

#[test]
fn invalid_messages_are_refused_by_the_parser_and_the_schema() {
    let schema = jsonschema::validator_for(&json(SCHEMA)).unwrap();
    let doc = json(MESSAGES);
    for e in entries(&doc, "invalid") {
        let text = e["text"].as_str().unwrap();
        assert!(ClientMessage::parse(text).is_err(), "{} parsed", e["name"]);
        if let Ok(value) = serde_json::from_str::<Value>(text) {
            assert!(!schema.is_valid(&value), "{} matches the schema", e["name"]);
        }
    }
}

#[test]
fn frames_decode_as_listed() {
    let doc = json(FRAMES);
    for e in entries(&doc, "valid") {
        let f = parse_frame(&hex(e["hex"].as_str().unwrap())).unwrap();
        assert_eq!(
            u64::from(f.header.stream_no),
            e["stream_no"].as_u64().unwrap()
        );
        assert_eq!(u64::from(f.header.seq), e["seq"].as_u64().unwrap());
        assert_eq!(f.header.sent_ms, e["sent_ms"].as_f64().unwrap());
        let want: Vec<i16> = e["samples"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap() as i16)
            .collect();
        assert_eq!(f.samples, want, "{}", e["name"]);
        // And they encode back to the same bytes.
        assert_eq!(
            bridge::protocol::encode_frame(&f.header, &f.samples),
            hex(e["hex"].as_str().unwrap())
        );
    }
    for e in entries(&doc, "invalid") {
        assert!(
            parse_frame(&hex(e["hex"].as_str().unwrap())).is_err(),
            "{}",
            e["name"]
        );
    }
}
