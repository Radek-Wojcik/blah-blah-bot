//! Publishing targets of Blah Blah Bot: the HTTP layer shared by the GitHub and Discord
//! publishers, the publish plan with its per-target rules and the public-repository check,
//! the outbox dispatcher that carries the plan out, and (behind the `test-support` feature)
//! stateful fake servers to test them against. No test in this repository talks to a real
//! host.

pub mod addr;
pub mod correction;
pub mod discord;
pub mod discord_bot;
pub mod export;
pub mod flow;
pub mod gate;
pub mod github;
pub mod http;
pub mod notion;
pub mod outbox;
pub mod plan;
pub mod rules;
pub mod webhook;

#[cfg(any(test, feature = "test-support"))]
pub mod testing;
