-- Persist the original HTML source alongside the plain-text body, so the
-- reader can render width-aware styled lines (and the browser export can use
-- the faithful source) without refetching. `NULL` for plain-text mail and for
-- rows cached before this migration, which keep rendering from `body`.
ALTER TABLE messages ADD COLUMN raw_html TEXT;
