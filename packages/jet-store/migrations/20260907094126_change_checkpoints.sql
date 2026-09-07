CREATE TABLE change_checkpoints (
    run_id TEXT NOT NULL REFERENCES runs(run_id),
    turn INTEGER NOT NULL CHECK (turn > 0),
    payload TEXT NOT NULL CHECK (json_valid(payload)),
    PRIMARY KEY (run_id, turn)
);
CREATE TRIGGER change_checkpoints_immutable BEFORE UPDATE ON change_checkpoints
BEGIN SELECT RAISE(ABORT, 'Change checkpoints are immutable'); END;
