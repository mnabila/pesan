-- The account identifier is now a UUID (TEXT), not the SQLite rowid, so the
-- on-disk Maildir tree is keyed by a stable id that survives a DB rebuild.
-- Existing cached data is disposable, so this drops and recreates every
-- account-keyed table with TEXT `id`/`account_id` columns. `secrets` is keyed by
-- `keychain_ref` (not account id), so it is left untouched.
DROP TRIGGER IF EXISTS messages_ai;
DROP TRIGGER IF EXISTS messages_ad;
DROP TRIGGER IF EXISTS messages_au;
DROP TABLE IF EXISTS messages_fts;
DROP TABLE IF EXISTS messages;
DROP TABLE IF EXISTS folders;
DROP TABLE IF EXISTS sync_state;
DROP TABLE IF EXISTS accounts;

CREATE TABLE accounts (
  id            TEXT PRIMARY KEY,
  name          TEXT NOT NULL UNIQUE,
  email         TEXT NOT NULL,
  provider      TEXT NOT NULL,
  keychain_ref  TEXT NOT NULL,
  is_default    INTEGER NOT NULL DEFAULT 0,
  created_at    INTEGER NOT NULL
);

CREATE TABLE folders (
  account_id TEXT NOT NULL,
  name       TEXT NOT NULL,
  total      INTEGER NOT NULL DEFAULT 0,
  unread     INTEGER NOT NULL DEFAULT 0,
  category   TEXT NOT NULL DEFAULT 'mailbox',
  position   INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (account_id, name)
);

CREATE TABLE messages (
  account_id     TEXT NOT NULL,
  folder         TEXT NOT NULL,
  uid            INTEGER NOT NULL,
  seen           INTEGER NOT NULL DEFAULT 0,
  flagged        INTEGER NOT NULL DEFAULT 0,
  from_name      TEXT,
  from_email     TEXT NOT NULL DEFAULT '',
  subject        TEXT NOT NULL DEFAULT '',
  date           INTEGER NOT NULL DEFAULT 0,
  has_attachment INTEGER NOT NULL DEFAULT 0,
  message_id     TEXT,
  snippet        TEXT,
  body           TEXT,
  PRIMARY KEY (account_id, folder, uid)
);
CREATE INDEX messages_by_date ON messages (account_id, folder, date DESC);

CREATE VIRTUAL TABLE messages_fts USING fts5(
  subject, from_name, from_email,
  content='messages', content_rowid='rowid'
);

CREATE TRIGGER messages_ai AFTER INSERT ON messages BEGIN
  INSERT INTO messages_fts(rowid, subject, from_name, from_email)
  VALUES (new.rowid, new.subject, new.from_name, new.from_email);
END;
CREATE TRIGGER messages_ad AFTER DELETE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, subject, from_name, from_email)
  VALUES ('delete', old.rowid, old.subject, old.from_name, old.from_email);
END;
CREATE TRIGGER messages_au AFTER UPDATE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, subject, from_name, from_email)
  VALUES ('delete', old.rowid, old.subject, old.from_name, old.from_email);
  INSERT INTO messages_fts(rowid, subject, from_name, from_email)
  VALUES (new.rowid, new.subject, new.from_name, new.from_email);
END;

CREATE TABLE sync_state (
  account_id   TEXT NOT NULL,
  folder       TEXT NOT NULL,
  uid_validity INTEGER,
  last_uid     INTEGER,
  updated_at   INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (account_id, folder)
);
