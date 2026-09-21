-- The search index catches up by reading the semantic Events after the
-- position it has derived through (ADR-0036). Without an index of their
-- own, that read walks every operational Event behind the last semantic
-- one, which is most of a long Run's journal, and it does so on every start
-- of a Plane whose newest Events are operational: a walk that alone could
-- spend the ready-time budget (ADR-0022). Semantic Events are the minority,
-- so the index stays small.
CREATE INDEX events_semantic_by_sequence ON events (sequence)
	WHERE class = 'semantic';
