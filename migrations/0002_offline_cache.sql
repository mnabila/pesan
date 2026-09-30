-- M6 offline cache: folders, messages (+ FTS5 full-text index), and per-folder
-- sync state, all keyed by `account_id`. The FTS index is external-content over
-- `messages`, kept in sync by triggers, so cache writes stay simple upserts.
CREATE TABLE IF NOT EXISTS folders (
  account_id TEXT NOT NULL,
  name       TEXT NOT NULL,
  total      INTEGER NOT NULL DEFAULT 0,
  unread     INTEGER NOT NULL DEFAULT 0,
  category   TEXT NOT NULL DEFAULT 'mailbox',
  position   INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (account_id, name)
);

CREATE TABLE IF NOT EXISTS messages (
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
CREATE INDEX IF NOT EXISTS messages_by_date
  ON messages (account_id, folder, date DESC);

CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
  subject, from_name, from_email, body,
  content='messages', content_rowid='rowid'
);

CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
  INSERT INTO messages_fts(rowid, subject, from_name, from_email, body)
  VALUES (new.rowid, new.subject, new.from_name, new.from_email, new.body);
END;
CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, subject, from_name, from_email, body)
  VALUES ('delete', old.rowid, old.subject, old.from_name, old.from_email, old.body);
END;
CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, subject, from_name, from_email, body)
  VALUES ('delete', old.rowid, old.subject, old.from_name, old.from_email, old.body);
  INSERT INTO messages_fts(rowid, subject, from_name, from_email, body)
  VALUES (new.rowid, new.subject, new.from_name, new.from_email, new.body);
END;

CREATE TABLE IF NOT EXISTS sync_state (
  account_id   TEXT NOT NULL,
  folder       TEXT NOT NULL,
  uid_validity INTEGER,
  last_uid     INTEGER,
  updated_at   INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (account_id, folder)
);
