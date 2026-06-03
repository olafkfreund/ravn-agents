-- Agent registry (#25). Agents auto-register from the events they emit.
CREATE TABLE IF NOT EXISTS agents (
    agent_id    uuid        PRIMARY KEY,
    host        text        NOT NULL,
    first_seen  timestamptz NOT NULL DEFAULT now(),
    last_seen   timestamptz NOT NULL DEFAULT now()
);

-- User-defined key/value labels. A chosen key is the topology grouping dimension.
CREATE TABLE IF NOT EXISTS agent_labels (
    agent_id uuid NOT NULL REFERENCES agents(agent_id) ON DELETE CASCADE,
    key      text NOT NULL,
    value    text NOT NULL,
    PRIMARY KEY (agent_id, key)
);

CREATE INDEX IF NOT EXISTS agent_labels_key_idx ON agent_labels (key, value);
