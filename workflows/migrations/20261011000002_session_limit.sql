-- Session-limit waits: a job whose agent run hit Claude's session limit goes back to `queued`
-- until the limit resets, and holds the other agent jobs of its Project meanwhile.
-- Additive only: new columns, a new table, and views that append columns.

ALTER TABLE workflows.jobs
    -- First session-limit hit for this job; the 7 day cap is measured from here.
    ADD COLUMN limit_wait_started_at timestamptz,
    -- Session-limit waits so far (drives the back-off when no reset time is known).
    ADD COLUMN limit_wait_count      integer NOT NULL DEFAULT 0,
    -- Why a queued job is waiting ('session_limit') and when it is due to run again.
    ADD COLUMN waiting_for           text,
    ADD COLUMN resumes_at            timestamptz;

-- One sandbox and Claude token per Project: agent jobs of a held Project are not claimed
-- until `blocked_until`.
CREATE TABLE workflows.project_holds (
    project_id    text        PRIMARY KEY,
    reason        text        NOT NULL,
    blocked_until timestamptz NOT NULL,
    set_at        timestamptz NOT NULL DEFAULT now()
);

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_workflows') THEN
        GRANT SELECT, INSERT, UPDATE, DELETE ON workflows.project_holds TO enzarb_workflows;
    ELSE
        RAISE NOTICE 'role enzarb_workflows missing; grants skipped';
    END IF;
END
$$;

CREATE OR REPLACE VIEW workflows_api.job_v AS
    SELECT id, work_item_id, kind, status, attempt, created_at, updated_at, output,
           waiting_for, resumes_at
    FROM workflows.jobs;

CREATE OR REPLACE VIEW workflows_api.work_item_v AS
    SELECT w.id, w.key, w.project_id AS project, w.workflow, w.state, w.state_data,
           w.created_at, w.updated_at,
           wj.waiting_for, wj.resumes_at
    FROM workflows.work_items w
    LEFT JOIN LATERAL (
        SELECT j.waiting_for, j.resumes_at
        FROM workflows.jobs j
        WHERE j.work_item_id = w.id AND j.status = 'queued' AND j.waiting_for IS NOT NULL
        ORDER BY j.resumes_at DESC NULLS LAST, j.id DESC
        LIMIT 1
    ) wj ON true;

-- The views expose waiting_for / resumes_at, so changes to them must notify
-- (see the header of 20261011000001_notify_work_item_changed.sql).
DROP TRIGGER IF EXISTS jobs_notify_upd ON workflows.jobs;
CREATE TRIGGER jobs_notify_upd AFTER UPDATE ON workflows.jobs
    FOR EACH ROW
    WHEN (OLD.status IS DISTINCT FROM NEW.status
       OR OLD.attempt IS DISTINCT FROM NEW.attempt
       OR OLD.run_after IS DISTINCT FROM NEW.run_after
       OR OLD.output IS DISTINCT FROM NEW.output
       OR OLD.work_item_id IS DISTINCT FROM NEW.work_item_id
       OR OLD.waiting_for IS DISTINCT FROM NEW.waiting_for
       OR OLD.resumes_at IS DISTINCT FROM NEW.resumes_at)
    EXECUTE FUNCTION workflows.notify_work_item_changed();
