-- Agent enrollment bookkeeping (#19): when an agent exchanged its bootstrap
-- token for a client certificate, and when that certificate expires.
ALTER TABLE agents
    ADD COLUMN IF NOT EXISTS enrolled_at     timestamptz,
    ADD COLUMN IF NOT EXISTS cert_not_after  timestamptz;
