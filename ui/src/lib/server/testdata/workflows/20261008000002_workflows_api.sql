-- Public contract (contracts.md section 6). Additive changes only.
CREATE SCHEMA IF NOT EXISTS workflows_api;

-- Views run with the owner's rights, so readers need no access to schema `workflows`.
CREATE VIEW workflows_api.work_item_v AS
    SELECT id, key, project_id AS project, workflow, state, state_data, created_at, updated_at
    FROM workflows.work_items;

CREATE VIEW workflows_api.transition_v AS
    SELECT id, work_item_id, from_state, to_state, event_type, at
    FROM workflows.transitions;

CREATE VIEW workflows_api.job_v AS
    SELECT id, work_item_id, kind, status, attempt, created_at, updated_at, output
    FROM workflows.jobs;

-- Returns the new event id, or NULL when dedupe_key was already ingested.
CREATE FUNCTION workflows_api.ingest_event(
    p_source text, p_dedupe_key text, p_work_item_key text, p_type text, p_payload jsonb
) RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, workflows, pg_temp
AS $$
DECLARE
    v_id bigint;
BEGIN
    INSERT INTO workflows.events (source, dedupe_key, work_item_key, type, payload)
    VALUES (p_source, p_dedupe_key, p_work_item_key, p_type, COALESCE(p_payload, '{}'::jsonb))
    ON CONFLICT (dedupe_key) DO NOTHING
    RETURNING id INTO v_id;

    IF v_id IS NOT NULL THEN
        PERFORM pg_notify('workflow_events', v_id::text);
    END IF;
    RETURN v_id;
END;
$$;

-- UI actions (approve | cancel | retry) become `ui:` events; returns the event id.
-- Event types: approve -> ApprovalGranted, cancel -> CancelRequested, retry -> RetryRequested.
CREATE FUNCTION workflows_api.submit_action(
    p_work_item_id bigint, p_actor jsonb, p_action text, p_payload jsonb
) RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, workflows, pg_temp
AS $$
DECLARE
    v_key  text;
    v_type text;
BEGIN
    v_type := CASE p_action
        WHEN 'approve' THEN 'ApprovalGranted'
        WHEN 'cancel'  THEN 'CancelRequested'
        WHEN 'retry'   THEN 'RetryRequested'
        ELSE NULL
    END;
    IF v_type IS NULL THEN
        RAISE EXCEPTION 'unknown action %', p_action USING ERRCODE = '22023';
    END IF;

    SELECT key INTO v_key FROM workflows.work_items WHERE id = p_work_item_id;
    IF v_key IS NULL THEN
        RAISE EXCEPTION 'work item % not found', p_work_item_id USING ERRCODE = 'no_data_found';
    END IF;

    RETURN workflows_api.ingest_event(
        'ui',
        'ui:' || gen_random_uuid()::text,
        v_key,
        v_type,
        jsonb_build_object('actor', p_actor, 'action', p_action, 'data', COALESCE(p_payload, '{}'::jsonb))
    );
END;
$$;

REVOKE ALL ON FUNCTION workflows_api.ingest_event(text, text, text, text, jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION workflows_api.submit_action(bigint, jsonb, text, jsonb) FROM PUBLIC;

-- Runtime roles are created by pgop in prod; absent roles (bare local DBs) are skipped with a notice.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_connectors') THEN
        GRANT USAGE ON SCHEMA workflows_api TO enzarb_connectors;
        GRANT EXECUTE ON FUNCTION workflows_api.ingest_event(text, text, text, text, jsonb) TO enzarb_connectors;
    ELSE
        RAISE NOTICE 'role enzarb_connectors missing; grants skipped';
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_ui') THEN
        GRANT USAGE ON SCHEMA workflows_api TO enzarb_ui;
        GRANT SELECT ON workflows_api.work_item_v, workflows_api.transition_v, workflows_api.job_v TO enzarb_ui;
        GRANT EXECUTE ON FUNCTION workflows_api.submit_action(bigint, jsonb, text, jsonb) TO enzarb_ui;
    ELSE
        RAISE NOTICE 'role enzarb_ui missing; grants skipped';
    END IF;
END
$$;
