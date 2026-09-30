/// A per-user account. Points at a provider (`config.yaml providers.*`)
/// and a keychain entry (`keychain_ref`) holding the OAuth refresh token or the
/// login password.
///
/// A domain entity: no persistence derives, no framework types. How a row is
/// stored and read back is the database adapter's business (see
/// `infrastructure::database::accounts`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// `None` until first persisted; afterwards the stable UUID that also names
    /// the account's Maildir directory.
    pub id: Option<String>,
    pub name: String,
    pub email: String,
    /// Key into `config.yaml providers.*`.
    pub provider: String,
    /// Secret-store key for this account's refresh token or password.
    pub keychain_ref: String,
    pub is_default: bool,
    /// Unix timestamp (seconds).
    pub created_at: i64,
}
