//! Shared process bootstrap: the setup every non-trivial command needs before
//! it can do anything - logging, config, the Tokio runtime, and the SQLite pool.
//!
//! Each command entry (`ui::run::start`, `daemon::start`) calls [`boot`] with the
//! log destination it wants, then drives the returned runtime. Keeping this here
//! (not in `main`) lets `main` stay a pure CLI dispatch table.

use anyhow::Result;

use crate::platform::config::{self, Config};
use crate::platform::db::{self, Db};
use crate::platform::logging::{self, LogMode};
use crate::platform::runtime;

/// The initialized process environment. Hold it for the lifetime of the command:
/// dropping `_log_guard` flushes and stops the logging worker.
pub struct Boot {
    pub config: Config,
    pub runtime: tokio::runtime::Runtime,
    pub pool: Db,
    // Kept alive so the non-blocking log writer keeps flushing; never read.
    _log_guard: tracing_appender::non_blocking::WorkerGuard,
}

/// Initialize logging (to `mode`), load config, build the runtime, and open the
/// DB pool. `mode` differs per command: the daemon mirrors logs to stdout for
/// journald, the TUI logs to the file only (stdout is its interface).
pub fn boot(mode: LogMode) -> Result<Boot> {
    let log_guard = logging::init_logging(mode)?;
    let config = Config::load()?;
    let runtime = runtime::build_runtime()?;
    let pool = runtime.block_on(db::open(&config::db_path()?))?;
    Ok(Boot {
        config,
        runtime,
        pool,
        _log_guard: log_guard,
    })
}
