//! Wire protocol version 2 between the Chrome extension and the application.
//!
//! One WebSocket carries every captured tab of one browser profile.
//!
//! - Text frames are JSON control messages with a `type` field ([`ClientMessage`] from the
//!   extension, [`ServerMessage`] from the application). Unknown fields are ignored, so a newer
//!   peer can add optional fields without breaking an older one.
//! - Binary frames carry audio: a 16-byte little-endian header `u32 stream_no | u32 seq |
//!   f64 sent_ms` followed by signed 16-bit little-endian mono samples at 16 kHz.
//!   `stream_no` is the number the application gave the stream in `start_ok`, `seq` counts the
//!   frames of the stream from 0, and `sent_ms` is the sender's wall clock (milliseconds since
//!   the Unix epoch) when the frame was captured. Frames buffered by the sender keep their
//!   original `sent_ms`.
//!
//! The JSON Schema `schemas/bridge-message.schema.json` describes the same messages; the
//! fixtures in `crates/bridge/tests/fixtures` are shared with the extension tests.

use serde::{Deserialize, Serialize};

/// Protocol version spoken by this application. Peers with another version are refused.
pub const PROTOCOL_VERSION: u32 = 2;
/// Sample rate of every stream.
pub const SAMPLE_RATE: u32 = 16_000;
/// Channels of every stream.
pub const CHANNELS: u32 = 1;
/// Sample format of every stream.
pub const FORMAT: &str = "s16le";
/// Samples per frame the extension sends (20 ms).
pub const FRAME_SAMPLES: u32 = 320;
/// Bytes of the binary frame header.
pub const HEADER_BYTES: usize = 16;
/// Largest text message accepted.
pub const MAX_TEXT_BYTES: usize = 16 * 1024;
/// Largest audio payload of one binary frame (300 ms).
pub const MAX_PAYLOAD_BYTES: usize = 9_600;
/// Largest binary message accepted.
pub const MAX_BINARY_BYTES: usize = HEADER_BYTES + MAX_PAYLOAD_BYTES;
/// How often the extension sends `ping`, announced in `hello_ok`.
pub const HEARTBEAT_MS: u32 = 5_000;
/// A connection that sends nothing for this long is closed.
pub const HEARTBEAT_TIMEOUT_MS: u32 = 15_000;
/// Longest id (stream, client) accepted.
pub const MAX_ID_LEN: usize = 64;
/// Longest tab title kept; longer titles are cut.
pub const MAX_TITLE_CHARS: usize = 200;
/// Longest host accepted.
pub const MAX_HOST_LEN: usize = 255;

/// A message that breaks the protocol. The connection is closed after it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    #[error("message too large ({0} bytes)")]
    TooLarge(usize),
    #[error("malformed message: {0}")]
    Malformed(String),
    #[error("invalid field `{field}`: {reason}")]
    Invalid { field: &'static str, reason: String },
}

fn invalid(field: &'static str, reason: impl Into<String>) -> ProtocolError {
    ProtocolError::Invalid {
        field,
        reason: reason.into(),
    }
}

/// Extension to application.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello(Hello),
    Start(Start),
    Resume(Resume),
    Stop(Stop),
    Meet(Meet),
    Group(Group),
    Ping(Ping),
    Pong(Ping),
}

/// First message of every connection: pairs with a code, or authenticates with the
/// client id and secret received at pairing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub version: u32,
    /// Short description of the client, for example `Chrome 136`.
    pub client: String,
    pub ext_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairing_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

/// A tab starts streaming. Audio frames follow only after `start_ok`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Start {
    /// Id chosen by the extension, unique per capture of a tab.
    pub stream_id: String,
    pub tab_id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<i64>,
    pub title: String,
    pub host: String,
    pub sample_rate: u32,
    pub channels: u32,
    pub format: String,
    pub frame_samples: u32,
}

/// After a reconnect: continue a stream that was accepted on an earlier connection.
/// `next_seq` is the sequence number of the next frame the extension sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resume {
    pub stream_id: String,
    pub next_seq: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    UserStop,
    TrackEnded,
    TabClosed,
    ConnectionLost,
}

impl StopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserStop => "user_stop",
            Self::TrackEnded => "track_ended",
            Self::TabClosed => "tab_closed",
            Self::ConnectionLost => "connection_lost",
        }
    }
}

