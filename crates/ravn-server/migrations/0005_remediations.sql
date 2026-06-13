-- Append-only remediation audit trail (#143).
--
-- One row per proposal; columns are updated in-place at each state transition
-- (Pending → Approved/Rejected → result). This is intentionally NOT a pure
-- event-log — RemediationRecord is the single authoritative view of a
-- lifecycle, and all updates are stamped with updated_at. The no-delete
-- constraint is enforced by the application layer (no DELETE is ever issued).
--
-- proposal / decision / result are stored as JSONB for two reasons:
--   1. The types use internally-tagged serde enums (Decision, ApprovalRef,
--      ActionResult) that map cleanly to JSON but are awkward as flat columns.
--   2. The schema is versioned by ravn-core; JSONB survives additive changes
--      without a migration.
--
-- The table is partitioned on proposal_created_at (the logical time of the
-- fault) to mirror the events table and support future retention policies.
CREATE TABLE IF NOT EXISTS remediation_records (
    -- Primary identity: the UUID embedded in the proposal.
    proposal_id         uuid        NOT NULL,
    proposal_created_at timestamptz NOT NULL,

    -- Denormalized lookup columns — indexed for the API query paths.
    agent_id            uuid        NOT NULL,
    host                text        NOT NULL,
    template_id         text        NOT NULL,
    risk_tier           text        NOT NULL,

    -- The triggering event, for cross-referencing with the events table.
    event_id            uuid        NOT NULL,

    -- Full proposal payload (RemediationProposal JSON).
    proposal            jsonb       NOT NULL,

    -- Current lifecycle state: "pending" | "approved" | "rejected".
    -- Denormalised for fast filtering; the authoritative form is in decision_json.
    decision_state      text        NOT NULL DEFAULT 'pending',

    -- Full Decision enum as JSON (includes ApprovalRef / rejection details).
    decision_json       jsonb       NOT NULL DEFAULT '"pending"',

    -- Set when a command is issued on approval.
    command_id          uuid,
    -- Ed25519 signature of the issued command (base64), for audit.
    command_signature   text,

    -- ActionResult JSON, set when the agent reports back.
    result_json         jsonb,

    -- Fault signature (#118): deterministic hash of the triggering event,
    -- retained so the knowledge base can be updated when the result lands.
    fault_signature     text        NOT NULL DEFAULT '',

    -- Audit timestamps.
    inserted_at         timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),

    PRIMARY KEY (proposal_id, proposal_created_at)
) PARTITION BY RANGE (proposal_created_at);

-- Catch-all partition so inserts always land somewhere.
CREATE TABLE IF NOT EXISTS remediation_records_default
    PARTITION OF remediation_records DEFAULT;

-- Fast lookup by command_id (needed when the agent reports a result).
CREATE INDEX IF NOT EXISTS rem_command_id_idx
    ON remediation_records (command_id)
    WHERE command_id IS NOT NULL;

-- Fast filter for the portal approval queue (pending proposals).
CREATE INDEX IF NOT EXISTS rem_decision_state_idx
    ON remediation_records (decision_state, proposal_created_at DESC);

-- Fast lookup by agent for the portal agent-detail view.
CREATE INDEX IF NOT EXISTS rem_agent_created_idx
    ON remediation_records (agent_id, proposal_created_at DESC);
