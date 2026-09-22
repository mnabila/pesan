use sqlx::FromRow;

/// A per-user account record. Points at a provider (`config.yaml providers.*`)
/// and a keychain entry (`keychain_ref`) holding the OAuth refresh token.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct Account {
    pub id: Option<String>,
    pub name: String,
    pub email: String,
    pub provider: String,
    pub keychain_ref: String,
    pub is_default: bool,
    pub created_at: i64,
}

pub mod connect;
pub mod connect_params;
pub mod onboarding;

/// Conventional keychain reference for a freshly created account.
/// Pure naming convention; the OS-keyring access lives in `infrastructure`.
pub fn keychain_ref_for(account_name: &str) -> String {
    format!("pesan/{account_name}/refresh")
}

/// Conventional keychain reference for a password-login account. Distinct from
/// the OAuth `/refresh` ref so the two secret kinds never collide.
pub fn keychain_ref_for_password(account_name: &str) -> String {
    format!("pesan/{account_name}/password")
}
