//! Launching the user's browser. A platform primitive: the OAuth flow opens a
//! consent URL and the reader opens an exported message, both through here.

use std::process::Command;

use anyhow::{Context, Result};

/// Open a file/URL in the browser using the configured command or system default.
/// `browser_cmd` can be a full command with arguments (e.g., "firefox --new-window").
/// Runs the browser in the background with output silenced.
pub fn open_in_browser(path: &std::path::Path, browser_cmd: Option<&str>) -> Result<()> {
    if let Some(cmd) = browser_cmd.filter(|s| !s.is_empty()) {
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        if parts.is_empty() {
            return Err(anyhow::anyhow!("empty browser command"));
        }
        let mut command = Command::new(parts[0]);
        if parts.len() > 1 {
            command.args(&parts[1..]);
        }
        command
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(|| format!("spawn browser: {}", cmd))?;
    } else {
        open::that_detached(path)
            .with_context(|| format!("open with system default: {}", path.display()))?;
    }
    Ok(())
}
