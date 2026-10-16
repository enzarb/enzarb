-- ADR-0022 read and write API (docs/workflows/contracts-v2.md). Views run with the owner's
-- rights; functions are SECURITY DEFINER with a pinned search_path.

CREATE VIEW engine_api.task_v AS
SELECT p.task_id, p.project, p.repo, p.workflow, p.title, p.subjects, p.status, p.end_status,
       p.phase, p.node, p."column", p.waiting_on, p.reason, p.text, p.actions,
       -- artifacts without content (task_detail has the content)
       COALESCE((SELECT jsonb_agg(a - 'content') FROM jsonb_array_elements(p.artifacts) a), '[]') AS artifacts,
       p.needs_you, p.cost_usd, p.created_at, p.updated_at
FROM engine.projection p;

-- For realtime: map a NOTIFY payload to its Project.
CREATE VIEW engine_api.task_project_v AS
SELECT task_id AS id, project FROM engine.projection;

CREATE FUNCTION engine_api.task_detail(p_task_id bigint) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = engine, pg_temp AS $$
SELECT jsonb_build_object(
    'projection', to_jsonb(p),
    'transitions', COALESCE((
        SELECT jsonb_agg(jsonb_build_object(
            'id', t.id, 'event_id', t.event_id, 'event_type', e.type, 'at', t.at,
            'from', t.from_state, 'to', t.to_state,
            'effects', (SELECT COALESCE(jsonb_agg(x->>'effect'), '[]') FROM jsonb_array_elements(t.effects) x))
            ORDER BY t.id)
        FROM transitions t LEFT JOIN events e ON e.id = t.event_id
        WHERE t.task_id = p.task_id), '[]'),
    'step_runs', COALESCE((
        SELECT jsonb_agg(to_jsonb(r) - 'stall_reported_at' ORDER BY r.queued_at)
        FROM step_runs r WHERE r.task_id = p.task_id), '[]'),
    'artifacts', p.artifacts,
    'facts', COALESCE((SELECT jsonb_object_agg(f.name, f.value) FROM facts f WHERE f.task_id = p.task_id), '{}'),
    'relations', COALESCE((SELECT jsonb_agg(to_jsonb(r)) FROM relations r WHERE r.task_id = p.task_id), '[]'))
FROM projection p WHERE p.task_id = p_task_id
$$;

CREATE FUNCTION engine_api.session_v(p_run_id text, p_after_seq int DEFAULT 0, p_limit int DEFAULT 500)
RETURNS TABLE (run_id text, seq int, at timestamptz, kind text, payload jsonb, truncated boolean)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = engine, pg_temp AS $$
SELECT s.run_id, s.seq, s.at, s.kind, s.payload, s.truncated FROM session_events s
WHERE s.run_id = p_run_id AND s.seq > p_after_seq
ORDER BY s.seq LIMIT LEAST(GREATEST(p_limit, 1), 5000)
$$;

CREATE FUNCTION engine_api.session_search(p_project text, p_query text)
RETURNS TABLE (run_id text, seq int, at timestamptz, kind text, task_id bigint)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = engine, pg_temp AS $$
SELECT s.run_id, s.seq, s.at, s.kind, r.task_id FROM session_events s
JOIN step_runs r ON r.run_id = s.run_id
WHERE r.project = p_project AND s.payload::text ILIKE '%' || p_query || '%'
ORDER BY s.at DESC LIMIT 200
$$;

