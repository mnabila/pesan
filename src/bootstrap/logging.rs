use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;

/// Log filter env vars, checked in order. Kept here so tests and callers agree
/// on the precedence without re-reading `main`.
pub static FILTER_ENV_VARS: [&str; 2] = ["PESAN_LOG", "RUST_LOG"];

fn default_filter() -> EnvFilter {
    EnvFilter::new("pesan=info")
}

/// Resolve the effective filter from the environment.
fn resolve_filter() -> EnvFilter {
    FILTER_ENV_VARS
        .iter()
        .find_map(|var| std::env::var(var).ok().map(EnvFilter::new))
        .unwrap_or_else(default_filter)
}

/// Install the global subscriber writing daily-rotated files under `dir`.
/// Returns the non-blocking writer guard; hold it for the process lifetime.
pub fn init_logging() -> Result<tracing_appender::non_blocking::WorkerGuard> {
    let dir = crate::bootstrap::config::log_dir()?;
    std::fs::create_dir_all(&dir).context("create log dir")?;
    let appender = tracing_appender::rolling::daily(&dir, "pesan.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);

    tracing_subscriber::fmt()
        .with_env_filter(resolve_filter())
        .with_writer(non_blocking)
        .with_ansi(false)
        .init();
    Ok(guard)
}
