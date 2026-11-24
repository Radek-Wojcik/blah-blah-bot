//! Local WebSocket bridge between the Chrome extension and the application.
//!
//! - [protocol]: the messages and the audio frame layout (version 2).
//! - [pairing]: one-time codes and client secrets.
//! - [server]: the socket on `127.0.0.1`, [conn] one connection, [hub] the shared state
//!   (clients, connections, tab streams) the application talks to.
//!
//! The server accepts loopback peers only, checks the `Host` and `Origin` headers of the
//! upgrade, allows two connections, limits message sizes and closes a connection that stays
//! silent for 15 seconds. See ADR 0020.

mod conn;
pub mod hub;
pub mod pairing;
pub mod protocol;
pub mod server;

pub use hub::{
    EndReason, FeedEvent, Hub, HubConfig, HubObserver, HubStatus, StreamFeed, TabStream,
};
pub use server::{Server, ServerConfig};
