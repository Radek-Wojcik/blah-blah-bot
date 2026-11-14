//! Logging to a daily rolling file in the application log folder, plus stderr in debug builds.
//!
//! Logs must never contain secrets or meeting content: log identifiers, counts and error
//! kinds only.

use std::path::Path;
use std::sync::OnceLock;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

static GUARD: OnceLock<WorkerGuard> = OnceLock::new();

pub fn init(log_dir: &Path) {
    let appender = tracing_appender::rolling::daily(log_dir, "blah-blah-bot.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let _ = GUARD.set(guard);

    let filter = EnvFilter::try_from_env("BBB_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let file_layer = fmt::layer().with_ansi(false).with_writer(writer);
    let registry = tracing_subscriber::registry().with(filter).with(file_layer);

    #[cfg(debug_assertions)]
    let registry = registry.with(fmt::layer().with_writer(std::io::stderr));

    let _ = registry.try_init();
}
