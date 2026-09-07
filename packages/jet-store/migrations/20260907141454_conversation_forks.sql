-- Explicit provenance for a Conversation created from an immutable Change
-- checkpoint (ADR-0035). Provenance keeps opaque source identities without a
-- foreign key so source and fork retain independent lifecycles. The trusted
-- Core validates the retained checkpoint in the transaction that inserts the
-- fork.
ALTER TABLE conversations
	ADD COLUMN fork_source_conversation_id TEXT;
ALTER TABLE conversations ADD COLUMN fork_source_run_id TEXT;
ALTER TABLE conversations
	ADD COLUMN fork_checkpoint_turn INTEGER
		CHECK (fork_checkpoint_turn > 0);

CREATE INDEX conversations_by_fork_source
	ON conversations (fork_source_conversation_id)
	WHERE fork_source_conversation_id IS NOT NULL;
