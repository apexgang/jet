-- The owner-only Security audit: an integrity-chained record of sensitive
-- core decisions, kept apart from the Event journal (ADR-0105).
-- ASVS 16.3.1 through 16.3.4: every record commits its chain link with the
-- decision it describes, and no column here can hold a credential, a
-- prompt, terminal output, or file content.

-- One authority epoch of the chain. A new epoch is begun explicitly by the
-- owner after an integrity failure and records the gap it leaves behind.
CREATE TABLE audit_epochs (
	epoch INTEGER PRIMARY KEY CHECK (epoch >= 1),
	started_at_unix_ms INTEGER NOT NULL,
	-- The head the preceding epoch was last known to have, and why the
	-- chain restarts. Both are absent for the first epoch, which succeeds
	-- nothing.
	preceding_sequence INTEGER CHECK (preceding_sequence >= 0),
	preceding_entry_hash BLOB CHECK (length(preceding_entry_hash) = 32),
	gap_reason TEXT CHECK (length(gap_reason) <= 128),
	CHECK ((preceding_sequence IS NULL) = (gap_reason IS NULL)),
	CHECK ((preceding_sequence IS NULL) = (preceding_entry_hash IS NULL))
);

-- ASVS 2.2.1/2.2.3: the trusted storage layer allowlists every risk and
-- outcome, bounds every stored value, and fixes the width of every hash.
CREATE TABLE security_audit (
	sequence INTEGER PRIMARY KEY,
	epoch INTEGER NOT NULL REFERENCES audit_epochs (epoch),
	record_id TEXT NOT NULL UNIQUE,
	recorded_at_unix_ms INTEGER NOT NULL,
	plane_id TEXT NOT NULL,
	actor_kind TEXT NOT NULL CHECK (length(actor_kind) <= 64),
	actor_id TEXT NOT NULL CHECK (length(actor_id) <= 64),
	target_kind TEXT NOT NULL CHECK (length(target_kind) <= 64),
	-- The opaque identifier the chain is computed over. It is derived from
	-- the target, so clearing `target_id` when the target is deleted leaves
	-- the chain intact and the record still says what it was about.
	target_reference BLOB NOT NULL CHECK (length(target_reference) = 32),
	-- The target's own identity, for as long as the Plane keeps the target.
	target_id TEXT CHECK (length(target_id) <= 128),
	decision TEXT NOT NULL CHECK (length(decision) <= 64),
	risk TEXT NOT NULL CHECK (risk IN ('routine', 'elevated', 'destructive')),
	outcome TEXT NOT NULL CHECK (
		outcome IN ('succeeded', 'denied', 'failed')
	),
	entry_hash BLOB NOT NULL CHECK (length(entry_hash) = 32)
);

CREATE INDEX security_audit_by_target ON security_audit (target_reference);

-- Positions the audit keeps outside its own rows: the sequence to assign
-- next, which never goes backwards even after retention removes rows, and
-- the record retention last removed, whose hash is where verification of
-- the remaining chain starts. Sequence zero means nothing was removed yet
-- and verification starts from the first epoch's genesis.
CREATE TABLE audit_state (
	singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
	next_sequence INTEGER NOT NULL CHECK (next_sequence >= 1),
	retained_after_epoch INTEGER NOT NULL CHECK (retained_after_epoch >= 0),
	retained_after_sequence INTEGER NOT NULL CHECK (
		retained_after_sequence >= 0
	),
	retained_after_hash BLOB CHECK (length(retained_after_hash) = 32),
	CHECK (
		(retained_after_sequence = 0)
			= (retained_after_hash IS NULL)
	),
	CHECK ((retained_after_sequence = 0) = (retained_after_epoch = 0))
);

INSERT INTO audit_state (
	singleton, next_sequence, retained_after_epoch, retained_after_sequence,
	retained_after_hash
) VALUES (1, 1, 0, 0, NULL);
