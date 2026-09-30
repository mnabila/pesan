use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::platform::sound::SoundHint;
use crate::mail::Envelope;
use crate::mail::application::ports;

/// Attach the configured sound to a notification via the matching freedesktop
/// hint. A no-op when no sound is requested (`sound: false`).
fn apply_sound(n: &mut notify_rust::Notification, sound: Option<&SoundHint>) {
    if let Some(sound) = sound {
        let hint = match sound {
            SoundHint::Name(name) => notify_rust::Hint::SoundName(name.clone()),
            SoundHint::File(path) => notify_rust::Hint::SoundFile(path.clone()),
        };
        n.hint(hint);
    }
}

/// Desktop notification via freedesktop/D-Bus (notify-send on Linux).
pub fn new_mail(
    account: &str,
    env: &Envelope,
    show_sender: bool,
    sound: Option<&SoundHint>,
) -> Result<()> {
    let mut body = Vec::new();
    if show_sender {
        body.push(format!(
            "{} <{}>",
            env.from.name.as_deref().unwrap_or(""),
            env.from.email
        ));
    }
    body.push(env.subject.clone());

    let mut n = notify_rust::Notification::new();
    n.summary(format!("New mail - {account}").as_str())
        .body(body.join("\n").as_str())
        .appname("pesan");
    apply_sound(&mut n, sound);
    n.show()
        .context("desktop notification failed (is a notification daemon running?)")?;
    Ok(())
}

/// One coalesced notification for a batch of new messages, avoiding a burst of
/// popups. A single message shows sender + subject; several show a count plus
/// the newest sender/subject.
pub fn new_mail_batch(
    account: &str,
    envelopes: &[Envelope],
    show_sender: bool,
    sound: Option<&SoundHint>,
) -> Result<()> {
    match envelopes {
        [] => return Ok(()),
        [only] => return new_mail(account, only, show_sender, sound),
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

    let mut n = notify_rust::Notification::new();
    n.summary(summary.as_str())
        .body(body.join("\n").as_str())
        .appname("pesan");
    apply_sound(&mut n, sound);
    n.show()
        .context("desktop notification failed (is a notification daemon running?)")?;
    Ok(())
}

//
// Port adapter
//

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
        if last.elapsed() < Duration::from_secs(30) {
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

impl ports::Notifier for DesktopNotifier {
    fn new_mail_batch(
        &self,
        account: &str,
        envelopes: &[Envelope],
        show_sender: bool,
        sound: Option<&SoundHint>,
    ) -> Result<()> {
        if !self.reachable() {
            // No notification daemon running: don't trigger a (futile) desktop popup.
            return Ok(());
        }
        match new_mail_batch(account, envelopes, show_sender, sound) {
            Ok(()) => {
                self.available.store(true, Ordering::Relaxed);
                Ok(())
            }
            Err(e) => {
                // A send failure is usually transient (a brief D-Bus hiccup), not
                // the daemon going away for good. Only suppress future attempts if
                // a fresh probe confirms no server is reachable; otherwise keep
                // notifying so one blip doesn't blackout mail for the cooldown.
                tracing::warn!("desktop notification failed: {e:#}");
                if notify_rust::get_server_information().is_err() {
                    self.available.store(false, Ordering::Relaxed);
                    if let Ok(mut last) = self.last_checked.lock() {
                        *last = Instant::now();
                    }
                }
                Ok(())
            }
        }
    }
}
