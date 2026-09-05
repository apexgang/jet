-- The Plane's Pairing gate: whether a new GUI client may begin Pairing at
-- all (ADR-0017).
-- ASVS 2.2.1/2.2.3: the trusted storage layer allowlists both states and
-- keeps the switch in one row, so the Plane cannot hold two answers to the
-- question of whether it is accepting new clients.
CREATE TABLE pairing_gate (
	singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
	state TEXT NOT NULL CHECK (state IN ('open', 'closed')),
	changed_at_unix_ms INTEGER NOT NULL
);

-- A Plane accepts no new client until its owner says so. The gate is the
-- one switch that widens who may control the Plane, so it fails closed and
-- an owner opens it for as long as the pairing takes.
INSERT INTO pairing_gate (singleton, state, changed_at_unix_ms)
	VALUES (1, 'closed', 0);
