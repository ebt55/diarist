-- 0004: the ASR's own text of a word where punctuation changed it (plan 16,
-- section 3.5). `text` holds the display text (punctuated, sentence-start
-- capitals), because segment regrouping rebuilds `segments.text` from it;
-- `raw_text` is NULL where it equals `text`. Every rebuild (Add or Remove
-- punctuation, Restore) starts from `COALESCE(raw_text, text)`.
ALTER TABLE words ADD COLUMN raw_text TEXT;
