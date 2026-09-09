-- Normalized Usage records (ADR-0023). Every row carries the provenance a
-- Query needs to answer honestly: where the measurement came from, what it
-- covers, whether it is estimated, whether it is final, and the Account
-- binding, Model, Conversation, Run, and time it belongs to. The Plane is
-- the store itself, which is authoritative only for its own Plane
-- (ADR-0016), so no row repeats it.

-- Jet's own accounting of what a Harness reported it consumed. `measurement`
-- is what stops one measurement from being counted twice: the Craft's native
-- usage identity where the Harness supplies one, and the turn it covers
-- otherwise. A repeated measurement replaces its row and is never summed
-- into it (ADR-0023).
CREATE TABLE usage_observations (
    observation_id TEXT PRIMARY KEY NOT NULL CHECK (length(observation_id) = 36),
    conversation_id TEXT NOT NULL REFERENCES conversations (conversation_id),
    run_id TEXT NOT NULL REFERENCES runs (run_id),
    measurement TEXT NOT NULL CHECK (length(measurement) BETWEEN 1 AND 256),
    native_usage_id TEXT CHECK (native_usage_id IS NULL OR length(native_usage_id) BETWEEN 1 AND 256),
    -- The binding an unbind removes stays named here: forgetting what may
    -- authenticate does not rewrite what was already consumed, so this
    -- column deliberately carries no foreign key.
    binding_id TEXT CHECK (binding_id IS NULL OR length(binding_id) = 36),
    provider TEXT CHECK (provider IS NULL OR length(provider) <= 64),
    model TEXT CHECK (model IS NULL OR length(model) <= 128),
    -- Whether the numbers cover one turn or the Run so far. A Run's total is
    -- the freshest Run-scoped record when one exists and the sum of its
    -- turn-scoped records otherwise; the two are never added together.
    scope TEXT NOT NULL CHECK (scope IN ('turn', 'run')),
    estimation TEXT NOT NULL CHECK (estimation IN ('measured', 'estimated')),
    finality TEXT NOT NULL CHECK (finality IN ('interim', 'final')),
    input_tokens INTEGER NOT NULL CHECK (input_tokens >= 0),
    cached_input_tokens INTEGER NOT NULL CHECK (cached_input_tokens >= 0),
    output_tokens INTEGER NOT NULL CHECK (output_tokens >= 0),
    reasoning_tokens INTEGER NOT NULL CHECK (reasoning_tokens >= 0),
    observed_at_unix_ms INTEGER NOT NULL,
    UNIQUE (run_id, measurement)
);

CREATE INDEX usage_observations_by_time ON usage_observations (observed_at_unix_ms);
CREATE INDEX usage_observations_by_conversation ON usage_observations (conversation_id, observed_at_unix_ms);

-- Provider-reported quota windows, kept as the snapshot history ADR-0045
-- retains rather than as one mutable current value. A window is identified
-- by the Provider's own name for it, and reads select the freshest snapshot
-- per window: separate windows are never summed into one another.
-- `digest` covers the reported content alone, so an unchanged response
-- becomes a heartbeat rather than another row, and moves the answer time
-- of the row it repeats instead of adding one.
CREATE TABLE usage_quota_snapshots (
    snapshot_id TEXT PRIMARY KEY NOT NULL CHECK (length(snapshot_id) = 36),
    binding_id TEXT NOT NULL CHECK (length(binding_id) = 36),
    provider TEXT NOT NULL CHECK (length(provider) <= 64),
    window_id TEXT NOT NULL CHECK (length(window_id) BETWEEN 1 AND 128),
    scope TEXT NOT NULL CHECK (scope IN ('provider_account', 'model')),
    model TEXT CHECK ((scope = 'model') = (model IS NOT NULL)
        AND (model IS NULL OR length(model) <= 128)),
    conversation_id TEXT REFERENCES conversations (conversation_id),
    run_id TEXT REFERENCES runs (run_id),
    -- The unit the Provider stated the window in. A share is hundredths of
    -- a percent out of 10,000, which is what a Provider that reports a
    -- filled fraction rather than a countable limit supplies.
    unit TEXT NOT NULL CHECK (unit IN ('tokens', 'requests', 'credits', 'share')),
    used INTEGER NOT NULL CHECK (used >= 0),
    limit_amount INTEGER CHECK (limit_amount IS NULL OR limit_amount >= 0),
    window_seconds INTEGER CHECK (window_seconds IS NULL OR window_seconds > 0),
    resets_at_unix_ms INTEGER,
    estimation TEXT NOT NULL CHECK (estimation IN ('measured', 'estimated')),
    finality TEXT NOT NULL CHECK (finality IN ('interim', 'final')),
    observed_at_unix_ms INTEGER NOT NULL,
    -- When the Provider last confirmed this window, which is what a Query
    -- calls fresh. An unchanged response is a heartbeat that stores no new
    -- row, so it moves this instead: a window is confirmed by an answer
    -- about that window and never by an answer about another one of the
    -- same binding (ADR-0023, ADR-0045).
    answered_at_unix_ms INTEGER NOT NULL CHECK (answered_at_unix_ms >= observed_at_unix_ms),
    digest BLOB NOT NULL CHECK (length(digest) = 32),
    CHECK (unit <> 'share' OR (limit_amount = 10000 AND used <= 10000))
);

CREATE INDEX usage_quota_snapshots_by_window ON usage_quota_snapshots (binding_id, window_id, observed_at_unix_ms);

-- Whether a binding's Provider answered the last time Jet asked. A Query
-- reports an unreachable Provider as unreachable rather than presenting its
-- last known window as current (ADR-0023).
CREATE TABLE usage_provider_reach (
    binding_id TEXT PRIMARY KEY NOT NULL CHECK (length(binding_id) = 36),
    state TEXT NOT NULL CHECK (state IN ('reachable', 'unreachable')),
    reason TEXT CHECK ((state = 'unreachable') = (reason IS NOT NULL)
        AND (reason IS NULL OR length(reason) BETWEEN 1 AND 256)),
    observed_at_unix_ms INTEGER NOT NULL
);
