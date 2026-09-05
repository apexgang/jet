-- Registered Projects: the Git working trees an interactive user granted
-- Jet access to through an explicit Path grant (ADR-0025, ADR-0101).
-- ASVS 2.2.1/2.2.3 and 5.3.2: the trusted storage layer keeps one canonical
-- absolute root per Project, bounds it, and records the authenticated Actor
-- that granted it. Ordinary file Commands address files through the
-- Project identity and a validated relative path, never through this root
-- directly.
CREATE TABLE projects (
	project_id TEXT PRIMARY KEY,
	root TEXT NOT NULL UNIQUE CHECK (length(root) <= 4096),
	actor_kind TEXT NOT NULL CHECK (length(actor_kind) <= 64),
	actor_id TEXT NOT NULL CHECK (length(actor_id) <= 64),
	registered_at_unix_ms INTEGER NOT NULL
);
