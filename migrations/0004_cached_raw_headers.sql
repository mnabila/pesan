-- Persist the raw RFC822 header block alongside the cached body, so the reader
-- can show full headers offline after a message has been opened once.
ALTER TABLE messages ADD COLUMN raw_headers TEXT;
