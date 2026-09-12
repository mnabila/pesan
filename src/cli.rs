use clap::{Parser, Subcommand};

/// A keyboard-driven terminal email client. Run without a subcommand to open
/// the interactive TUI.
#[derive(Parser, Debug)]
#[command(
    name = "pesan",
    version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("PESAN_GIT_SHA"), ")"),
    about,
    long_about = None
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Print the version and exit.
    Version,
    /// Run the mail server in the foreground.
    ///
    /// Owns all IMAP/SMTP sessions, keeps the local cache warm, raises desktop
    /// notifications, and serves the TUI over a unix socket. The TUI is a strict
    /// client - it needs this daemon running to go online (without it, the TUI
    /// shows cached mail only). Logs to stdout; stop with SIGTERM / Ctrl-C. Runs
    /// cleanly under `systemd --user`.
    Daemon,
}

/// `pesan version`. Pure - needs no config or DB. (clap also serves `--version`.)
pub fn print_version() {
    println!("pesan {} ({})", env!("CARGO_PKG_VERSION"), env!("PESAN_GIT_SHA"));
}

#[cfg(test)]
mod tests {}
