use anyhow::{Context, Result};

/// A single multi-thread runtime drives everything. DB access (sqlx) is async,
/// so the pool is opened via `block_on`; the blocking OAuth flow deliberately
/// runs outside `block_on` (on dedicated threads) so it never nests a runtime.
pub fn build_runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")
}
