-- Step 5 (ADR-0029 section 8): `action` and `wait` nodes.

ALTER TABLE tracks.node_runs
    -- The GitHub call an `action` node is about to make (action, repo, rendered fields, the
    -- idempotency marker). Written before the call, so a restart knows the call may have happened.
    ADD COLUMN intent jsonb,
    -- A `wait` node: {on, if, timeout_secs, until, resumed_by_event, timed_out}.
    ADD COLUMN wait   jsonb;

ALTER TABLE tracks.runs
    -- The node_runs row of the wait the run is parked in (status `waiting`).
    ADD COLUMN wait_node_run_id bigint,
    -- When the parked wait times out (NULL: only the run's deadline bounds it).
    ADD COLUMN wait_until       timestamptz,
    -- Set (together with status `running`) by the router on a matching event or by the timeout
    -- sweep: {outputs: {event, timed_out}, event_id}. The executor applies it with `advance`.
    ADD COLUMN wait_result      jsonb;

CREATE INDEX runs_waiting_idx ON tracks.runs (repo) WHERE status = 'waiting';

ALTER TABLE tracks.event_decisions
    -- Runs this event resumed out of a `wait` node: [{run, workflow, node}].
    ADD COLUMN resumed jsonb NOT NULL DEFAULT '[]'::jsonb;
ALTER TABLE tracks.event_decisions DROP CONSTRAINT event_decisions_outcome_check;
ALTER TABLE tracks.event_decisions
    ADD CONSTRAINT event_decisions_outcome_check
    CHECK (outcome IN ('started', 'no_match', 'skipped', 'error', 'resumed'));
