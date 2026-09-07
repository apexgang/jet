-- Orphan detection survives restart; only an interactive resolution clears it.
CREATE TABLE orphaned_executions (
    execution_id TEXT PRIMARY KEY NOT NULL,
    state TEXT NOT NULL CHECK (json_valid(state))
);
-- Immutable requests dispatched through the existing transactional outbox.
CREATE TABLE execution_resolutions (
    effect_id TEXT PRIMARY KEY NOT NULL REFERENCES effects(effect_id),
    request TEXT NOT NULL CHECK (json_valid(request))
);
