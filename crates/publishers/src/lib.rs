//! Publishing targets of Blah Blah Bot: the HTTP layer shared by the GitHub and Discord
//! publishers, and (behind the `test-support` feature) stateful fake servers to test them
//! against. No test in this repository talks to a real host.

pub mod http;

#[cfg(any(test, feature = "test-support"))]
pub mod testing;
