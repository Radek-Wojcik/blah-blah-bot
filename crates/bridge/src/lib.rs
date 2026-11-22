//! Local WebSocket bridge between the Chrome extension and the application.
//!
//! [protocol] defines the messages and the audio frame layout (version 2), [pairing] the
//! one-time codes and client secrets.

pub mod pairing;
pub mod protocol;
