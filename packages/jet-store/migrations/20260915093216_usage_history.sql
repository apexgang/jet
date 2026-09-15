-- Downsampled Jet-observed consumption (ADR-0045). Raw observations are
-- kept for ninety days, hourly aggregates for one year, and daily
-- aggregates after that. The tiers and their sweeps are the core's; the
-- store keeps the rows and rebuilds them.
--
-- An aggregate is never added to in place. Every write to a raw row marks
-- the hours it touched in `usage_dirty_hours`, and the rebuild recounts
-- those hours from the raw rows under the same deduplication the current
-- totals use: one measurement counts once, and a Run that restates a
-- cumulative total contributes that total and not the turns it covers. A
-- measurement replaced after it was first counted is therefore counted as
-- replaced. An hour whose raw rows are already swept cannot be recounted
-- and stands as it was last built.
CREATE TABLE usage_aggregates (
    resolution TEXT NOT NULL CHECK (resolution IN ('hour', 'day')),
    bucket_start_unix_ms INTEGER NOT NULL,
    binding_id TEXT CHECK (binding_id IS NULL OR length(binding_id) = 36),
    provider TEXT CHECK (provider IS NULL OR length(provider) <= 64),
    model TEXT CHECK (model IS NULL OR length(model) <= 128),
    input_tokens INTEGER NOT NULL CHECK (input_tokens >= 0),
    cached_input_tokens INTEGER NOT NULL CHECK (cached_input_tokens >= 0),
    output_tokens INTEGER NOT NULL CHECK (output_tokens >= 0),
    reasoning_tokens INTEGER NOT NULL CHECK (reasoning_tokens >= 0),
    measurements INTEGER NOT NULL CHECK (measurements >= 0),
    estimated INTEGER NOT NULL CHECK (estimated >= 0),
    interim INTEGER NOT NULL CHECK (interim >= 0)
);

-- A rebuild removes and reinserts whole buckets, which is what keeps one
-- row per bucket and grouping without a UNIQUE that NULL columns would
-- not hold.
CREATE INDEX usage_aggregates_by_bucket ON usage_aggregates (
    resolution, bucket_start_unix_ms);
CREATE INDEX usage_aggregates_by_binding ON usage_aggregates (
    resolution, binding_id, bucket_start_unix_ms);

-- The hours whose raw rows changed since the aggregates were last rebuilt.
CREATE TABLE usage_dirty_hours (
    hour_start_unix_ms INTEGER PRIMARY KEY NOT NULL
);
