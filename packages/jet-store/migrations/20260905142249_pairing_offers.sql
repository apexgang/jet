-- The one Pairing offer a Plane may have open, and the claim a GUI client
-- makes against it (ADR-0017).
-- ASVS 2.2.1/2.2.3 and 14.1.4: the trusted storage layer allowlists every
-- state, fixes the width of every hash and key, and has no column able to
-- hold the offer's one-time secret. The secret is disclosed once, to the
-- owner who opened the offer, and reaches this table only as a salted
-- digest it cannot be recovered from.
--
-- One row, because a Plane pairs with one client at a time: opening an
-- offer replaces whatever was open, so a person reading a code off the
-- target's screen knows it is the code the Plane is waiting for.
CREATE TABLE pairing_offers (
	singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
	offer_id TEXT NOT NULL,
	method TEXT NOT NULL CHECK (method IN ('manual_code', 'qr_payload')),
	-- The reachable endpoint a QR payload advertises, which a manual code
	-- does not carry: its user already knows where the Plane is.
	endpoint TEXT CHECK (length(endpoint) <= 255),
	secret_salt BLOB NOT NULL CHECK (length(secret_salt) = 16),
	secret_digest BLOB NOT NULL CHECK (length(secret_digest) = 32),
	state TEXT NOT NULL CHECK (
		state IN ('offered', 'awaiting_confirmation', 'invalidated')
	),
	-- Why the offer stopped being usable, which only an invalidated offer
	-- has and every invalidated offer has.
	invalidation TEXT CHECK (
		invalidation IN ('too_many_attempts', 'gate_closed')
	),
	failed_attempts INTEGER NOT NULL CHECK (failed_attempts >= 0),
	opened_by TEXT NOT NULL,
	opened_at_unix_ms INTEGER NOT NULL,
	expires_at_unix_ms INTEGER NOT NULL,
	-- What the claiming client presented. An offer no client has claimed
	-- holds none of it, and one that has been claimed holds all of it.
	claimed_by TEXT,
	key_algorithm TEXT CHECK (key_algorithm IN ('ed25519')),
	public_key BLOB CHECK (length(public_key) = 32),
	challenge BLOB CHECK (length(challenge) = 32),
	authentication_string TEXT CHECK (length(authentication_string) <= 16),
	CHECK ((method = 'qr_payload') = (endpoint IS NOT NULL)),
	CHECK ((state = 'invalidated') = (invalidation IS NOT NULL)),
	CHECK ((claimed_by IS NULL) = (key_algorithm IS NULL)),
	CHECK ((claimed_by IS NULL) = (public_key IS NULL)),
	CHECK ((claimed_by IS NULL) = (challenge IS NULL)),
	CHECK ((claimed_by IS NULL) = (authentication_string IS NULL)),
	-- An unclaimed offer names no client, and a claimed one names one.
	CHECK (state <> 'offered' OR claimed_by IS NULL),
	CHECK (state <> 'awaiting_confirmation' OR claimed_by IS NOT NULL)
);
