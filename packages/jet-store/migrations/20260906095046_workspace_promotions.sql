-- Workspace promotions: the user-directed application of a Workspace's
-- changes to a permanent checkout or branch of its Project (ADR-0025).
-- A row keeps exactly what the preview bound: the base, the Workspace and
-- destination trees, the destination commit, and the merged result, with
-- the Actor that confirmed it and where the promotion stands. A promotion
-- the preview could not settle is recorded as conflicted with its paths,
-- so the Workspace keeps inspectable conflict state and the destination
-- is never written; every other promotion is applied by a durable Effect
-- (ADR-0064) and settled by the outcome that Effect records.
-- ASVS 5.3.2: the storage layer bounds every name and path it keeps.
CREATE TABLE workspace_promotions (
	promotion_id TEXT PRIMARY KEY,
	workspace_id TEXT NOT NULL REFERENCES workspaces (workspace_id),
	actor_kind TEXT NOT NULL,
	actor_id TEXT NOT NULL,
	destination_kind TEXT NOT NULL CHECK (
		destination_kind IN ('local_checkout', 'branch')
	),
	destination_branch TEXT CHECK (
		(destination_branch IS NULL) = (destination_kind = 'local_checkout')
		AND (destination_branch IS NULL OR length(destination_branch) <= 1024)
	),
	base_commit TEXT NOT NULL CHECK (length(base_commit) BETWEEN 40 AND 64),
	workspace_tree TEXT NOT NULL
		CHECK (length(workspace_tree) BETWEEN 40 AND 64),
	destination_commit TEXT NOT NULL
		CHECK (length(destination_commit) BETWEEN 40 AND 64),
	destination_tree TEXT NOT NULL
		CHECK (length(destination_tree) BETWEEN 40 AND 64),
	result_tree TEXT NOT NULL CHECK (length(result_tree) BETWEEN 40 AND 64),
	changed_paths INTEGER NOT NULL CHECK (changed_paths >= 0),
	state TEXT NOT NULL CHECK (
		state IN ('applying', 'promoted', 'conflicted', 'failed', 'outcome_unknown')
	),
	recorded_at_unix_ms INTEGER NOT NULL,
	settled_at_unix_ms INTEGER CHECK (
		(settled_at_unix_ms IS NULL) = (state = 'applying')
	)
);

CREATE INDEX workspace_promotions_by_workspace
	ON workspace_promotions (workspace_id);

CREATE TABLE workspace_promotion_conflicts (
	promotion_id TEXT NOT NULL REFERENCES workspace_promotions (promotion_id),
	position INTEGER NOT NULL CHECK (position >= 0),
	path TEXT NOT NULL CHECK (length(path) <= 4096),
	kind TEXT NOT NULL CHECK (kind IN ('diverged', 'untracked')),
	PRIMARY KEY (promotion_id, position)
);

-- An Effect that applies a promotion names it, the way one that starts a
-- Run names the Run.
ALTER TABLE effects
	ADD COLUMN promotion_id TEXT REFERENCES workspace_promotions (promotion_id);
