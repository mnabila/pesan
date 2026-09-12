use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::time::ChronoLocal;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

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

/// Where logs go. Both modes write the daily-rotated file under the log dir;
/// `FileAndStdout` additionally echoes to stdout so a supervisor (systemd)
/// captures the same lines via journald.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogMode {
    /// TUI: file only (stdout is the interface).
    File,
    /// Foreground daemon: file (for a durable on-disk copy) and stdout (journald).
    FileAndStdout,
}

/// Install the global subscriber. Always writes a daily-rotated file
/// (`pesan.log.YYYY-MM-DD`, rolling at local midnight while the process runs);
/// `FileAndStdout` also mirrors to stdout. Returns the non-blocking file writer
/// guard - hold it for the process lifetime or logs may be lost on exit.
pub fn init_logging(mode: LogMode) -> Result<tracing_appender::non_blocking::WorkerGuard> {
    let dir = crate::bootstrap::config::log_dir()?;
    std::fs::create_dir_all(&dir).context("create log dir")?;
    let appender = tracing_appender::rolling::daily(&dir, "pesan.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);

    let file_layer = tracing_subscriber::fmt::layer()
        .with_timer(ChronoLocal::rfc_3339())
        .with_ansi(false)
        .with_writer(non_blocking);
    let base = tracing_subscriber::registry()
        .with(resolve_filter())
        .with(file_layer);

    match mode {
        LogMode::FileAndStdout => {
            let stdout_layer = tracing_subscriber::fmt::layer()
                .with_timer(ChronoLocal::rfc_3339())
                .with_ansi(false)
                .with_writer(std::io::stdout);
            base.with(stdout_layer).init();
        }
        LogMode::File => base.init(),
    }
    Ok(guard)
}
