-- Normalized detection events (the persisted form of ravn_core::Message).
--
-- Range-partitioned on occurred_at for time-series scale. A partitioned table
-- requires every unique constraint to include the partition key, hence the
-- composite primary key (id, occurred_at).
CREATE TABLE IF NOT EXISTS events (
    id             uuid        NOT NULL,
    occurred_at    timestamptz NOT NULL,
    observed_at    timestamptz NOT NULL,
    received_at    timestamptz NOT NULL DEFAULT now(),
    agent_id       uuid        NOT NULL,
    host           text        NOT NULL,
    severity       text        NOT NULL,
    source         text        NOT NULL,
    title          text        NOT NULL,
    category_hints text[]      NOT NULL DEFAULT '{}',
    payload        jsonb       NOT NULL,
    explanation    jsonb,
    PRIMARY KEY (id, occurred_at)
) PARTITION BY RANGE (occurred_at);

-- Catch-all partition so inserts always land somewhere. Time-bucketed
-- partitions and retention are layered on later (observability epic #8).
CREATE TABLE IF NOT EXISTS events_default PARTITION OF events DEFAULT;

CREATE INDEX IF NOT EXISTS events_agent_occurred_idx ON events (agent_id, occurred_at DESC);
CREATE INDEX IF NOT EXISTS events_severity_idx       ON events (severity);
CREATE INDEX IF NOT EXISTS events_source_idx         ON events (source);
CREATE INDEX IF NOT EXISTS events_received_idx       ON events (received_at DESC);
