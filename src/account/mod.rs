//! Account-management slice: identity, credentials, onboarding, and the
//! persistence/keyring adapters behind the account ports.

pub mod application;
pub mod domain;
pub mod infrastructure;

pub use domain::Account;

/// Conventional keychain reference for a freshly created account.
/// Pure naming convention; the OS-keyring access lives in the infrastructure layer.
pub fn keychain_ref_for(account_name: &str) -> String {
    format!("pesan/{account_name}/refresh")
}

/// Conventional keychain reference for a password-login account. Distinct from
/// the OAuth `/refresh` ref so the two secret kinds never collide.
pub fn keychain_ref_for_password(account_name: &str) -> String {
    format!("pesan/{account_name}/password")
}
