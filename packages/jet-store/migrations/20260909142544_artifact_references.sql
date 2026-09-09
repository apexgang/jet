CREATE TABLE artifact_references (
    run_id TEXT NOT NULL REFERENCES runs(run_id),
    sha256 TEXT NOT NULL CHECK(length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
    size INTEGER NOT NULL CHECK(size >= 0),
    PRIMARY KEY (run_id, sha256)
);
CREATE INDEX artifact_references_hash ON artifact_references(sha256);