/// A stream ended on the extension side, with the sender's counters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    pub stream_id: String,
    pub reason: StopReason,
    pub frames_sent: u64,
    pub samples_sent: u64,
    pub frames_dropped: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetState {
    Joined,
    Left,
}

/// A Google Meet tab became audible (`joined`) or closed (`left`). Carries the host only,
/// never the meeting code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meet {
    pub tab_id: u32,
    pub host: String,
    pub state: MeetState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupTabStatus {
    Captured,
    Waiting,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupTab {
    pub tab_id: u32,
    pub status: GroupTabStatus,
}

/// Capture status of the tabs of a tab group the user is capturing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub group_id: i64,
    pub tabs: Vec<GroupTab>,
}

/// Heartbeat; the other side answers with `pong` carrying the same `t`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Ping {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t: Option<f64>,
}

/// Application to extension.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    HelloOk(HelloOk),
    Error(ErrorMessage),
    StartOk(StartOk),
    StartRefused(StartRefused),
    State(State),
    Ping(Ping),
    Pong(Ping),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloOk {
    pub client_id: String,
    /// Sent once, right after pairing; the extension keeps it to authenticate later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    /// Id of this connection, for logs.
    pub session_id: String,
    pub app_version: String,
    pub recording: bool,
    pub heartbeat_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The protocol version is not supported.
    BadVersion,
    /// The pairing code is wrong, or no pairing was started in the application.
    BadCode,
    /// The pairing code expired or was locked after too many wrong attempts.
    CodeExpired,
    /// The client id or secret is unknown (never paired, or unpaired in the application).
    Unpaired,
    /// Too many connections are open.
    Busy,
    /// A message broke the protocol.
    BadMessage,
}

/// Sent before the application closes the connection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorMessage {
    pub code: ErrorCode,
    pub message: String,
}

/// The stream is recorded; binary frames use `stream_no`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StartOk {
    pub stream_id: String,
    pub stream_no: u32,
    pub track: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefuseReason {
    /// No recording is running.
    NotRecording,
    /// The stream id is already in use.
    Duplicate,
    /// The application does not add tabs on its own (setting), or adding the source failed.
    Declined,
    /// `resume` named a stream the application does not know (any more).
    UnknownStream,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StartRefused {
    pub stream_id: String,
    pub reason: RefuseReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamPhase {
    /// Frames are recorded.
    Recording,
    /// The connection of the stream was lost; it waits for `resume`.
    Waiting,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamState {
    pub stream_id: String,
    pub track: String,
    pub status: StreamPhase,
}

/// The complete recording state as the extension should show it. Sent after `hello_ok` and
/// whenever it changes. A stream missing from `streams` is not recorded (any more).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub recording: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meeting_id: Option<String>,
    pub streams: Vec<StreamState>,
}

/// True for ids made of letters, digits and `_ - . :`, 1 to [`MAX_ID_LEN`] long.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'))
}

fn check_id(field: &'static str, id: &str) -> Result<(), ProtocolError> {
    if valid_id(id) {
        Ok(())
    } else {
        Err(invalid(
            field,
            "use 1 to 64 letters, digits, `_`, `-`, `.` or `:`",
        ))
    }
}

fn check_len(field: &'static str, text: &str, max: usize) -> Result<(), ProtocolError> {
    if text.len() > max {
        Err(invalid(field, format!("longer than {max} bytes")))
    } else {
        Ok(())
    }
}

impl ClientMessage {
    /// Parse and validate a text frame.
    pub fn parse(text: &str) -> Result<Self, ProtocolError> {
        if text.len() > MAX_TEXT_BYTES {
            return Err(ProtocolError::TooLarge(text.len()));
        }
        let msg: Self =
            serde_json::from_str(text).map_err(|e| ProtocolError::Malformed(e.to_string()))?;
        msg.validate()?;
        Ok(msg)
    }

    /// Field rules the JSON shape alone does not express.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        match self {
            Self::Hello(h) => {
                check_len("client", &h.client, 200)?;
                check_len("ext_version", &h.ext_version, 64)?;
                if let Some(code) = &h.pairing_code {
                    check_len("pairing_code", code, 32)?;
                }
                if let Some(id) = &h.client_id {
                    check_id("client_id", id)?;
                }
                if let Some(secret) = &h.secret {
                    check_len("secret", secret, 128)?;
                }
                let by_code = h.pairing_code.is_some();
                let by_secret = h.client_id.is_some() && h.secret.is_some();
                if by_code == by_secret {
                    return Err(invalid(
                        "hello",
                        "send either `pairing_code` or `client_id` with `secret`",
                    ));
                }
                Ok(())
            }
            Self::Start(s) => {
                check_id("stream_id", &s.stream_id)?;
                check_len("host", &s.host, MAX_HOST_LEN)?;
                check_len("title", &s.title, MAX_TEXT_BYTES)?;
                if s.sample_rate != SAMPLE_RATE {
                    return Err(invalid("sample_rate", format!("must be {SAMPLE_RATE}")));
                }
                if s.channels != CHANNELS {
                    return Err(invalid("channels", format!("must be {CHANNELS}")));
                }
                if s.format != FORMAT {
                    return Err(invalid("format", format!("must be `{FORMAT}`")));
                }
                let max_samples = (MAX_PAYLOAD_BYTES / 2) as u32;
                if s.frame_samples == 0 || s.frame_samples > max_samples {
                    return Err(invalid(
                        "frame_samples",
                        format!("must be 1 to {max_samples}"),
                    ));
                }
                Ok(())
            }
            Self::Resume(r) => check_id("stream_id", &r.stream_id),
            Self::Stop(s) => check_id("stream_id", &s.stream_id),
            Self::Meet(m) => check_len("host", &m.host, MAX_HOST_LEN),
            Self::Group(g) => {
                if g.tabs.len() > 256 {
                    return Err(invalid("tabs", "more than 256 tabs"));
                }
                Ok(())
            }
            Self::Ping(p) | Self::Pong(p) => match p.t {
                Some(t) if !t.is_finite() => Err(invalid("t", "not a number")),
                _ => Ok(()),
            },
        }
    }
}

