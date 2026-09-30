CREATE TABLE IF NOT EXISTS accounts (
  id            TEXT PRIMARY KEY,
  name          TEXT NOT NULL UNIQUE,
  email         TEXT NOT NULL,
  provider      TEXT NOT NULL,
  keychain_ref  TEXT NOT NULL,
  is_default    INTEGER NOT NULL DEFAULT 0,
  created_at    INTEGER NOT NULL
);
