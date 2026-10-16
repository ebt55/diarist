-- 0002: meeting and transcript run ids are never reused after a delete
-- (docs/02-data-model.md, "Ids"). Rowid tables hand out max(id) + 1, so
-- deleting the newest meeting gave its id to the next one, and anything that
-- remembered the old meeting by id saw the new one. The store now takes the
-- next id from this table and advances it in the same transaction as the
-- insert (an explicit id in the INSERT). No table is rebuilt: seven tables
-- cascade on meetings(id).
CREATE TABLE counters (
  key   TEXT PRIMARY KEY,
  value INTEGER NOT NULL
);

INSERT INTO counters (key, value)
  SELECT 'next_meeting_id', COALESCE(MAX(id), 0) + 1 FROM meetings;
INSERT INTO counters (key, value)
  SELECT 'next_run_id', COALESCE(MAX(id), 0) + 1 FROM transcript_runs;
