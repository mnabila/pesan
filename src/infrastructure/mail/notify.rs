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
/// delegates to [`new_mail_batch`].
pub struct DesktopNotifier;

#[allow(dead_code)] // port consumed from Phase 2 onward
impl crate::application::ports::Notifier for DesktopNotifier {
    fn new_mail_batch(
        &self,
        account: &str,
        envelopes: &[Envelope],
        show_sender: bool,
    ) -> Result<()> {
        new_mail_batch(account, envelopes, show_sender)
    }
}
