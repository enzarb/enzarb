-- Sandbox-boot waits: a job whose Project sandbox is not ready yet goes back to `queued` and
-- polls, with its own time budget instead of spending `max_attempts`.
-- Additive only. `waiting_for` / `resumes_at` (and their views and trigger) already exist
-- from 20261011000002_session_limit.sql and are reused with the value 'sandbox'.

ALTER TABLE workflows.jobs
    -- First "sandbox not ready" for the current start; the boot budget is measured from here.
    -- Cleared once the sandbox accepts the run.
    ADD COLUMN sandbox_wait_started_at timestamptz;
