CREATE TABLE scheduled_tasks (
    schedule_id TEXT PRIMARY KEY NOT NULL,
    conversation_id TEXT NOT NULL REFERENCES conversations(conversation_id),
    next_due_unix_ms INTEGER NOT NULL,
    state TEXT NOT NULL CHECK(json_valid(state))
);
CREATE INDEX scheduled_tasks_due ON scheduled_tasks(next_due_unix_ms, schedule_id);
CREATE INDEX scheduled_tasks_conversation ON scheduled_tasks(conversation_id);
