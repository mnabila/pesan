-- Body content lives entirely in the Maildir file now; the SQLite index keeps
-- only the envelope (sender/subject/flags). Rebuild the FTS index over just
-- subject + sender (dropping the `body` column) and clear the stored plain-text
-- bodies. Full-text search therefore matches sender/subject, not message text.
DROP TRIGGER IF EXISTS messages_ai;
DROP TRIGGER IF EXISTS messages_ad;
DROP TRIGGER IF EXISTS messages_au;
DROP TABLE IF EXISTS messages_fts;

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

INSERT INTO messages_fts(messages_fts) VALUES('rebuild');
UPDATE messages SET body = NULL;
