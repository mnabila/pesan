use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::domain::Envelope;

/// Desktop notification via freedesktop/D-Bus (notify-send on Linux).
pub fn new_mail(account: &str, env: &Envelope, show_sender: bool) -> Result<()> {
    let mut body = Vec::new();
    if show_sender {
        body.push(format!(
            "{} <{}>",
            env.from.name.as_deref().unwrap_or(""),
            env.from.email
        ));
    }
    body.push(env.subject.clone());

    notify_rust::Notification::new()
        .summary(format!("New mail - {account}").as_str())
        .body(body.join("\n").as_str())
        .appname("pesan")
        .show()
        .context("desktop notification failed (is a notification daemon running?)")?;
    Ok(())
}

/// One coalesced notification for a batch of new messages, avoiding a burst of
/// popups. A single message shows sender + subject; several show a count plus
/// the newest sender/subject.
pub fn new_mail_batch(account: &str, envelopes: &[Envelope], show_sender: bool) -> Result<()> {
    match envelopes {
        [] => return Ok(()),
        [only] => return new_mail(account, only, show_sender),
        _ => {}
    }

    let newest = &envelopes[0];
    let summary = format!("{} new messages - {account}", envelopes.len());
    let mut body = Vec::new();
    if show_sender {
        let sender = newest
            .from
            .name
            .as_deref()
            .filter(|n| !n.is_empty())
            .unwrap_or(newest.from.email.as_str());
        body.push(format!("{sender}: {}", newest.subject));
    } else {
        body.push(newest.subject.clone());
    }
    body.push(format!("and {} more", envelopes.len() - 1));

    notify_rust::Notification::new()
        .summary(summary.as_str())
        .body(body.join("\n").as_str())
        .appname("pesan")
        .show()
        .context("desktop notification failed (is a notification daemon running?)")?;
    Ok(())
}

// Port adapter ----------------------------------------------------------

/// [`Notifier`] backed by the platform desktop-notification mechanism;
/// Desktop notification backend. It probes for a running notification daemon
/// (via D-Bus) and skips sending when none is available, so a TUI/SSH/headless
/// session doesn't spam warnings or attempt futile popups. If the daemon goes
/// away mid-session it stops trying until a cooldown re-probe succeeds.
pub struct DesktopNotifier {
    available: AtomicBool,
    last_checked: Mutex<Instant>,
}

impl Default for DesktopNotifier {
    fn default() -> Self {
        Self {
            available: AtomicBool::new(true),
            last_checked: Mutex::new(Instant::now()),
        }
    }
}

impl DesktopNotifier {
    pub fn new() -> Self {
        Self::default()
    }

    /// True when a notification server is reachable. When the last known state
    /// was "unavailable" it re-probes on a cooldown so a daemon that starts later
    /// is picked up.
    fn reachable(&self) -> bool {
        if self.available.load(Ordering::Relaxed) {
            return true;
        }
        let mut last = match self.last_checked.lock() {
            Ok(g) => g,
            Err(_) => return false,
        };
        if last.elapsed() < Duration::from_secs(300) {
            return false;
        }
        *last = Instant::now();
        match notify_rust::get_server_information() {
            Ok(_) => {
                self.available.store(true, Ordering::Relaxed);
                true
            }
            Err(_) => false,
        }
    }
}

#[allow(dead_code)] // port consumed from Phase 2 onward
impl crate::application::ports::Notifier for DesktopNotifier {
    fn new_mail_batch(
        &self,
        account: &str,
        envelopes: &[Envelope],
        show_sender: bool,
    ) -> Result<()> {
        if !self.reachable() {
            // No notification daemon running: don't trigger a (futile) desktop popup.
            return Ok(());
        }
        match new_mail_batch(account, envelopes, show_sender) {
            Ok(()) => {
                self.available.store(true, Ordering::Relaxed);
                Ok(())
            }
            Err(e) => {
                // The daemon went away; suppress further attempts until the next probe.
                tracing::debug!("desktop notification failed: {e:#}");
                self.available.store(false, Ordering::Relaxed);
                Ok(())
            }
        }
    }
}
