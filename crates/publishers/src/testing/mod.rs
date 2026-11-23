//! Fake services for tests: a generic fault-injecting HTTP server, a stateful GitHub and a
//! stateful Discord webhook, and builders for drafts and cycles. They listen on loopback ports only; nothing here can reach a
//! real host. Available with the `test-support` feature (and in this crate's own tests).

pub mod discord;
pub mod discord_bot;
pub mod fixtures;
pub mod github;
pub mod lab;
pub mod notion;
pub mod server;

pub use discord::FakeDiscord;
pub use discord_bot::FakeDiscordBot;
pub use github::{FakeGithub, Visibility};
pub use notion::FakeNotion;
pub use server::{FakeServer, Fault, Recorded, Reply};
