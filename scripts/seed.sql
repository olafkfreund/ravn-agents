-- SRE Control Plane Database Seed Script
-- Clears and populates mock data for local portal development & testing.

-- Clean up existing tables
TRUNCATE TABLE agent_labels, remediation_records, events, agents CASCADE;

-- 1. Populate Agents (0002_agents.sql & 0004_enrollment.sql)
INSERT INTO agents (agent_id, host, first_seen, last_seen, enrolled_at, cert_not_after)
VALUES
    ('019e9219-6d81-70d2-9651-3a1ee7d149d2', 'ravn-dev', now() - interval '1 day', now() - interval '5 seconds', now() - interval '1 day', now() + interval '364 days'),
    ('019e9219-6d81-70d2-9651-3a1ee7d149d3', 'web-prod-01', now() - interval '2 days', now() - interval '12 seconds', now() - interval '2 days', now() + interval '363 days'),
    ('019e9219-6d81-70d2-9651-3a1ee7d149d4', 'db-prod-01', now() - interval '3 days', now() - interval '8 seconds', now() - interval '3 days', now() + interval '362 days'),
    ('019e9219-6d81-70d2-9651-3a1ee7d149d5', 'frontend-prod-01', now() - interval '3 days', now() - interval '4 minutes', now() - interval '3 days', now() + interval '362 days');

-- 2. Populate Agent Labels
INSERT INTO agent_labels (agent_id, key, value)
VALUES
    ('019e9219-6d81-70d2-9651-3a1ee7d149d2', 'env', 'dev'),
    ('019e9219-6d81-70d2-9651-3a1ee7d149d2', 'role', 'agent'),
    ('019e9219-6d81-70d2-9651-3a1ee7d149d3', 'env', 'prod'),
    ('019e9219-6d81-70d2-9651-3a1ee7d149d3', 'role', 'web'),
    ('019e9219-6d81-70d2-9651-3a1ee7d149d4', 'env', 'prod'),
    ('019e9219-6d81-70d2-9651-3a1ee7d149d4', 'role', 'db'),
    ('019e9219-6d81-70d2-9651-3a1ee7d149d5', 'env', 'prod'),
    ('019e9219-6d81-70d2-9651-3a1ee7d149d5', 'role', 'web');

-- 3. Populate Events (0001_events.sql & 0003_agent_health.sql)
INSERT INTO events (id, occurred_at, observed_at, agent_id, host, severity, source, title, category_hints, payload, explanation)
VALUES
    -- Event 1: Failed systemd service on ravn-dev
    ('019eac89-0fbe-7320-9f66-79393c50afe1', now() - interval '1 hour', now() - interval '1 hour', '019e9219-6d81-70d2-9651-3a1ee7d149d2', 'ravn-dev', 'error', 'failed_unit', 'Systemd unit flaky.service failed', ARRAY['systemd', 'flaky.service'], 
     '{"kind": "failed_unit", "unit": "flaky.service", "result": "exit-code", "recent_log": ["Jun 09 13:18:40 ravn-dev systemd[1]: flaky.service: Failed with result exit-code."]}', 
     '{"explanation": "The flaky.service systemd unit failed with exit-code. This is a known unstable testing service.", "suggested_check": "systemctl status flaky.service"}'),

    -- Event 2: Kubernetes Crashloop pod on ravn-dev
    ('019eac89-15de-7cc2-8652-89c5fd515aef', now() - interval '45 minutes', now() - interval '45 minutes', '019e9219-6d81-70d2-9651-3a1ee7d149d2', 'ravn-dev', 'error', 'kube_workload', 'Kubernetes pod crasher in crash loop', ARRAY['kubernetes', 'crasher', 'crashloop'],
     '{"kind": "kube_workload", "namespace": "ravn-test", "object_kind": "Pod", "name": "crasher", "reason": "CrashLoopBackOff", "message": "Back-off restarting failed container boom in pod crasher", "count": 12}',
     '{"explanation": "The Kubernetes pod crasher in namespace ravn-test entered CrashLoopBackOff. The container is crashing continuously.", "suggested_check": "kubectl logs crasher -n ravn-test"}'),

    -- Event 3: Nginx config drift on web-prod-01
    ('019eac89-0fd7-7e81-8b2a-ff833bcf2a82', now() - interval '30 minutes', now() - interval '30 minutes', '019e9219-6d81-70d2-9651-3a1ee7d149d3', 'web-prod-01', 'warning', 'config_drift', 'Configuration drift in /etc/nginx/nginx.conf', ARRAY['config', 'nginx'],
     '{"kind": "config_drift", "path": "/etc/nginx/nginx.conf", "old_hash": "b2c3d4", "new_hash": "a1b2c3", "diff": "@@ -5,4 +5,4 @@\n-  worker_connections 768;\n+  worker_connections 1024;"}',
     NULL),

    -- Event 4: DB Connection pool exhausted on db-prod-01
    ('019eac89-0fd7-7e81-8b2a-ff833bcf2a85', now() - interval '15 minutes', now() - interval '15 minutes', '019e9219-6d81-70d2-9651-3a1ee7d149d4', 'db-prod-01', 'critical', 'journald', 'PostgreSQL connection pool exhausted', ARRAY['database', 'postgres'],
     '{"kind": "journald", "unit": "postgresql.service", "priority": 3, "message": "FATAL: remaining connection slots are reserved for non-replication superuser connections"}',
     '{"explanation": "PostgreSQL has run out of connection slots. Active application clients are unable to establish database sessions.", "suggested_check": "SELECT * FROM pg_stat_activity;"}');

