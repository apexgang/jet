-- Retain Git delivery attempts and per-Conversation bindings.
CREATE TABLE git_deliveries (
    delivery_id TEXT PRIMARY KEY NOT NULL,
    conversation_id TEXT NOT NULL REFERENCES conversations(conversation_id),
    acknowledged INTEGER NOT NULL DEFAULT 0 CHECK(acknowledged IN (0, 1)),
    state TEXT NOT NULL CHECK(length(state) <= 65536)
);
CREATE INDEX git_deliveries_conversation ON git_deliveries(conversation_id, delivery_id);

CREATE TABLE git_drafts (
    conversation_id TEXT PRIMARY KEY NOT NULL REFERENCES conversations(conversation_id),
    url TEXT NOT NULL CHECK(length(url) <= 2048)
);

CREATE TABLE git_branches (
    conversation_id TEXT PRIMARY KEY NOT NULL REFERENCES conversations(conversation_id),
    branch TEXT NOT NULL CHECK(length(branch) <= 240)
);
