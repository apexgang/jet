-- Imported conversations: Harness-native Conversation identities discovered
-- outside Jet and registered so a managed Run can continue them (ADR-0010).
-- A row keeps the identity as the Harness spells it, the directory the
-- Harness reported working in, and the interactive Actor that registered
-- it. Whether a live process still holds the identity is observed, never
-- stored. One identity is registered once; the Conversation that continues
-- it, when one has been created, points back at the row.
-- ASVS 5.3.2: the trusted storage layer bounds every identity and path it
-- keeps; the core validates them before they reach a row.
CREATE TABLE imported_conversations (
	import_id TEXT PRIMARY KEY,
	harness TEXT NOT NULL CHECK (length(harness) BETWEEN 1 AND 128),
	native_conversation TEXT NOT NULL
		CHECK (length(native_conversation) BETWEEN 1 AND 1024),
	working_directory TEXT CHECK (length(working_directory) <= 4096),
	actor_kind TEXT NOT NULL,
	actor_id TEXT NOT NULL,
	imported_at_unix_ms INTEGER NOT NULL,
	UNIQUE (harness, native_conversation)
);

ALTER TABLE conversations
	ADD COLUMN import_id TEXT REFERENCES imported_conversations (import_id);

CREATE UNIQUE INDEX conversations_by_import
	ON conversations (import_id)
	WHERE import_id IS NOT NULL;
