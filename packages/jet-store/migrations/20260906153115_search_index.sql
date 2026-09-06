-- The Plane-local Search index: a full-text projection of human-visible
-- Conversation content that the core derives from committed semantic
-- Events. It is never an authority: every row can be rebuilt from the
-- journal, and dropping it changes no Conversation (ADR-0036, ADR-0057).
--
-- Each document names the Conversation it belongs to and the Event
-- sequence that carried the content, which is the stable reference a hit
-- reports and the key forgetting removes by. FTS5 tables take no CHECK
-- constraints, so the store bounds document bodies in code.
CREATE VIRTUAL TABLE search_documents USING fts5 (
	conversation_id UNINDEXED,
	sequence UNINDEXED,
	field UNINDEXED,
	body,
	tokenize = 'unicode61 remove_diacritics 2'
);

-- How far into the journal the index has been derived. An interrupted
-- indexer resumes strictly after this position, so index writes are
-- idempotent per Event and never touch the journal (ADR-0078).
CREATE TABLE search_index_state (
	singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
	indexed_through_sequence INTEGER NOT NULL
		CHECK (indexed_through_sequence >= 0)
);

INSERT INTO search_index_state (singleton, indexed_through_sequence)
VALUES (1, 0);