-- 4. Populate Remediation Records (0005_remediations.sql)
INSERT INTO remediation_records (proposal_id, proposal_created_at, agent_id, host, template_id, risk_tier, event_id, proposal, decision_state, decision_json, command_id, command_signature, result_json, fault_signature)
VALUES
    -- Remediation 1: Already resolved (flaky.service restart)
    ('019ead4c-9a75-7b72-beb2-d91bd2a537be', now() - interval '1 hour', '019e9219-6d81-70d2-9651-3a1ee7d149d2', 'ravn-dev', 'failed-unit-restart', 'safe', '019eac89-0fbe-7320-9f66-79393c50afe1',
     '{"id": "019ead4c-9a75-7b72-beb2-d91bd2a537be", "event_id": "019eac89-0fbe-7320-9f66-79393c50afe1", "agent_id": "019e9219-6d81-70d2-9651-3a1ee7d149d2", "host": "ravn-dev", "template_id": "failed-unit-restart", "template_version": 1, "risk_tier": "safe", "params": {"unit": "flaky.service"}, "rationale": "Restart flaky.service which is in failed state.", "created_at": "2026-06-22T08:00:00Z"}',
     'approved',
     '{"decision": "approved", "by": {"kind": "human", "user": "sre-ops-admin", "approved_at": "2026-06-22T08:01:00Z"}}',
     '019ead4c-9a75-7b72-beb2-d91bd2a537bf',
     'dGVzdC1zaWduYXR1cmUtZWRlc3NhLWNvbW1hbmQtZW52ZWxvcGUtc2lnbmVkLWJ5LXNhZmUta2V5Cg==',
     '{"command_id": "019ead4c-9a75-7b72-beb2-d91bd2a537bf", "status": "succeeded", "detail": "systemd unit flaky.service restarted successfully", "observed_state": "active", "finished_at": "2026-06-22T08:01:05Z"}',
     'fault-sig-flaky-service-failed-unit'),

    -- Remediation 2: Pending (Crashlooping Pod)
    ('019ead4c-da37-72b0-ae7e-69ce97026245', now() - interval '45 minutes', '019e9219-6d81-70d2-9651-3a1ee7d149d2', 'ravn-dev', 'k8s-pod-restart', 'safe', '019eac89-15de-7cc2-8652-89c5fd515aef',
     '{"id": "019ead4c-da37-72b0-ae7e-69ce97026245", "event_id": "019eac89-15de-7cc2-8652-89c5fd515aef", "agent_id": "019e9219-6d81-70d2-9651-3a1ee7d149d2", "host": "ravn-dev", "template_id": "k8s-pod-restart", "template_version": 1, "risk_tier": "safe", "params": {"pod": "crasher", "namespace": "ravn-test"}, "rationale": "Delete crashlooping pod crasher to force controller recreation.", "created_at": "2026-06-22T08:15:00Z"}',
     'pending',
     '{"decision": "pending"}',
     NULL, NULL, NULL,
     'fault-sig-crasher-pod-crashloop');
