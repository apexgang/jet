-- No-Visa effects reserve their identity before external work begins.
CREATE TABLE remote_operations (
    client_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    request_digest BLOB NOT NULL CHECK(length(request_digest) = 32),
    recorded_at_unix_ms INTEGER NOT NULL,
    origin_plane_id TEXT NOT NULL CHECK(length(origin_plane_id) = 36),
    origin_conversation_id TEXT NOT NULL CHECK(length(origin_conversation_id) = 36),
    origin_run_id TEXT NOT NULL CHECK(length(origin_run_id) = 36),
    workspace_id TEXT NOT NULL CHECK(length(workspace_id) = 36),
    request TEXT CHECK(request IS NULL OR length(request) <= 1048576),
    state TEXT NOT NULL CHECK(state IN ('pending', 'approved', 'running', 'finished', 'denied', 'expired')),
    result TEXT CHECK(result IS NULL OR length(result) <= 1048576),
    PRIMARY KEY (client_id, operation_id)
);
CREATE INDEX remote_operation_expiry ON remote_operations(recorded_at_unix_ms) WHERE state <> 'expired';
