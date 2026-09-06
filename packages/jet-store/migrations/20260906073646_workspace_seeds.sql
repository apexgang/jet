-- What a Workspace was seeded with from its Project's Local checkout
-- (ADR-0025). The seed is an immutable Git tree the core captured before
-- the Workspace existed and applied once it did; the row keeps the tree's
-- identity and how many paths it changed against the base. A Workspace
-- seeded with nothing keeps both columns NULL.
-- ASVS 5.3.2: the storage layer bounds the object name it keeps.
ALTER TABLE workspaces
	ADD COLUMN seed_tree TEXT
		CHECK (seed_tree IS NULL OR length(seed_tree) BETWEEN 40 AND 64);
ALTER TABLE workspaces
	ADD COLUMN seed_changed_paths INTEGER
		CHECK (
			(seed_changed_paths IS NULL) = (seed_tree IS NULL)
			AND (seed_changed_paths IS NULL OR seed_changed_paths >= 0)
		);
