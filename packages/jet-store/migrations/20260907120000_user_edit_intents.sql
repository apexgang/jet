-- Write-ahead intent for restart-safe direct file replacement (ADR-0064).
CREATE TABLE user_edit_intents (
	actor_kind TEXT NOT NULL CHECK (actor_kind = 'interactive_client'),
	actor_id TEXT NOT NULL,
	command_id TEXT NOT NULL,
	request_digest BLOB NOT NULL CHECK (length(request_digest) = 32),
	recorded_at_unix_ms INTEGER NOT NULL,
	plan TEXT NOT NULL CHECK (json_valid(plan) AND length(plan) <= 1048576),
	PRIMARY KEY (actor_kind, actor_id, command_id)
);
