-- ADR-0022 read and write API (docs/workflows/contracts-v2.md). Views run with the owner's
-- rights; functions are SECURITY DEFINER with a pinned search_path.

CREATE VIEW engine_api.task_v AS
SELECT p.task_id, p.project, t.repo, t.workflow_ref, p.title, p.subjects, p.status, p.end_status,
       p.phase, p.node, p."column", p.waiting_on, p.reason, p.text, p.actions,
       -- artifacts without content (task_detail has the content)
       COALESCE((SELECT jsonb_agg(a - 'content') FROM jsonb_array_elements(p.artifacts) a), '[]') AS artifacts,
       p.needs_you, p.cost_usd, t.created_at, p.updated_at, t.ended_at
FROM engine.projection p JOIN engine.tasks t ON t.id = p.task_id;

CREATE FUNCTION engine_api.task_detail(p_task_id bigint) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = engine, engine_api, pg_temp AS $$
SELECT to_jsonb(v) - 'artifacts' || jsonb_build_object(
    'artifacts', (SELECT p.artifacts FROM projection p WHERE p.task_id = v.task_id),
    'transitions', COALESCE((
        SELECT jsonb_agg(jsonb_build_object(
            'at', tr.at, 'event_type', e.type,
            'from', jsonb_build_object('status', tr.from_state->>'status', 'node', tr.from_state->>'node'),
            'to', jsonb_build_object('status', tr.to_state->>'status', 'node', tr.to_state->>'node'),
            'effects', jsonb_array_length(tr.effects)) ORDER BY tr.id)
        FROM transitions tr LEFT JOIN events e ON e.id = tr.event_id
        WHERE tr.task_id = v.task_id), '[]'),
    'step_runs', COALESCE((
        SELECT jsonb_agg(to_jsonb(s) - 'task_id' - 'stall_reported_at' ORDER BY s.started_at NULLS LAST, s.run_id)
        FROM step_runs s WHERE s.task_id = v.task_id), '[]'),
    'facts', COALESCE((SELECT jsonb_object_agg(f.name, f.value) FROM facts f WHERE f.task_id = v.task_id), '{}'),
    'relations', COALESCE((SELECT jsonb_agg(jsonb_build_object('kind', r.kind, 'target', r.target_key))
                           FROM relations r WHERE r.task_id = v.task_id), '[]'))
FROM task_v v WHERE v.task_id = p_task_id
$$;

CREATE FUNCTION engine_api.session_v(p_run_id text, p_after_seq int, p_limit int)
RETURNS TABLE (seq int, at timestamptz, kind text, payload jsonb, truncated boolean)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = engine, pg_temp AS $$
SELECT s.seq, s.at, s.kind, s.payload, s.truncated FROM session_events s
WHERE s.run_id = p_run_id AND s.seq > p_after_seq
ORDER BY s.seq LIMIT LEAST(GREATEST(p_limit, 1), 5000)
$$;

CREATE FUNCTION engine_api.session_search(p_project text, p_query text)
RETURNS TABLE (run_id text, task_id bigint, seq int, at timestamptz, kind text, snippet text)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = engine, pg_temp AS $$
SELECT e.run_id, s.task_id, e.seq, e.at, e.kind, left(e.payload::text, 200)
FROM session_events e JOIN step_runs s ON s.run_id = e.run_id
WHERE s.project = p_project AND e.payload::text ILIKE '%' || p_query || '%'
ORDER BY e.at DESC LIMIT 100
$$;

