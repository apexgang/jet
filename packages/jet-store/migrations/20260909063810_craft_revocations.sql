-- Revocations are additive: replaying older signed metadata cannot undo one.
CREATE TABLE craft_revocations (
    sha256 TEXT PRIMARY KEY NOT NULL CHECK (length(sha256) = 64)
);
