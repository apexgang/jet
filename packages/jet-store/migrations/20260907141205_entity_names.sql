-- Expand-only name columns remain nullable so the previous release can still
-- insert rows during the rollback window. The current reader supplies the
-- deterministic fallback for those legacy rows (ADR-0044, ADR-0073).
ALTER TABLE conversations ADD COLUMN name TEXT
	CHECK (name IS NULL OR length(CAST(name AS BLOB)) BETWEEN 1 AND 256);
ALTER TABLE conversations ADD COLUMN name_source TEXT
	CHECK (name_source IS NULL OR name_source IN (
		'manual', 'utility', 'harness_native', 'deterministic'
	))
	CHECK ((name IS NULL) = (name_source IS NULL));
ALTER TABLE conversations ADD COLUMN revision INTEGER NOT NULL DEFAULT 1
	CHECK (revision >= 1);

ALTER TABLE runs ADD COLUMN name TEXT
	CHECK (name IS NULL OR length(CAST(name AS BLOB)) BETWEEN 1 AND 256);
ALTER TABLE runs ADD COLUMN name_source TEXT
	CHECK (name_source IS NULL OR name_source IN (
		'manual', 'utility', 'harness_native', 'deterministic'
	))
	CHECK ((name IS NULL) = (name_source IS NULL));

-- Names live in a new projection so the previous release never reads an
-- unfamiliar `field = 'name'` from its existing FTS table during rollback.
CREATE VIRTUAL TABLE search_name_documents USING fts5 (
	conversation_id UNINDEXED,
	sequence UNINDEXED,
	field UNINDEXED,
	body,
	tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TABLE search_name_index_state (
	singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
	reconciled_through_sequence INTEGER NOT NULL
		CHECK (reconciled_through_sequence >= 0)
);

INSERT INTO search_name_index_state (
	singleton, reconciled_through_sequence
) VALUES (1, 0);
