-- Step 4 (ADR-0029 section 8): the executor claims and runs `pending`/`running` runs.

-- Lease: one tracks replica drives a run at a time. The holder renews `lease_until` while it
-- works; a crashed replica's lease expires and another replica resumes the run.
ALTER TABLE tracks.runs
    ADD COLUMN lease_owner    text,
    ADD COLUMN lease_until    timestamptz,
    -- Set by POST /v1/runs/{id}/stop; the driver cancels the run at its next check.
    ADD COLUMN stop_requested boolean NOT NULL DEFAULT false,
    -- created_at + the workflow's `timeout`; past it the run ends failed.
    ADD COLUMN deadline       timestamptz;

CREATE INDEX runs_claim_idx ON tracks.runs (created_at)
    WHERE finished_at IS NULL AND status IN ('pending', 'running');

ALTER TABLE tracks.node_runs
    -- The sandbox step id (`run_id` of step_start); written before the step is sent so a restart
    -- re-attaches with step_status instead of starting it again.
    ADD COLUMN step_run_id text,
    -- The rendered node (Rendered JSON), for the run view.
    ADD COLUMN rendered    jsonb,
    -- `mode` of an agent node, `task`/`run` of a command: duplicated for listing.
    ADD COLUMN mode        text,
    ADD COLUMN git         jsonb;
