-- Message content of record moved to the interoperable Maildir file store; the
-- `messages` table is now just the fast index. Clear the now-unused raw HTML and
-- raw header blobs (they are re-derived from the Maildir file on open) to
-- reclaim space. The columns are left in place - dropping a column desyncs the
-- external-content FTS rowids - but stay NULL from here on.
UPDATE messages SET raw_html = NULL, raw_headers = NULL;
