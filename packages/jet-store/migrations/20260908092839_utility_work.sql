CREATE TABLE utility_jobs (
    job_id TEXT PRIMARY KEY NOT NULL,
    state TEXT NOT NULL CHECK(length(state) <= 65536)
);
