-- 0003: the mic words the echo gate removed from a run (plan 15, section
-- 3.2), as a JSON list [{start_ms, end_ms, text, coefficient}], NULL when the
-- gate never ran. Kept out of `engine_info`, which `get_meeting` returns
-- whole on every edit (a two-hour speaker recording drops about 12,000
-- words); only the meeting details line and its Restore action read it.
ALTER TABLE transcript_runs ADD COLUMN echo_dropped TEXT;
