-- Durable external-work Effect outbox (ADR-0064, ADR-0067).
-- ASVS 2.2.1/2.2.3: CHECK constraints allow only valid state, safety, key,
-- and retry-bound combinations at the trusted storage layer.
CREATE TABLE effects (
	effect_id TEXT PRIMARY KEY,
	command_id TEXT NOT NULL,
	run_id TEXT REFERENCES runs (run_id),
	kind TEXT NOT NULL,
	safety TEXT NOT NULL CHECK (
		safety IN ('read_only', 'idempotent', 'ambiguous')
	),
	external_key TEXT,
	max_attempts INTEGER NOT NULL CHECK (max_attempts >= 1),
	state TEXT NOT NULL CHECK (
		state IN ('pending', 'in_flight', 'completed', 'failed', 'outcome_unknown')
	),
	attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
	CHECK (
		(safety = 'idempotent' AND external_key IS NOT NULL)
		OR (safety <> 'idempotent' AND external_key IS NULL)
	),
	CHECK (safety <> 'ambiguous' OR max_attempts = 1)
);

CREATE INDEX unresolved_effects ON effects (state);
