use anyhow::Result;

use crate::domain::Envelope;

#[allow(dead_code)]
pub trait Notifier: Send + Sync {
    fn new_mail_batch(
        &self,
        account: &str,
        envelopes: &[Envelope],
        show_sender: bool,
    ) -> Result<()>;
}
