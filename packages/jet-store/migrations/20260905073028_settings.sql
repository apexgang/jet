-- Mutable Settings, resolved from Plane, Project, and Conversation scopes
-- with built-in defaults supplied by the core (ADR-0085).
-- ASVS 2.2.1/2.2.3: the trusted storage layer allowlists scopes, bounds
-- every stored value, and rejects a scope without its subject identity.
CREATE TABLE settings (
	key TEXT NOT NULL,
	scope TEXT NOT NULL CHECK (
		scope IN ('plane', 'project', 'conversation')
	),
	scope_id TEXT,
	value TEXT NOT NULL CHECK (length(value) <= 4096),
	updated_at_unix_ms INTEGER NOT NULL,
	CHECK ((scope = 'plane') = (scope_id IS NULL))
);

-- One value per key and scope. Plane rows carry no subject identity, and
-- SQLite treats every NULL as distinct, so the coalesced expression is what
-- makes the Plane row unique.
CREATE UNIQUE INDEX settings_identity
	ON settings (key, scope, COALESCE(scope_id, ''));
