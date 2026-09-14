-- Plane transfer: moving one Conversation's Home Plane in phases, without
-- two simultaneously authoritative copies (ADR-0062, ADR-0070).
--
-- Every Conversation carries its authority: `home` while this Plane owns
-- it, `prepared` while it is a validated import that cannot run work or
-- fire schedules, and `relinquished` once this Plane has retired its
-- authority through a fence. The epoch counts the authorities the
-- Conversation has had; the Authority fence names the one it retired, so
-- a restored snapshot that still holds it cannot continue it.
ALTER TABLE conversations ADD COLUMN authority TEXT NOT NULL DEFAULT 'home'
	CHECK (authority IN ('home', 'prepared', 'relinquished'));
ALTER TABLE conversations
	ADD COLUMN authority_epoch INTEGER NOT NULL DEFAULT 1
	CHECK (authority_epoch >= 1);

-- One row per transfer this Plane took part in, on either side. The
-- source keeps it through `prepared` and `relinquished`; the target
-- through `prepared` and `committed`. The bundle hash binds every retry
-- to the bytes the transfer was prepared with.
CREATE TABLE plane_transfers (
	transfer_id TEXT PRIMARY KEY NOT NULL,
	conversation_id TEXT NOT NULL
		REFERENCES conversations (conversation_id) ON DELETE CASCADE,
	role TEXT NOT NULL CHECK (role IN ('source', 'target')),
	phase TEXT NOT NULL
		CHECK (phase IN ('prepared', 'relinquished', 'committed')),
	retired_epoch INTEGER NOT NULL CHECK (retired_epoch >= 1),
	peer_plane_id TEXT NOT NULL,
	bundle_sha256 TEXT NOT NULL
		CHECK (length(bundle_sha256) = 64
			AND bundle_sha256 NOT GLOB '*[^0-9a-f]*'),
	prepared_at_unix_ms INTEGER NOT NULL,
	settled_at_unix_ms INTEGER
);

CREATE INDEX plane_transfers_by_conversation
	ON plane_transfers (conversation_id);

-- How many Authority-fence records this store has applied, beside the
-- Deletion-ledger count it mirrors (ADR-0102).
ALTER TABLE plane ADD COLUMN fences_applied INTEGER NOT NULL DEFAULT 0;

-- Jet Trash learns the Transfer tombstone: the source's content, kept for
-- a while after the authority left. SQLite cannot widen a CHECK in place,
-- so the table is rebuilt, as the Autodelete migration did.
CREATE TABLE conversation_trash_widened (
	conversation_id TEXT PRIMARY KEY NOT NULL
		REFERENCES conversations (conversation_id) ON DELETE CASCADE,
	reason TEXT NOT NULL
		CHECK (reason IN ('manual', 'automatic', 'everywhere',
			'autodelete', 'autodelete_everywhere', 'transferred')),
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
