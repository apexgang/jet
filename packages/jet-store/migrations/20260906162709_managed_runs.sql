-- Expand-only execution metadata; lifecycle remains authoritative in runs.
CREATE TABLE run_executions (
    run_id TEXT NOT NULL PRIMARY KEY REFERENCES runs(run_id),
    plan TEXT NOT NULL CHECK (json_valid(plan)),
    state TEXT NOT NULL CHECK (json_valid(state))
);
