//! Fake services for tests: a generic fault-injecting HTTP server, a stateful GitHub and a
//! stateful Discord webhook. They listen on loopback ports only; nothing here can reach a
//! real host. Available with the `test-support` feature (and in this crate's own tests).

pub mod discord;
pub mod github;
pub mod server;

pub use discord::FakeDiscord;
pub use github::{FakeGithub, Visibility};
pub use server::{FakeServer, Fault, Recorded, Reply};
