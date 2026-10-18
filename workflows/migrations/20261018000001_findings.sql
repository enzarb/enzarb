-- Liveness findings (ADR-0024): observation only. Written by the engine's periodic `platform`
-- pass; read through engine_api.finding_v and engine_api.task_v.

CREATE TABLE engine.findings (
    key text PRIMARY KEY,
    -- NULL for platform-scope findings (one per Project).
    task_id bigint REFERENCES engine.tasks (id) ON DELETE CASCADE,
    project text NOT NULL,
    check_id text NOT NULL,
    kind text NOT NULL CHECK (kind IN ('dead_wait', 'platform_failure')),
    severity text NOT NULL CHECK (severity IN ('info', 'warning', 'critical')),
    severity_rank smallint GENERATED ALWAYS AS (
        CASE severity WHEN 'critical' THEN 3 WHEN 'warning' THEN 2 ELSE 1 END) STORED,
    text text NOT NULL,
    first_seen timestamptz NOT NULL DEFAULT now(),
    last_seen timestamptz NOT NULL DEFAULT now(),
    -- Cleared rows are kept for history; a condition that returns reopens the same key.
    cleared_at timestamptz
);
CREATE INDEX findings_open_task ON engine.findings (task_id) WHERE cleared_at IS NULL;
CREATE INDEX findings_open_project ON engine.findings (project) WHERE cleared_at IS NULL;

-- Consecutive refreshes that left the fact unknown (null); maintained by the fact refresh.
ALTER TABLE engine.facts ADD COLUMN unknown_refreshes int NOT NULL DEFAULT 0;

CREATE VIEW engine_api.finding_v AS
SELECT f.key, f.task_id, f.project, f.check_id, f.kind, f.severity, f.text, f.first_seen, f.last_seen
FROM engine.findings f WHERE f.cleared_at IS NULL;

-- Same columns in the same order, `findings` appended. An interpreter reason always wins; a
-- finding fills in `reason` (its check id) and `text` only when the interpreter has none, so
-- the projection the reducer writes is never touched by findings.
CREATE OR REPLACE VIEW engine_api.task_v AS
SELECT p.task_id, p.project, t.repo, t.workflow_ref, p.title, p.subjects, p.status, p.end_status,
       p.phase, p.node, p."column", p.waiting_on,
       CASE WHEN COALESCE(p.reason, '') = '' THEN top.check_id ELSE p.reason END AS reason,
       CASE WHEN COALESCE(p.reason, '') = '' THEN COALESCE(NULLIF(p.text, ''), top.text) ELSE p.text END AS text,
       p.actions,
       -- artifacts without content (task_detail has the content)
       COALESCE((SELECT jsonb_agg(a - 'content') FROM jsonb_array_elements(p.artifacts) a), '[]') AS artifacts,
       p.needs_you, p.cost_usd, t.created_at, p.updated_at, t.ended_at,
       COALESCE((SELECT jsonb_agg(jsonb_build_object(
                     'key', f.key, 'check', f.check_id, 'kind', f.kind, 'severity', f.severity,
                     'text', f.text, 'first_seen', f.first_seen, 'last_seen', f.last_seen)
                 ORDER BY f.severity_rank DESC, f.first_seen, f.key)
                FROM engine.findings f WHERE f.task_id = p.task_id AND f.cleared_at IS NULL), '[]') AS findings
FROM engine.projection p JOIN engine.tasks t ON t.id = p.task_id
LEFT JOIN LATERAL (
    SELECT f.check_id, f.text FROM engine.findings f
    WHERE f.task_id = p.task_id AND f.cleared_at IS NULL
    ORDER BY f.severity_rank DESC, f.first_seen, f.key LIMIT 1
) top ON true;

CREATE OR REPLACE FUNCTION engine.apply_grants() RETURNS void
LANGUAGE plpgsql SECURITY DEFINER SET search_path = engine, pg_temp AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_workflows') THEN
        GRANT USAGE ON SCHEMA engine, engine_api TO enzarb_workflows;
        GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA engine TO enzarb_workflows;
        GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA engine TO enzarb_workflows;
        GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA engine TO enzarb_workflows;
        GRANT SELECT ON ALL TABLES IN SCHEMA engine_api TO enzarb_workflows;
        GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA engine_api TO enzarb_workflows;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_ui') THEN
        GRANT USAGE ON SCHEMA engine_api TO enzarb_ui;
        GRANT SELECT ON engine_api.task_v, engine_api.finding_v TO enzarb_ui;
        GRANT EXECUTE ON FUNCTION
            engine_api.task_detail(bigint), engine_api.session_v(text, int, int),
            engine_api.session_search(text, text), engine_api.board_v(text),
            engine_api.workflow_v(text, text), engine_api.metrics(text, timestamptz, timestamptz),
            engine_api.submit(bigint, jsonb, text, jsonb),
            engine_api.secret_set(text, text, text, text), engine_api.secret_delete(text, text, text),
            engine_api.secret_list(text, text), engine_api.chat_v(text)
        TO enzarb_ui;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_connectors') THEN
        GRANT USAGE ON SCHEMA engine_api TO enzarb_connectors;
        GRANT EXECUTE ON FUNCTION engine_api.ingest(text, text, text, text, jsonb),
            engine_api.secret_for_receiver(text, text) TO enzarb_connectors;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_realtime') THEN
        EXECUTE format('GRANT CONNECT ON DATABASE %I TO enzarb_realtime', current_database());
        GRANT USAGE ON SCHEMA engine_api TO enzarb_realtime;
        GRANT SELECT ON engine_api.task_v, engine_api.finding_v TO enzarb_realtime;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_sandbox_proxy') THEN
        GRANT USAGE ON SCHEMA engine_api TO enzarb_sandbox_proxy;
        GRANT EXECUTE ON FUNCTION engine_api.chat_record(text, text, text, text),
            engine_api.chat_event(text, text, jsonb) TO enzarb_sandbox_proxy;
    END IF;
END
$$;
SELECT engine.apply_grants();
