-- Add migration script here
-- Destination-owned launch material captured when a Conversation fork is
-- created (ADR-0035). Nothing here references a source row: deleting or
-- retaining the source has no effect on the fork's first execution.
CREATE TABLE conversation_fork_launches (
	conversation_id TEXT PRIMARY KEY NOT NULL
		REFERENCES conversations (conversation_id) ON DELETE CASCADE,
	checkpoint_commit TEXT NOT NULL
		CHECK (length(checkpoint_commit) BETWEEN 40 AND 64),
	checkpoint_tree TEXT NOT NULL
		CHECK (length(checkpoint_tree) BETWEEN 40 AND 64),
	source_craft TEXT CHECK (
		source_craft IS NULL OR length(source_craft) <= 524288
	),
	source_native_conversation TEXT CHECK (
		source_native_conversation IS NULL
		OR length(source_native_conversation) BETWEEN 1 AND 4096
	),
	context TEXT NOT NULL CHECK (length(context) <= 32768)
);
