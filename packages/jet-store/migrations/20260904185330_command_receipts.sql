-- Durable Command receipts for Actor-scoped idempotency (ADR-0093).
ALTER TABLE runs ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;

CREATE TRIGGER runs_revision_after_lifecycle_update
AFTER UPDATE OF lifecycle ON runs
WHEN OLD.lifecycle <> NEW.lifecycle
BEGIN
	UPDATE runs SET revision = revision + 1 WHERE run_id = NEW.run_id;
END;

CREATE TABLE command_receipts (
	actor_kind TEXT NOT NULL,
	actor_id TEXT NOT NULL,
	command_id TEXT NOT NULL,
	request_digest BLOB CHECK (length(request_digest) = 32),
	recorded_at_unix_ms INTEGER NOT NULL,
	outcome_version INTEGER,
	outcome TEXT CHECK (length(outcome) <= 65536),
	PRIMARY KEY (actor_kind, actor_id, command_id)
);
