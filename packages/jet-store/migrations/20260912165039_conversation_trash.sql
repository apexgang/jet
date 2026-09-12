-- Jet Trash: Conversations staged for permanent deletion, with the reason
-- they were staged and when their grace period ends (ADR-0011, ADR-0015).
-- A row is the whole grace state; restoring removes it, and expiry removes
-- the Conversation itself through the Deletion ledger (ADR-0102).
CREATE TABLE conversation_trash (
	conversation_id TEXT PRIMARY KEY NOT NULL
		REFERENCES conversations (conversation_id) ON DELETE CASCADE,
	reason TEXT NOT NULL
		CHECK (reason IN ('manual', 'automatic', 'everywhere')),
	trashed_at_unix_ms INTEGER NOT NULL,
	expires_at_unix_ms INTEGER NOT NULL
		CHECK (expires_at_unix_ms >= trashed_at_unix_ms)
);

CREATE INDEX conversation_trash_by_expiry
	ON conversation_trash (expires_at_unix_ms);
