-- Agent health snapshot from heartbeats (#20).
ALTER TABLE agents
    ADD COLUMN IF NOT EXISTS last_heartbeat    timestamptz,
    ADD COLUMN IF NOT EXISTS uptime_secs       bigint,
    ADD COLUMN IF NOT EXISTS inference_enabled boolean,
    ADD COLUMN IF NOT EXISTS events_published  bigint;
