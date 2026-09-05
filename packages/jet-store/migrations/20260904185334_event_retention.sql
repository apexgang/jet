-- Event retention classification and sequence tombstones. The high-water row
-- survives deletion so snapshots never move backward (ADR-0078, ADR-0092).
-- ASVS 2.2.1/2.2.2: the trusted storage layer allowlists Event classes.
ALTER TABLE events ADD COLUMN class TEXT NOT NULL DEFAULT 'semantic'
	CHECK (class IN ('semantic', 'operational'));

CREATE TABLE event_journal_state (
	singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
	high_water_sequence INTEGER NOT NULL CHECK (high_water_sequence >= 0),
	minimum_replay_cursor INTEGER NOT NULL CHECK (minimum_replay_cursor >= 0),
	CHECK (minimum_replay_cursor <= high_water_sequence)
);

INSERT INTO event_journal_state (
	singleton, high_water_sequence, minimum_replay_cursor
) SELECT 1, COALESCE(MAX(sequence), 0), 0 FROM events;

CREATE TRIGGER events_advance_high_water
AFTER INSERT ON events
BEGIN
	UPDATE event_journal_state
	SET high_water_sequence = NEW.sequence
	WHERE singleton = 1;
END;
