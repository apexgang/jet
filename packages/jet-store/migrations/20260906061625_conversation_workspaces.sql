-- Where a Conversation does its work, and the managed Workspaces that
-- isolate Conversations from one another (ADR-0025).
-- A Conversation records which Project it works in and whether it does so
-- in a managed Workspace or in the Project's own Local checkout. A
-- Workspace is a Git worktree owned by exactly one Conversation, at a
-- Jet-owned root, detached from the immutable base commit it started from.
-- ASVS 5.3.2: the trusted storage layer bounds every path and revision it
-- keeps; the core validates them before they reach a row.
ALTER TABLE conversations
	ADD COLUMN working_tree TEXT NOT NULL DEFAULT 'none';
ALTER TABLE conversations
	ADD COLUMN project_id TEXT REFERENCES projects (project_id);

CREATE INDEX conversations_by_project
	ON conversations (project_id, working_tree);

CREATE TABLE workspaces (
	workspace_id TEXT PRIMARY KEY,
	conversation_id TEXT NOT NULL UNIQUE
		REFERENCES conversations (conversation_id),
	project_id TEXT NOT NULL REFERENCES projects (project_id),
	root TEXT NOT NULL UNIQUE CHECK (length(root) <= 4096),
	base_selection TEXT NOT NULL CHECK (length(base_selection) <= 1024),
	base_commit TEXT NOT NULL CHECK (length(base_commit) BETWEEN 40 AND 64),
	created_at_unix_ms INTEGER NOT NULL
);

CREATE INDEX workspaces_by_project ON workspaces (project_id);
