-- Why a work item is waiting: workflows derives `state_data.wait`
-- ({waiting_on, reason, text, url?}) on every transition; the view exposes it.
-- Additive: the existing columns keep their names and order. Rows written before this
-- migration have NULLs until their next transition.
CREATE OR REPLACE VIEW workflows_api.work_item_v AS
    SELECT w.id, w.key, w.project_id AS project, w.workflow, w.state, w.state_data,
           w.created_at, w.updated_at,
           wj.waiting_for, wj.resumes_at,
           ut.usage_totals,
           w.state_data #>> '{wait,waiting_on}' AS waiting_on,
           w.state_data #>> '{wait,reason}' AS wait_reason,
           w.state_data #>> '{wait,text}' AS wait_text
    FROM workflows.work_items w
    LEFT JOIN LATERAL (
        SELECT j.waiting_for, j.resumes_at
        FROM workflows.jobs j
        WHERE j.work_item_id = w.id AND j.status = 'queued' AND j.waiting_for IS NOT NULL
        ORDER BY j.resumes_at DESC NULLS LAST, j.id DESC
        LIMIT 1
    ) wj ON true
    LEFT JOIN LATERAL (
        SELECT NULLIF(jsonb_strip_nulls(jsonb_build_object(
                   'input_tokens',                sum((u.usage->>'input_tokens')::bigint),
                   'output_tokens',               sum((u.usage->>'output_tokens')::bigint),
                   'cache_read_input_tokens',     sum((u.usage->>'cache_read_input_tokens')::bigint),
                   'cache_creation_input_tokens', sum((u.usage->>'cache_creation_input_tokens')::bigint),
                   'cost_usd',                    sum((u.usage->>'cost_usd')::numeric),
                   'models', (
                       SELECT jsonb_agg(DISTINCT m.model)
                       FROM workflows.jobs m_j
                       CROSS JOIN LATERAL jsonb_array_elements_text(
                           CASE WHEN jsonb_typeof(m_j.usage->'models') = 'array'
                                THEN m_j.usage->'models' ELSE '[]'::jsonb END) AS m(model)
                       WHERE m_j.work_item_id = w.id
                   )
               )), '{}'::jsonb) AS usage_totals
        FROM workflows.jobs u
        WHERE u.work_item_id = w.id AND u.usage IS NOT NULL
    ) ut ON true;
