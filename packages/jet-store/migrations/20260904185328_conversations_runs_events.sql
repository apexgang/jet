-- Conversations, their Runs, and the Plane-local Event journal
-- (ADR-0001, ADR-0020, ADR-0096).
CREATE TABLE conversations (
	conversation_id TEXT PRIMARY KEY,
	retention TEXT NOT NULL,
	created_at_unix_ms INTEGER NOT NULL
);

CREATE TABLE runs (
	run_id TEXT PRIMARY KEY,
	conversation_id TEXT NOT NULL REFERENCES conversations (conversation_id),
	lifecycle TEXT NOT NULL,
	created_at_unix_ms INTEGER NOT NULL,
	ended_at_unix_ms INTEGER
);

CREATE INDEX runs_by_conversation ON runs (conversation_id);

CREATE TABLE events (
	sequence INTEGER PRIMARY KEY AUTOINCREMENT,
	event_id TEXT NOT NULL UNIQUE,
	actor_kind TEXT NOT NULL,
	actor_id TEXT,
	recorded_at_unix_ms INTEGER NOT NULL,
	conversation_id TEXT,
	run_id TEXT,
	kind TEXT NOT NULL,
	payload_version INTEGER NOT NULL,
	payload TEXT NOT NULL CHECK (length(payload) <= 65536)
);

CREATE INDEX events_by_conversation ON events (conversation_id, sequence);
