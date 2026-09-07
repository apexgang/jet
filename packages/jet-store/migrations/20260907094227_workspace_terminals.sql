-- Workspace-owned terminal identity and durable launch/close Effects.
CREATE TABLE workspace_terminals (
 terminal_id TEXT PRIMARY KEY NOT NULL,
 workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
 state TEXT NOT NULL CHECK (state IN ('opening','open','closing','closed','unavailable')),
 plan TEXT NOT NULL
);
CREATE INDEX terminals_by_workspace ON workspace_terminals(workspace_id);
ALTER TABLE effects ADD COLUMN terminal_id TEXT REFERENCES workspace_terminals(terminal_id);
