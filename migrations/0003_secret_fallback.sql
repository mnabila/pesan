-- Fallback secret store: refresh tokens keyed by an account's `keychain_ref`,
-- used only when the OS keychain is unavailable. Plaintext on disk (same as the
-- rest of the DB), so it is a downgrade from the encrypted OS keychain.
CREATE TABLE IF NOT EXISTS secrets (
  ref   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