CREATE FUNCTION engine_api.board_v(p_project text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = engine, pg_temp AS $$
SELECT COALESCE((SELECT r.board FROM resolutions r WHERE r.project = p_project
                 ORDER BY r.updated_at DESC LIMIT 1), '{"columns": []}')
$$;

CREATE FUNCTION engine_api.workflow_v(p_project text, p_repo text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = engine, pg_temp AS $$
SELECT r.view FROM resolutions r WHERE r.project = p_project AND r.repo = lower(p_repo)
$$;

CREATE FUNCTION engine_api.metrics(p_project text, p_from timestamptz, p_to timestamptz)
RETURNS TABLE (workflow text, phase text, step text, runs bigint, avg_wall_secs double precision,
               active_secs double precision, queue_secs double precision, input_tokens bigint,
               output_tokens bigint, cost_usd numeric, retries bigint, resumes bigint, stalls bigint,
               steers bigint)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = engine, pg_temp AS $$
SELECT t.workflow_ref, split_part(s.node, '/', 1), s.step, count(*),
       avg(extract(epoch FROM s.ended_at - s.started_at))::double precision,
       sum(s.active_secs), sum(s.queue_secs),
       sum((s.tokens->>'input')::bigint), sum((s.tokens->>'output')::bigint), sum(s.cost_usd),
       sum(GREATEST(s.attempt - 1, 0)), sum(s.resumes), sum(s.stalls), sum(s.steers)
FROM step_runs s JOIN tasks t ON t.id = s.task_id
WHERE s.project = p_project AND s.started_at >= p_from AND s.started_at < p_to
GROUP BY 1, 2, 3 ORDER BY 1, 2, 3
$$;

-- The UI's only write path. The engine turns `ui.<kind>` into a catalog event (or a resume)
-- and decides everything; nothing is validated against task state here.
CREATE FUNCTION engine_api.submit(p_task_id bigint, p_actor jsonb, p_kind text, p_data jsonb)
RETURNS bigint
LANGUAGE plpgsql SECURITY DEFINER SET search_path = engine, pg_temp AS $$
DECLARE
    id bigint;
BEGIN
    IF p_kind NOT IN ('command', 'comment', 'cancel', 'resume', 'stop', 'steer') THEN
        RAISE EXCEPTION 'unknown kind %', p_kind USING ERRCODE = '22023';
    END IF;
    INSERT INTO events (dedupe_key, project, task_id, type, payload)
    SELECT 'ui:' || gen_random_uuid(), t.project, t.id, 'ui.' || p_kind,
           jsonb_build_object('source', 'ui', 'actor', p_actor, 'kind', p_kind, 'data', COALESCE(p_data, '{}'))
    FROM tasks t WHERE t.id = p_task_id
    RETURNING events.id INTO id;
    IF id IS NULL THEN
        RAISE EXCEPTION 'task % not found', p_task_id USING ERRCODE = 'P0002';
    END IF;
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

-- Stored as `pending`; the engine encrypts it with ENZARB_SECRETS_KEY on its next tick.
CREATE FUNCTION engine_api.secret_set(p_scope text, p_owner text, p_name text, p_plaintext text)
RETURNS void
LANGUAGE sql SECURITY DEFINER SET search_path = engine, pg_temp AS $$
INSERT INTO secrets (scope, owner, name, ciphertext, pending, updated_at)
VALUES (p_scope, COALESCE(p_owner, ''), p_name, NULL, p_plaintext, now())
ON CONFLICT (scope, owner, name) DO UPDATE
SET ciphertext = NULL, pending = EXCLUDED.pending, updated_at = now()
$$;

CREATE FUNCTION engine_api.secret_delete(p_scope text, p_owner text, p_name text) RETURNS boolean
LANGUAGE sql SECURITY DEFINER SET search_path = engine, pg_temp AS $$
WITH d AS (DELETE FROM secrets WHERE scope = p_scope AND owner = COALESCE(p_owner, '') AND name = p_name
           RETURNING 1)
SELECT EXISTS (SELECT 1 FROM d)
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
RETURNS TABLE (run_id text, project text, title text, started_at timestamptz, last_at timestamptz, events bigint)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = engine, pg_temp AS $$
SELECT c.run_id, c.project, c.title, c.started_at,
       (SELECT max(e.at) FROM session_events e WHERE e.run_id = c.run_id),
       (SELECT count(*) FROM session_events e WHERE e.run_id = c.run_id)
FROM chats c WHERE c."user" = p_user ORDER BY c.started_at DESC
$$;

REVOKE ALL ON ALL FUNCTIONS IN SCHEMA engine_api FROM PUBLIC;

-- Grants, guarded: pgop creates the runtime roles (re-asserted by `enzarb-workflows migrate`).
CREATE FUNCTION engine.apply_grants() RETURNS void
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
        GRANT SELECT ON engine_api.task_v TO enzarb_ui;
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
        GRANT SELECT ON engine_api.task_v TO enzarb_realtime;
    END IF;
END
$$;
SELECT engine.apply_grants();
REVOKE ALL ON FUNCTION engine.apply_grants() FROM PUBLIC;
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_workflows') THEN
        GRANT EXECUTE ON FUNCTION engine.apply_grants() TO enzarb_workflows;
    END IF;
END $$;