CREATE FUNCTION engine_api.board_v(p_project text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = engine, pg_temp AS $$
SELECT r.board FROM resolutions r WHERE r.project = p_project ORDER BY r.updated_at DESC LIMIT 1
$$;

CREATE FUNCTION engine_api.workflow_v(p_project text, p_repo text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = engine, pg_temp AS $$
SELECT jsonb_build_object('bindings', r.bindings, 'workflows', r.workflows, 'errors', r.errors,
                          'updated_at', r.updated_at)
FROM resolutions r WHERE r.project = p_project AND r.repo = lower(p_repo)
$$;

CREATE FUNCTION engine_api.metrics(p_project text, p_from timestamptz, p_to timestamptz)
RETURNS TABLE (workflow text, phase text, step text, runs bigint, failures bigint,
               active_secs double precision, queue_secs double precision, cost_usd numeric,
               resumes bigint, stalls bigint, steers bigint)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = engine, pg_temp AS $$
SELECT t.workflow_ref, split_part(r.node, '/', 1), r.step, count(*),
       count(*) FILTER (WHERE r.exit IS DISTINCT FROM 'success'),
       sum(r.active_secs), sum(r.queue_secs), COALESCE(sum(r.cost_usd), 0),
       sum(r.resumes), sum(r.stalls), sum(r.steers)
FROM step_runs r JOIN tasks t ON t.id = r.task_id
WHERE r.project = p_project AND r.queued_at >= p_from AND r.queued_at < p_to
GROUP BY 1, 2, 3 ORDER BY 1, 2, 3
$$;

-- The UI's only write path: a catalog event (source `ui`) for the task. The engine decides.
CREATE FUNCTION engine_api.submit(p_task_id bigint, p_actor jsonb, p_kind text, p_data jsonb)
RETURNS bigint
LANGUAGE plpgsql SECURITY DEFINER SET search_path = engine, pg_temp AS $$
DECLARE
    t tasks;
    body text;
    cmd text;
    args jsonb := '[]';
    payload jsonb;
    ty text := 'comment';
    id bigint;
BEGIN
    SELECT * INTO t FROM tasks WHERE tasks.id = p_task_id;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'no task %', p_task_id USING ERRCODE = 'no_data_found';
    END IF;
    IF p_actor->>'login' IS NULL OR p_actor->>'login' = '' THEN
        RAISE EXCEPTION 'actor.login is required' USING ERRCODE = 'invalid_parameter_value';
    END IF;
    CASE p_kind
        WHEN 'command' THEN
            cmd := lower(p_data->>'command');
            IF cmd IS NULL OR cmd !~ '^[a-z0-9_-]+$' THEN
                RAISE EXCEPTION 'invalid command' USING ERRCODE = 'invalid_parameter_value';
            END IF;
            args := COALESCE(p_data->'args', '[]');
            body := '/' || cmd || COALESCE((SELECT ' ' || string_agg(a, ' ') FROM jsonb_array_elements_text(args) a), '');
        WHEN 'comment', 'steer' THEN
            body := COALESCE(p_data->>'body', '');
        WHEN 'cancel', 'stop' THEN
            cmd := p_kind;
            body := '/' || p_kind;
        WHEN 'resume' THEN
            ty := 'engine:resume';
        ELSE
            RAISE EXCEPTION 'unknown kind %', p_kind USING ERRCODE = 'invalid_parameter_value';
    END CASE;
    IF ty = 'comment' THEN
        payload := jsonb_build_object('type', 'comment', 'body', body)
            || CASE WHEN cmd IS NULL THEN '{}'::jsonb ELSE jsonb_build_object('command', cmd, 'args', args) END;
    ELSE
        payload := jsonb_build_object('kind', 'resume');
    END IF;
    payload := payload || jsonb_build_object(
        'id', 'ui:' || gen_random_uuid()::text,
        'repo', t.repo,
        -- can_write is filled by the engine from GitHub, never from here.
        'actor', jsonb_build_object('login', p_actor->>'login', 'can_write', false, 'bot', false),
        'at', to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'),
        'source', 'ui');
    INSERT INTO events (dedupe_key, project, task_id, type, payload)
    VALUES (payload->>'id', t.project, t.id, ty, payload)
    RETURNING events.id INTO id;
    RETURN id;
END
$$;

-- Connector receiver: one catalog event into the inbox. NULL when the dedupe key was seen.
CREATE FUNCTION engine_api.ingest(p_dedupe_key text, p_project text, p_subject_key text,
                                  p_type text, p_payload jsonb) RETURNS bigint
LANGUAGE sql SECURITY DEFINER SET search_path = engine, pg_temp AS $$
INSERT INTO events (dedupe_key, project, subject_key, type, payload)
VALUES (p_dedupe_key, p_project, p_subject_key, p_type, p_payload)
ON CONFLICT (dedupe_key) DO NOTHING
RETURNING id
$$;

CREATE FUNCTION engine_api.secret_set(p_scope text, p_owner text, p_name text, p_plaintext text)
RETURNS void
LANGUAGE sql SECURITY DEFINER SET search_path = engine, pg_temp AS $$
INSERT INTO secrets (scope, owner, name, ciphertext, pending, updated_at)
VALUES (p_scope, COALESCE(p_owner, ''), p_name, NULL, p_plaintext, now())
ON CONFLICT (scope, owner, name) DO UPDATE
SET ciphertext = NULL, pending = EXCLUDED.pending, updated_at = now()
$$;

CREATE FUNCTION engine_api.secret_delete(p_scope text, p_owner text, p_name text) RETURNS void
LANGUAGE sql SECURITY DEFINER SET search_path = engine, pg_temp AS $$
DELETE FROM secrets WHERE scope = p_scope AND owner = COALESCE(p_owner, '') AND name = p_name
$$;

CREATE FUNCTION engine_api.secret_list(p_scope text, p_owner text)
RETURNS TABLE (name text, updated_at timestamptz)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = engine, pg_temp AS $$
SELECT s.name, s.updated_at FROM secrets s
WHERE s.scope = p_scope AND s.owner = COALESCE(p_owner, '') ORDER BY s.name
$$;

-- Receiver only (custom-event HMAC): the Project-scope secret `p_name`.
CREATE FUNCTION engine_api.secret_for_receiver(p_project text, p_name text)
RETURNS TABLE (ciphertext bytea, pending text)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = engine, pg_temp AS $$
SELECT s.ciphertext, s.pending FROM secrets s
WHERE s.scope = 'project' AND s.owner = p_project AND s.name = p_name
$$;

CREATE FUNCTION engine_api.chat_v(p_user text)
RETURNS TABLE (run_id text, project text, title text, created_at timestamptz, updated_at timestamptz)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = engine, pg_temp AS $$
SELECT c.run_id, c.project, c.title, c.created_at, c.updated_at FROM chats c
WHERE c."user" = p_user ORDER BY c.updated_at DESC
$$;

REVOKE ALL ON ALL FUNCTIONS IN SCHEMA engine_api FROM PUBLIC;

-- Grants, guarded: pgop creates the runtime roles.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_workflows') THEN
        GRANT USAGE ON SCHEMA engine, engine_api TO enzarb_workflows;
        GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA engine TO enzarb_workflows;
        GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA engine TO enzarb_workflows;
        GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA engine TO enzarb_workflows;
        GRANT SELECT ON ALL TABLES IN SCHEMA engine_api TO enzarb_workflows;
        GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA engine_api TO enzarb_workflows;
        ALTER DEFAULT PRIVILEGES IN SCHEMA engine GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO enzarb_workflows;
        ALTER DEFAULT PRIVILEGES IN SCHEMA engine GRANT USAGE, SELECT ON SEQUENCES TO enzarb_workflows;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_ui') THEN
        GRANT USAGE ON SCHEMA engine_api TO enzarb_ui;
        GRANT SELECT ON engine_api.task_v, engine_api.task_project_v TO enzarb_ui;
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
        GRANT USAGE ON SCHEMA engine_api TO enzarb_realtime;
        GRANT SELECT ON engine_api.task_project_v TO enzarb_realtime;
    END IF;
END
$$;
