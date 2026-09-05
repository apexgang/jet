-- The GUI clients this Plane has Paired with, and the confirmation an
-- offer needs before one of them is written (ADR-0017).
-- ASVS 2.2.1/2.2.3 and 14.1.4: the trusted storage layer allowlists every
-- algorithm and access state and fixes the width of the key. The durable
-- credential here is a public key, so nothing in this table is secret; the
-- private half never leaves the client installation that generated it.

-- Who confirmed the open offer, which is the person at the target agreeing
-- that the authentication string on both screens is the same one. It is
-- added rather than folded into the offer's state so a release that does
-- not know about it still reads every offer it wrote.
ALTER TABLE pairing_offers ADD COLUMN confirmed_by TEXT;

-- One row per Client identity: pairing again replaces the key the Plane
-- holds for it rather than accumulating a second pairing for one client.
CREATE TABLE paired_clients (
	client_id TEXT PRIMARY KEY,
	key_algorithm TEXT NOT NULL CHECK (key_algorithm IN ('ed25519')),
	public_key BLOB NOT NULL CHECK (length(public_key) = 32),
	-- The Pairing protocol the key was established under, retained so a
	-- later release can tell what it has to keep working with.
	pairing_protocol TEXT NOT NULL CHECK (length(pairing_protocol) <= 64),
	-- Whether the client may control the Plane right now. Revoking deletes
	-- the row and its key; disabling keeps both.
	access TEXT NOT NULL CHECK (access IN ('enabled', 'disabled')),
	paired_at_unix_ms INTEGER NOT NULL
);
