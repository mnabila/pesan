use clap::{Parser, Subcommand};

/// A keyboard-driven terminal email client. Run without a subcommand to open
/// the interactive TUI.
#[derive(Parser, Debug)]
#[command(name = "pesan", version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Print the version and exit.
    Version,
}

/// `pesan version`. Pure - needs no config or DB. (clap also serves `--version`.)
pub fn print_version() {
    println!("pesan {}", env!("CARGO_PKG_VERSION"));
}

#[cfg(test)]
mod tests {}
