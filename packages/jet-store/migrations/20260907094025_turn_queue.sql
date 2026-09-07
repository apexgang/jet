-- Bounded current queue; completed admission history remains in the journal.
CREATE TABLE turn_queues (
    conversation_id TEXT PRIMARY KEY REFERENCES conversations(conversation_id) ON DELETE CASCADE,
    state TEXT NOT NULL CHECK (json_valid(state)),
    pending_count INTEGER NOT NULL CHECK (pending_count >= 0)
);
CREATE INDEX pending_turn_queues ON turn_queues(conversation_id) WHERE pending_count > 0;
