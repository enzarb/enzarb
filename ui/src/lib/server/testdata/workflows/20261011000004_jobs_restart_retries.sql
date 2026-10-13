-- Sandbox-restart retries: an agent run lost because its sandbox restarted is re-queued with
-- backoff without spending the attempt budget; this counts those retries (capped by the
-- executor). Additive only; the workflows_api views are unaffected.

ALTER TABLE workflows.jobs
    ADD COLUMN restart_retries integer NOT NULL DEFAULT 0;
