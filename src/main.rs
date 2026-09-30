mod account;
mod daemon;
mod mail;
mod platform;
mod ui;
mod wiring;

#[cfg(test)]
mod layering_tests;

use anyhow::Result;
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
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
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

/// Pure CLI dispatch: parse the command line and hand off to the module that
/// owns each command. Everything else (logging, config, runtime, DB) is set up
/// by the command entry points themselves (see `platform::boot`).
fn main() -> Result<()> {
    // clap handles `--help`/`--version` and unknown commands (printing usage and
    // exiting) before we get here.
    let cli = Cli::parse();

    match cli.command {
        // No subcommand: launch the interactive TUI (the default).
        None => ui::run::start(),
        // Pure: no config/DB/logging needed. (clap also serves `--version`.)
        Some(Command::Version) => {
            println!("pesan {} ({})", env!("CARGO_PKG_VERSION"), env!("PESAN_GIT_SHA"));
            Ok(())
        }
        // Headless auto-fetch loop + local mail server, until SIGTERM / Ctrl-C.
        Some(Command::Daemon) => daemon::start(),
    }
}
