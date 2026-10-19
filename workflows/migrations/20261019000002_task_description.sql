-- The task's description: the issue (or, for a pull-request task, the first pull request) the
-- task works on, from the task document. Additive: `task_detail` gains `description`
-- (`{title, body, author, url, state, labels}` or null); `task_v` is unchanged so the board and
-- list stay small. The body is untrusted text: the UI renders it as sanitized markdown.
CREATE OR REPLACE FUNCTION engine_api.task_detail(p_task_id bigint) RETURNS jsonb
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
                           FROM relations r WHERE r.task_id = v.task_id), '[]'),
    'description', (
        SELECT jsonb_build_object(
            'title', COALESCE(s.o->>'title', ''), 'body', COALESCE(s.o->>'body', ''),
            'author', s.o->>'author', 'url', s.o->>'url', 'state', s.o->>'state',
            'labels', COALESCE(s.o->'labels', '[]'))
        FROM (SELECT COALESCE(NULLIF(t.state->'doc'->'subjects'->'issue', 'null'::jsonb),
                              t.state->'doc'->'subjects'->'pull_requests'->0) AS o
                FROM tasks t WHERE t.id = v.task_id) s
        WHERE s.o IS NOT NULL AND jsonb_typeof(s.o) = 'object'))
FROM task_v v WHERE v.task_id = p_task_id
$$;