impl ServerMessage {
    /// The JSON text of the message.
    pub fn to_text(&self) -> String {
        // Serializing these plain structs cannot fail.
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Parse a message from the application (used by test clients).
    pub fn parse(text: &str) -> Result<Self, ProtocolError> {
        serde_json::from_str(text).map_err(|e| ProtocolError::Malformed(e.to_string()))
    }
}

impl ClientMessage {
    /// The JSON text of the message (used by test clients).
    pub fn to_text(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Header of a binary audio frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameHeader {
    pub stream_no: u32,
    pub seq: u32,
    /// Sender wall clock at capture, ms since the Unix epoch.
    pub sent_ms: f64,
}

/// One decoded audio frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub header: FrameHeader,
    pub samples: Vec<i16>,
}

/// Decode a binary frame.
pub fn parse_frame(bytes: &[u8]) -> Result<Frame, ProtocolError> {
    if bytes.len() > MAX_BINARY_BYTES {
        return Err(ProtocolError::TooLarge(bytes.len()));
    }
    if bytes.len() <= HEADER_BYTES {
        return Err(ProtocolError::Malformed("frame without samples".into()));
    }
    let payload = &bytes[HEADER_BYTES..];
    if payload.len() % 2 != 0 {
        return Err(ProtocolError::Malformed(
            "odd number of payload bytes".into(),
        ));
    }
    let u32_at =
        |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    let mut f = [0u8; 8];
    f.copy_from_slice(&bytes[8..16]);
    let sent_ms = f64::from_le_bytes(f);
    if !sent_ms.is_finite() {
        return Err(invalid("sent_ms", "not a number"));
    }
    let samples = payload
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    Ok(Frame {
        header: FrameHeader {
            stream_no: u32_at(0),
            seq: u32_at(4),
            sent_ms,
        },
        samples,
    })
}

/// Encode a binary frame (used by test clients and the fake sender).
pub fn encode_frame(header: &FrameHeader, samples: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_BYTES + samples.len() * 2);
    out.extend_from_slice(&header.stream_no.to_le_bytes());
    out.extend_from_slice(&header.seq.to_le_bytes());
    out.extend_from_slice(&header.sent_ms.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip() {
        let h = FrameHeader {
            stream_no: 7,
            seq: 42,
            sent_ms: 1_794_579_901_123.5,
        };
        let samples = [0i16, 1, -1, i16::MAX, i16::MIN];
        let bytes = encode_frame(&h, &samples);
        assert_eq!(bytes.len(), HEADER_BYTES + 10);
        let f = parse_frame(&bytes).unwrap();
        assert_eq!(f.header, h);
        assert_eq!(f.samples, samples);
    }

    #[test]
    fn header_is_little_endian() {
        let bytes = encode_frame(
            &FrameHeader {
                stream_no: 1,
                seq: 0x0102_0304,
                sent_ms: 1.0,
            },
            &[0x0102],
        );
        assert_eq!(&bytes[0..4], &[1, 0, 0, 0]);
        assert_eq!(&bytes[4..8], &[4, 3, 2, 1]);
        assert_eq!(&bytes[8..16], &1.0f64.to_le_bytes());
        assert_eq!(&bytes[16..], &[2, 1]);
    }

    #[test]
    fn bad_frames_are_rejected() {
        let ok = encode_frame(
            &FrameHeader {
                stream_no: 1,
                seq: 0,
                sent_ms: 0.0,
            },
            &[1, 2],
        );
        assert!(matches!(
            parse_frame(&ok[..HEADER_BYTES]),
            Err(ProtocolError::Malformed(_))
        ));
        assert!(matches!(
            parse_frame(&ok[..HEADER_BYTES + 3]),
            Err(ProtocolError::Malformed(_))
        ));
        let big = vec![0u8; MAX_BINARY_BYTES + 2];
        assert!(matches!(parse_frame(&big), Err(ProtocolError::TooLarge(_))));
        let mut nan = ok.clone();
        nan[8..16].copy_from_slice(&f64::NAN.to_le_bytes());
        assert!(matches!(
            parse_frame(&nan),
            Err(ProtocolError::Invalid { .. })
        ));
        // The largest frame is accepted.
        let max = vec![0u8; MAX_BINARY_BYTES];
        assert_eq!(parse_frame(&max).unwrap().samples.len(), 4_800);
    }

    #[test]
    fn hello_needs_exactly_one_way_to_authenticate() {
        let both = r#"{"type":"hello","version":2,"client":"c","ext_version":"1","pairing_code":"ABCD1234","client_id":"c1","secret":"s"}"#;
        assert!(ClientMessage::parse(both).is_err());
        let none = r#"{"type":"hello","version":2,"client":"c","ext_version":"1"}"#;
        assert!(ClientMessage::parse(none).is_err());
        let id_only =
            r#"{"type":"hello","version":2,"client":"c","ext_version":"1","client_id":"c1"}"#;
        assert!(ClientMessage::parse(id_only).is_err());
    }

    #[test]
    fn start_must_use_the_fixed_format() {
        let base = |extra: &str| {
            format!(
                r#"{{"type":"start","stream_id":"s1","tab_id":3,"title":"t","host":"h","sample_rate":16000,"channels":1,"format":"s16le","frame_samples":320{extra}}}"#
            )
        };
        assert!(ClientMessage::parse(&base("")).is_ok());
        let wrong_rate = base("").replace("16000", "48000");
        assert!(ClientMessage::parse(&wrong_rate).is_err());
        let wrong_format = base("").replace("s16le", "f32le");
        assert!(ClientMessage::parse(&wrong_format).is_err());
        let bad_id = base("").replace("\"s1\"", "\"a b\"");
        assert!(ClientMessage::parse(&bad_id).is_err());
    }

    #[test]
    fn unknown_fields_are_ignored_and_unknown_types_rejected() {
        let extra = r#"{"type":"ping","t":5,"future_field":true}"#;
        assert_eq!(
            ClientMessage::parse(extra).unwrap(),
            ClientMessage::Ping(Ping { t: Some(5.0) })
        );
        assert!(ClientMessage::parse(r#"{"type":"teleport"}"#).is_err());
        assert!(ClientMessage::parse("not json").is_err());
    }

    #[test]
    fn oversized_text_is_rejected_before_parsing() {
        let text = format!(
            r#"{{"type":"ping","pad":"{}"}}"#,
            "x".repeat(MAX_TEXT_BYTES)
        );
        assert!(matches!(
            ClientMessage::parse(&text),
            Err(ProtocolError::TooLarge(_))
        ));
    }

    #[test]
    fn ids_are_restricted() {
        assert!(valid_id("tab-12_3.4:5"));
        assert!(!valid_id(""));
        assert!(!valid_id("a/b"));
        assert!(!valid_id(&"x".repeat(MAX_ID_LEN + 1)));
    }
}
