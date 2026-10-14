-- Token usage and cost per job (docs/contracts.md section 6).
-- Additive only: a new column, and views that append columns.
--
-- `jobs.usage` is the sum over all runs of the job (session-limit waits, restart retries and
-- the recovery run each produce a run). Shape = enzarb-proto `Usage`:
-- {input_tokens?, output_tokens?, cache_read_input_tokens?, cache_creation_input_tokens?,
--  models?: [text], cost_usd?}. A missing key means "not reported", never 0; `cost_usd` is the
-- cost as reported by the agent. NULL when no run reported anything.

ALTER TABLE workflows.jobs
    ADD COLUMN usage      jsonb,
    -- Run ids already summed into `usage`, so a repeated callback is not counted twice.
    ADD COLUMN usage_runs text[] NOT NULL DEFAULT '{}';

CREATE OR REPLACE VIEW workflows_api.job_v AS
    SELECT id, work_item_id, kind, status, attempt, created_at, updated_at, output,
           waiting_for, resumes_at,
           usage
    FROM workflows.jobs;

-- usage_totals: per field, the sum over the item's jobs that reported it; a field no job
-- reported is absent (so cost is absent, not 0, when no job reported a cost). NULL when no
-- job has usage.
CREATE OR REPLACE VIEW workflows_api.work_item_v AS
    SELECT w.id, w.key, w.project_id AS project, w.workflow, w.state, w.state_data,
           w.created_at, w.updated_at,
           wj.waiting_for, wj.resumes_at,
           ut.usage_totals
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

-- The views expose usage, so changes to it must notify (see the header of
-- 20261011000001_notify_work_item_changed.sql). A callback that carries usage often changes only
-- this column (a session-limit wait), so it is listed on its own.
DROP TRIGGER IF EXISTS jobs_notify_upd ON workflows.jobs;
CREATE TRIGGER jobs_notify_upd AFTER UPDATE ON workflows.jobs
    FOR EACH ROW
    WHEN (OLD.status IS DISTINCT FROM NEW.status
       OR OLD.attempt IS DISTINCT FROM NEW.attempt
       OR OLD.run_after IS DISTINCT FROM NEW.run_after
       OR OLD.output IS DISTINCT FROM NEW.output
       OR OLD.work_item_id IS DISTINCT FROM NEW.work_item_id
       OR OLD.waiting_for IS DISTINCT FROM NEW.waiting_for
       OR OLD.resumes_at IS DISTINCT FROM NEW.resumes_at
       OR OLD.usage IS DISTINCT FROM NEW.usage)
    EXECUTE FUNCTION workflows.notify_work_item_changed();
