-- How many Deletion-ledger records this store has applied. It travels
-- with every snapshot, so a copy says which deletions it predates
-- (ADR-0102).
ALTER TABLE plane ADD COLUMN deletions_applied INTEGER NOT NULL DEFAULT 0;
