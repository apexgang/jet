-- Immutable plans for idempotent third-party Craft publication (ADR-0013,
-- ADR-0064). The externally visible manifest is written only by the Effect
-- whose acceptance committed with this row.
CREATE TABLE craft_installation_plans (
	effect_id TEXT PRIMARY KEY REFERENCES effects (effect_id),
	craft_id TEXT NOT NULL UNIQUE CHECK (
		length(craft_id) BETWEEN 1 AND 80
	),
	plan TEXT NOT NULL CHECK (length(plan) <= 262144)
);
