//! Logging to a daily rolling file in the application log folder, plus stderr in debug builds.
//!
//! Logs must never contain secrets or meeting content: log identifiers, counts and error
//! kinds only.

use std::path::Path;
use std::sync::OnceLock;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

static GUARD: OnceLock<WorkerGuard> = OnceLock::new();

fn build_filter(env_val: Option<&str>, clamp_http: bool) -> EnvFilter {
    let mut filter = env_val
        .and_then(|v| EnvFilter::try_new(v).ok())
        .unwrap_or_else(|| EnvFilter::new("info"));
    if clamp_http {
        // No build traces HTTP client headers/bodies via BBB_LOG.
        for d in ["ureq=warn", "ureq_proto=warn"] {
            if let Ok(parsed) = d.parse() {
                filter = filter.add_directive(parsed);
            }
        }
    }
    filter
}

pub fn init(log_dir: &Path) {
    let appender = tracing_appender::rolling::daily(log_dir, "blah-blah-bot.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let _ = GUARD.set(guard);

    let env_val = std::env::var("BBB_LOG").ok();
    let filter = build_filter(env_val.as_deref(), true);
    let file_layer = fmt::layer().with_ansi(false).with_writer(writer);
    let registry = tracing_subscriber::registry().with(filter).with(file_layer);

    #[cfg(debug_assertions)]
    let registry = registry.with(fmt::layer().with_writer(std::io::stderr));

    let _ = registry.try_init();
    log_panics();
}

/// Records every panic in the log: where it happened and the kind of its payload, never its
/// text (it can hold a token, a webhook or a path). Without this a panic in a background
/// thread leaves no trace in the file log.
fn log_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let kind = match info.payload().downcast_ref::<&str>() {
            Some(_) => "str",
            None if info.payload().is::<String>() => "string",
            None => "other",
        };
        let place = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let thread = std::thread::current();
        tracing::error!(
            thread = thread.name().unwrap_or("unnamed"),
            location = %place,
            payload = kind,
            "a thread panicked (panic text withheld)"
        );
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_filter_clamps_ureq_tracing() {
        let clamped = build_filter(Some("info,ureq=trace,ureq_proto=debug"), true).to_string();
        assert!(clamped.contains("ureq=warn"), "{clamped}");
        assert!(clamped.contains("ureq_proto=warn"), "{clamped}");
        assert!(!clamped.contains("ureq=trace"), "{clamped}");
        assert!(!clamped.contains("ureq_proto=debug"), "{clamped}");
    }
}
