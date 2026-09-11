use anyhow::Result;

use crate::domain::Envelope;

/// Sound to play with a desktop notification, mapped to a freedesktop hint by
/// the backend. `File` is an absolute path to a sound file (`sound-file` hint);
/// `Name` is a sound-theme name like `message-new-instant` (`sound-name` hint).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SoundHint {
    Name(String),
    File(String),
}

#[allow(dead_code)]
pub trait Notifier: Send + Sync {
    fn new_mail_batch(
        &self,
        account: &str,
        envelopes: &[Envelope],
        show_sender: bool,
        sound: Option<&SoundHint>,
    ) -> Result<()>;
}
