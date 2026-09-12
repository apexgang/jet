-- Autodelete rules: a natural-language prompt compiled once by the Utility
-- model into a bounded inactivity predicate, which executes only after its
-- owner approves the interpretation shown to them (ADR-0015, ADR-0099).
--
-- `inactive_days` is the interpretation the owner edited by hand or
-- approved; while NULL the draft is whatever the compiling Utility job
-- produced. `approved_at_unix_ms` marks an approved rule; any edit clears
-- it. `everywhere` records the separate authorization to request native
-- Harness deletion for matches and needs an approved rule.
CREATE TABLE autodelete_rules (
	rule_id TEXT PRIMARY KEY NOT NULL,
	prompt TEXT NOT NULL CHECK (length(prompt) BETWEEN 1 AND 4096),
	utility_job_id TEXT NOT NULL REFERENCES utility_jobs (job_id),
	inactive_days INTEGER
		CHECK (inactive_days IS NULL OR inactive_days BETWEEN 1 AND 36500),
	approved_at_unix_ms INTEGER
		CHECK (approved_at_unix_ms IS NULL OR inactive_days IS NOT NULL),
	everywhere INTEGER NOT NULL DEFAULT 0
		CHECK (everywhere IN (0, 1))
		CHECK (everywhere = 0 OR approved_at_unix_ms IS NOT NULL),
	created_at_unix_ms INTEGER NOT NULL,
	updated_at_unix_ms INTEGER NOT NULL
);

-- Jet Trash learns two reasons: a match of an approved Autodelete rule,
-- and one whose rule was separately authorized to delete everywhere.
-- SQLite cannot widen a CHECK in place, so the table is rebuilt. The store
-- opens with foreign keys enforced; nothing references conversation_trash,
-- so dropping it under that setting is safe.
CREATE TABLE conversation_trash_widened (
	conversation_id TEXT PRIMARY KEY NOT NULL
		REFERENCES conversations (conversation_id) ON DELETE CASCADE,
	reason TEXT NOT NULL
		CHECK (reason IN ('manual', 'automatic', 'everywhere',
			'autodelete', 'autodelete_everywhere')),
	trashed_at_unix_ms INTEGER NOT NULL,
	expires_at_unix_ms INTEGER NOT NULL
		CHECK (expires_at_unix_ms >= trashed_at_unix_ms)
);

INSERT INTO conversation_trash_widened
	SELECT conversation_id, reason, trashed_at_unix_ms, expires_at_unix_ms
	FROM conversation_trash;

DROP TABLE conversation_trash;

ALTER TABLE conversation_trash_widened RENAME TO conversation_trash;

CREATE INDEX conversation_trash_by_expiry
	ON conversation_trash (expires_at_unix_ms);
