-- Plane identity and daemon lifecycle counters.
CREATE TABLE plane (
	singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
	plane_id TEXT NOT NULL,
	daemon_starts INTEGER NOT NULL DEFAULT 0
);
