-- Why a work item is waiting: workflows derives `state_data.wait`
-- ({waiting_on, reason, text, url?}) on every transition; the view exposes it.
-- Additive: the existing columns keep their names and order. Rows written before this
-- migration have NULLs until their next transition.
CREATE OR REPLACE VIEW workflows_api.work_item_v AS
    SELECT w.id, w.key, w.project_id AS project, w.workflow, w.state, w.state_data,
           w.created_at, w.updated_at,
           wj.waiting_for, wj.resumes_at,
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
    ) wj ON true;
